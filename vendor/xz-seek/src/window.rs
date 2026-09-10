//! Compressed bytes already in hand, addressed in the file's own coordinates.
//!
//! A [`Window`] is a [`CompressedSource`] over a byte range someone else
//! fetched. It is what lets a block decode be driven over a buffer instead of
//! over a device without inverting the seam: a worker is handed the block and a
//! window holding that block's compressed range, and pulls out of it exactly as
//! it pulls out of a file — see `docs/design/architecture.md`, "The window is a
//! source over bytes someone else fetched".
//!
//! Two properties are the whole of its contract, and both exist so that a
//! mistake in the *caller's* range arithmetic is reported as a mistake:
//!
//! * **Its coordinates are the file's.** [`Window::new`] is told the whole
//!   compressed file's size and reports that from [`CompressedSource::size`], so
//!   nothing renumbers and a window is substitutable where the real source goes.
//! * **It refuses what it does not hold.** A read whose bytes are not in the
//!   window is an [`io::Error`] naming both ranges, never a short return — a
//!   short return means end of file to everything above, and a block decode
//!   turns that into [`Error::Truncated`](crate::Error::Truncated), which would
//!   report a fetch-policy bug as a damaged file.

use std::io;
use std::ops::Range;

use crate::source::CompressedSource;

/// A [`CompressedSource`] over compressed bytes already fetched.
///
/// Generic over the backing so that no shape has to copy to use it: a pooled
/// `Vec<u8>`, an `Arc<[u8]>` shared by the several block decodes one coalesced
/// fetch covers, or a `bytes::Bytes` straight out of an object store all satisfy
/// `AsRef<[u8]>`, and the bound is `std`-only so the unsafe-free graph is
/// untouched. [`Clone`] follows from `B: Clone`, so the shared shapes cost a
/// refcount and a slice and everyone else pays nothing.
///
/// ```
/// use xz_seek::{CompressedSource, Window};
///
/// // Bytes 100..110 of a 1,000-byte file.
/// let window = Window::new(100, 1_000, vec![0u8; 10]);
/// assert_eq!(window.size().unwrap(), 1_000);
/// assert_eq!(window.range(), 100..110);
///
/// let mut buf = [0u8; 4];
/// assert_eq!(window.read_at(106, &mut buf).unwrap(), 4);
/// // Bytes the window does not hold are an error, never a short read.
/// assert!(window.read_at(108, &mut buf).is_err());
/// ```
///
/// **A window may hold any number of blocks**, or part of one, or a stream's
/// tail: a base offset plus bytes says nothing about block boundaries, so
/// nothing here has an opinion about what was fetched.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Window<B: AsRef<[u8]>> {
    base: u64,
    file_size: u64,
    bytes: B,
}

impl<B: AsRef<[u8]>> Window<B> {
    /// A window holding `bytes`, which are the compressed file's bytes starting
    /// at `base`, in a file of `file_size` bytes.
    ///
    /// `file_size` is the **whole file's** size and not the window's: every
    /// offset in a [`SeekTable`](crate::SeekTable) is file-absolute, so a window
    /// that renumbered its coordinate space would invite arithmetic bugs in
    /// exactly the layer that has to get arithmetic right.
    ///
    /// Nothing is checked here. A window claiming bytes past `file_size` is a
    /// caller's contradiction, and the only thing it can produce is a read
    /// answered out of bytes the file does not have.
    pub fn new(base: u64, file_size: u64, bytes: B) -> Self {
        Window {
            base,
            file_size,
            bytes,
        }
    }

    /// The file-absolute range this window holds.
    ///
    /// **The end saturates at `u64::MAX`**, so a window whose `base + len`
    /// overflows still has a range, and a read it cannot answer still reaches
    /// the outside-the-window error in every build profile. A plain `+`
    /// would panic where overflow checks are on and wrap where they are off,
    /// which reports a caller's arithmetic mistake two different ways depending
    /// on how the caller was compiled — the one thing this type exists not to
    /// do. Saturating loses nothing: the bytes past `u64::MAX` are bytes no
    /// offset can name.
    pub fn range(&self) -> Range<u64> {
        self.base..self.base.saturating_add(self.bytes.as_ref().len() as u64)
    }

    /// The bytes themselves.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    /// The backing, returned to whoever owns it.
    ///
    /// A fetch stage recycling buffers needs its buffer back; a window that
    /// swallowed it would put the allocation one layer down, where the caller's
    /// pool cannot reach it.
    pub fn into_inner(self) -> B {
        self.bytes
    }

    /// The error a read outside the window is.
    ///
    /// It names the range asked for **and** the range held, because those two
    /// together are the whole diagnosis: the caller's arithmetic produced one of
    /// them and its fetch produced the other.
    fn outside(&self, offset: u64, end: u64) -> io::Error {
        let held = self.range();
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "xz-seek: a read of {offset}..{end} falls outside the window, \
                 which holds {}..{} of a {}-byte file",
                held.start, held.end, self.file_size
            ),
        )
    }
}

/// The impl bound is the backing's bound and nothing more.
///
/// A thread capability on the backing would enable nothing here: `Send` and
/// `Sync` on `Window<B>` are auto-traits that follow from `B` whatever this impl
/// says, and the bulk read asks for what it needs at its own signature. What a
/// bound here would do is withhold the impl from a non-`Sync` backing such as
/// `Rc<[u8]>` and tax generic code bounded by `AsRef<[u8]>` alone — which is the
/// rule [`CompressedSource`] itself states.
impl<B: AsRef<[u8]>> CompressedSource for Window<B> {
    /// The bytes at `offset`, if the window holds every one the file would give.
    ///
    /// **A short return means end of file and nothing else**, which is the
    /// trait's contract and is why anything else is an error rather than a short
    /// fill. The two are reconciled by asking what the *file* would return: the
    /// requested range clipped to `file_size`. If those bytes are in the window
    /// they are copied out; if that clipped range is empty the file has nothing
    /// there and the answer is `Ok(0)` exactly as a real source's is; otherwise
    /// the window cannot answer and says so.
    ///
    /// So a window ending at the file's end short-returns where the file itself
    /// would, and a window ending anywhere else never short-returns at all.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let asked_end = offset.saturating_add(buf.len() as u64);
        let end = asked_end.min(self.file_size);
        let n = end.saturating_sub(offset);
        if n == 0 {
            return Ok(0);
        }
        let held = self.range();
        if offset < held.start || end > held.end {
            return Err(self.outside(offset, asked_end));
        }
        let lo = (offset - held.start) as usize;
        let n = n as usize;
        buf[..n].copy_from_slice(&self.bytes.as_ref()[lo..lo + n]);
        Ok(n)
    }

    /// The whole compressed file's size, as the constructor was told it.
    fn size(&self) -> io::Result<u64> {
        Ok(self.file_size)
    }

    /// The bytes themselves, whenever the window holds the whole range asked
    /// for and the file has every one of them.
    ///
    /// A window is the shape this method exists for: the bytes are already in
    /// hand, so the copy `read_at` performs is pure loss to a caller that only
    /// wants to read them.
    ///
    /// **The two conditions are `read_at`'s, minus its one concession.** That
    /// method answers a read the window holds *or* a read the file itself would
    /// short-return, and clips to `file_size` to tell them apart; here a range
    /// that
    /// is not held whole and inside the file is simply declined, because
    /// declining costs a copy and short-returning would mean end of file. So
    /// the tail window that short-returns from `read_at` lends nothing over the
    /// same range, and both answers are right.
    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let end = offset.checked_add(len as u64)?;
        if end > self.file_size {
            return None;
        }
        let held = self.range();
        if offset < held.start || end > held.end {
            return None;
        }
        let lo = (offset - held.start) as usize;
        Some(&self.bytes.as_ref()[lo..lo + len])
    }
}

/// The extent and the file, never the bytes: a window is routinely megabytes.
impl<B: AsRef<[u8]>> std::fmt::Debug for Window<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = self.range();
        f.debug_struct("Window")
            .field("base", &held.start)
            .field("end", &held.end)
            .field("file_size", &self.file_size)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window carries itself across a thread boundary, on the two backings
    /// the parallel path will hand it.
    ///
    /// `Send` and `Sync` on `Window<B>` are auto-traits that follow from `B`,
    /// and `Clone + Send + 'static` is what the bulk read's own signature asks
    /// of a source — see `docs/design/architecture.md`, "The window is a source
    /// over bytes someone else fetched". Both are inferred rather than declared,
    /// so nothing in the type's own text would notice a field being added that
    /// broke either, and the consumer that would notice is several slices out.
    ///
    /// A test rather than a bound, for the reason the `CompressedSource` impl
    /// carries none either: a bound on the impl constrains the backing and says
    /// nothing about the window, and asserting the capability in the type system
    /// would tax every caller who does not need it.
    #[test]
    fn a_window_is_send_and_sync_and_satisfies_the_bulk_read_s_bound() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        fn assert_shareable_source<T: Clone + Send + 'static>() {}

        assert_send::<Window<Vec<u8>>>();
        assert_sync::<Window<Vec<u8>>>();
        assert_shareable_source::<Window<Vec<u8>>>();

        assert_send::<Window<std::sync::Arc<[u8]>>>();
        assert_sync::<Window<std::sync::Arc<[u8]>>>();
        assert_shareable_source::<Window<std::sync::Arc<[u8]>>>();
    }

    /// A backing that is neither `Send` nor `Sync` is still a source.
    ///
    /// The `CompressedSource` impl bounds the backing by `AsRef<[u8]>` alone, so
    /// an `Rc<[u8]>` window reads like any other. It is the one shape that fails
    /// to compile if a thread capability is ever added back to that impl, and
    /// nothing else in the crate would notice — every other backing the tests
    /// use is `Send + Sync` already.
    #[test]
    fn a_backing_that_is_neither_send_nor_sync_is_still_a_source() {
        fn read_through_the_trait<S: CompressedSource>(s: &S, offset: u64, len: usize) -> Vec<u8> {
            let mut buf = vec![0u8; len];
            let n = s.read_at(offset, &mut buf).unwrap();
            buf.truncate(n);
            buf
        }

        let bytes: std::rc::Rc<[u8]> = (40u8..50).collect::<Vec<u8>>().into();
        let w = Window::new(40, 100, bytes);
        assert_eq!(w.size().unwrap(), 100);
        assert_eq!(read_through_the_trait(&w, 44, 3), vec![44, 45, 46]);
        assert!(w.read_at(50, &mut [0u8; 1]).is_err());
    }

    fn ten_of_a_hundred() -> Window<Vec<u8>> {
        // Bytes 40..50 of a 100-byte file, each byte its own offset.
        Window::new(40, 100, (40u8..50).collect())
    }

    #[test]
    fn a_read_the_window_holds_is_filled_from_it() {
        let w = ten_of_a_hundred();
        for (offset, len) in [(40u64, 10usize), (40, 1), (49, 1), (44, 3), (45, 5)] {
            let mut buf = vec![0u8; len];
            assert_eq!(w.read_at(offset, &mut buf).unwrap(), len, "{offset}+{len}");
            let want: Vec<u8> = (offset as u8..offset as u8 + len as u8).collect();
            assert_eq!(buf, want, "{offset}+{len}");
        }
        assert_eq!(w.range(), 40..50);
        assert_eq!(w.bytes().len(), 10);
        assert_eq!(w.size().unwrap(), 100);
    }

    #[test]
    fn a_read_the_window_does_not_hold_is_an_error_naming_both_ranges() {
        let w = ten_of_a_hundred();
        // Before it, across its start, across its end, and wholly past it.
        for (offset, len) in [(0u64, 4usize), (38, 4), (48, 4), (60, 4), (50, 1)] {
            let mut buf = vec![0u8; len];
            let e = w
                .read_at(offset, &mut buf)
                .expect_err(&format!("{offset}+{len} is outside 40..50"));
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
            let text = e.to_string();
            assert!(
                text.contains(&format!("{offset}..{}", offset + len as u64))
                    && text.contains("40..50")
                    && text.contains("100-byte"),
                "{text}"
            );
        }
    }

    #[test]
    fn only_a_window_ending_at_the_file_s_end_ever_returns_short() {
        // The same ten bytes, now the tail of a 50-byte file.
        let tail = Window::new(40, 50, (40u8..50).collect::<Vec<u8>>());
        let mut buf = [0u8; 8];
        // Fill-or-EOF: the file ends at 50, so this is the file's own answer.
        assert_eq!(tail.read_at(46, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], &[46, 47, 48, 49]);
        // At and past the end there is nothing to give, and that is not an error.
        assert_eq!(tail.read_at(50, &mut buf).unwrap(), 0);
        assert_eq!(tail.read_at(u64::MAX, &mut buf).unwrap(), 0);

        // The same read against a window that is *not* the file's tail is a
        // fetch-policy bug, and is reported rather than looking like EOF.
        assert!(ten_of_a_hundred().read_at(46, &mut buf).is_err());
    }

    /// The window answers what the file it was cut from would answer.
    ///
    /// The oracle is `impl CompressedSource for &[u8]` over the whole file, and
    /// the axis is every `(offset, len)` that can land differently against a
    /// window's two edges and the file's own end.
    #[test]
    fn a_window_answers_as_the_file_does_wherever_it_can_answer_at_all() {
        let file: Vec<u8> = (0..=255u8).cycle().take(200).collect();
        let whole: &[u8] = &file;

        for (base, len) in [(0usize, 200usize), (0, 60), (64, 72), (140, 60), (200, 0)] {
            let w = Window::new(
                base as u64,
                file.len() as u64,
                file[base..base + len].to_vec(),
            );
            let held = base as u64..(base + len) as u64;

            for offset in (0u64..=210).step_by(3).chain([
                held.start.saturating_sub(1),
                held.start,
                held.end.saturating_sub(1),
                held.end,
                199,
                200,
                201,
            ]) {
                for want in [0usize, 1, 7, 64, 200] {
                    let mut ours = vec![0xAAu8; want];
                    let mut theirs = vec![0xAAu8; want];
                    let n = whole.read_at(offset, &mut theirs).unwrap();
                    match w.read_at(offset, &mut ours) {
                        Ok(got) => {
                            // Answering at all means answering identically.
                            assert_eq!(got, n, "{base}+{len} at {offset}+{want}");
                            assert_eq!(ours, theirs, "{base}+{len} at {offset}+{want}");
                        }
                        Err(_) => {
                            // Refusing means the file had bytes the window did
                            // not hold.
                            let asked = offset..offset + want as u64;
                            assert!(
                                n > 0
                                    && (asked.start < held.start || asked.end.min(200) > held.end),
                                "{base}+{len} refused {offset}+{want}, which it holds"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A window lends exactly what it holds, and never past the file's end.
    ///
    /// The axis is the one the trait's own tests run — `source.rs`'s
    /// `loan_axis` — with the window's two edges added, and the property is
    /// `source.rs`'s `a_loan_agrees_with_a_read`, so a window is held to the
    /// same sentence as every other implementor rather than to its own reading.
    ///
    /// What is asserted beside it is the *positive* half, which no shared
    /// property can carry: a source that declined everything would satisfy the
    /// contract and lend nothing.
    #[test]
    fn a_window_lends_what_it_holds_whole_and_declines_the_rest() {
        use crate::source::{a_loan_agrees_with_a_read, loan_axis};

        let w = ten_of_a_hundred();
        let axis = loan_axis(100).into_iter().chain([
            (39u64, 1usize),
            (39, 2),
            (40, 10),
            (49, 1),
            (49, 2),
            (50, 0),
        ]);
        a_loan_agrees_with_a_read(&w, axis);

        assert_eq!(w.slice_at(40, 10).unwrap(), &(40u8..50).collect::<Vec<_>>());
        assert_eq!(w.slice_at(44, 3).unwrap(), &[44, 45, 46]);
        assert_eq!(w.slice_at(50, 0).unwrap(), b"");

        // Before it, straddling either edge, and wholly past it.
        for (offset, len) in [(0u64, 4usize), (39, 2), (48, 4), (50, 1), (60, 4)] {
            assert_eq!(w.slice_at(offset, len), None, "{offset}+{len}");
        }

        // A tail window short-returns from `read_at` where the file ends and
        // lends nothing over the same range: a loan is whole or it is absent.
        let tail = Window::new(40, 50, (40u8..50).collect::<Vec<u8>>());
        assert_eq!(tail.read_at(46, &mut [0u8; 8]).unwrap(), 4);
        assert_eq!(tail.slice_at(46, 8), None);
        assert_eq!(tail.slice_at(46, 4).unwrap(), &[46, 47, 48, 49]);
        a_loan_agrees_with_a_read(&tail, loan_axis(50));

        // An overflowing extent declines rather than wrapping into a range that
        // appears to fit, which is `range()`'s saturation seen from here.
        let far = Window::new(u64::MAX - 4, u64::MAX, vec![0u8; 10]);
        assert_eq!(far.slice_at(u64::MAX - 4, 8), None);
        assert_eq!(far.slice_at(u64::MAX - 4, 4).unwrap(), &[0, 0, 0, 0]);
        a_loan_agrees_with_a_read(&far, loan_axis(u64::MAX));
    }

    /// A window whose end overflows `u64` is refused, not panicked at.
    ///
    /// `range()` saturates, so this behaves the same under `cargo test` — where
    /// `[profile.dev]` leaves overflow checks on — and under `--release`, where
    /// a plain `+` would wrap instead. Every entry point that reads the extent
    /// is exercised: `range` itself, `read_at`'s refusal, the `outside` error's
    /// text, and `Debug`.
    #[test]
    fn a_window_whose_end_overflows_is_still_refused_rather_than_panicking() {
        let w = Window::new(u64::MAX - 4, u64::MAX, vec![0u8; 10]);
        assert_eq!(w.range(), u64::MAX - 4..u64::MAX);
        assert_eq!(
            format!("{w:?}"),
            format!(
                "Window {{ base: {}, end: {}, file_size: {} }}",
                u64::MAX - 4,
                u64::MAX,
                u64::MAX
            )
        );

        // A read the window does not hold is the error naming both ranges.
        let mut buf = [0u8; 4];
        let e = w.read_at(0, &mut buf).expect_err("0..4 is outside");
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        let text = e.to_string();
        assert!(
            text.contains("0..4") && text.contains(&format!("{}..{}", u64::MAX - 4, u64::MAX)),
            "{text}"
        );

        // And the file's own last four bytes are still answered as the file
        // would answer them, short because the file ends there.
        let mut buf = [0xAAu8; 8];
        assert_eq!(w.read_at(u64::MAX - 4, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn the_backing_comes_back_out_and_a_window_clones_with_it() {
        let w = Window::new(40, 100, vec![7u8; 10]);
        let copy = w.clone();
        assert_eq!(copy.range(), w.range());
        assert_eq!(w.into_inner(), vec![7u8; 10]);

        // Borrowed and shared backings are windows too.
        let bytes = [1u8, 2, 3];
        let borrowed = Window::new(0, 3, &bytes[..]);
        assert_eq!(borrowed.bytes(), &[1, 2, 3]);
        let shared: Window<std::sync::Arc<[u8]>> = Window::new(0, 3, bytes.into());
        assert_eq!(shared.clone().into_inner().len(), 3);

        assert_eq!(
            format!("{:?}", ten_of_a_hundred()),
            "Window { base: 40, end: 50, file_size: 100 }"
        );
    }
}
