//! Where the compressed bytes come from.
//!
//! The crate opens nothing. A caller hands it something that can answer "give
//! me `len` bytes at `offset`" and "how big are you", and that is the whole
//! contract — see `docs/design/roadmap.md`, "The compressed bytes arrive through
//! `CompressedSource`". A local file, a memory map, a ranged HTTP endpoint and
//! an object store all satisfy it; only the first two are implemented here.
//!
//! `&T` and `Arc<T>` are sources whenever `T` is, which is what lets N readers
//! share one — see [`CompressedSource`].
//!
//! A source that already holds its bytes may **lend** them instead of copying
//! them out, through the provided [`CompressedSource::slice_at`]. Declining is
//! always correct, so the whole of the fill-or-EOF machinery stays on
//! [`CompressedSource::read_at`] and a loan can only ever skip a copy.

use std::io;

/// A byte source the reader addresses positionally.
///
/// `&self`, against the reader's `&mut self`, so that N readers can share one
/// source. The trait deliberately carries no `Send`, `Sync` or `Clone` bound:
/// the parallel entry point will ask for what it needs at its own signature
/// rather than taxing every implementor today.
///
/// **Sharing one source is what the blanket impls below are for.** A [`Reader`]
/// takes its source by value, so `&self` on its own does not let two readers
/// exist over one file: the shared shapes are `Reader<&T>` and `Reader<Arc<T>>`,
/// and they compile because `&T` and `Arc<T>` are sources whenever `T` is.
///
/// [`Reader`]: crate::Reader
pub trait CompressedSource {
    /// Fill `buf` with the bytes at `offset`, returning how many were written.
    ///
    /// **Fill-or-EOF.** A return shorter than `buf.len()` means the source ended
    /// there, and means nothing else — never a partial read that a retry would
    /// complete. An implementation over a transport that short-reads must loop
    /// internally, as the [`std::fs::File`] implementation does.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// The source's whole size in bytes.
    ///
    /// Required, not optional: the footer walk starts at the end of the file.
    fn size(&self) -> io::Result<u64>;

    /// The `len` bytes at `offset`, borrowed from the source, when it holds
    /// every one of them.
    ///
    /// **`Some` only for a range the source holds whole *and* that lies inside
    /// its own [`size`](CompressedSource::size).** Both halves are the
    /// implementor's obligation: a source that lends a partly-held range hands a
    /// decode bytes that are not the file's, and one that lends past its own end
    /// invents them. What a consumer can check is the length — the decode
    /// debug-asserts that a `Some` is exactly `len` bytes at the loan site, and
    /// this crate's dev profile keeps debug assertions on — and nothing above
    /// can check that the bytes are the source's own.
    ///
    /// **`None` means *ask [`read_at`](CompressedSource::read_at)*, and never
    /// means end of file.** Declining is always a correct answer — the default
    /// declines every range — so every short-read, truncation and EOF path
    /// stays in `read_at` alone. A loan can skip a copy and cannot introduce an
    /// error case.
    ///
    /// The two answers agree where both exist: wherever this returns `Some(s)`,
    /// `read_at` over the same range fills `s` and returns `s.len()`.
    ///
    /// Implemented by [`Window`](crate::Window) and by `&[u8]`, forwarded by
    /// `&T` and `Arc<T>`, and declined by [`std::fs::File`], which holds
    /// nothing.
    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let _ = (offset, len);
        None
    }
}

/// A source that never lends, wrapping one that would.
///
/// It inherits [`read_at`](CompressedSource::read_at) and
/// [`size`](CompressedSource::size) from the source it wraps and takes
/// [`slice_at`](CompressedSource::slice_at)'s declining default, so a pass over
/// it runs the copying branch of whatever it is handed to. Its use is to turn
/// *a loan cannot change an outcome* from a design claim into a gated one, by
/// putting the same bytes through both branches: without it a borrowing source
/// and a copying one are different sources as well as different branches, and a
/// divergence surfaces as the wrong disagreement.
///
/// **In this crate rather than in `harness`, because a `harness` type cannot
/// satisfy this crate's own trait from inside `src/`** — the dev-dependency
/// cycle links two instances of the library into the unit-test binary and their
/// traits do not unify. See `docs/design/architecture.md`, "The declining
/// wrapper is a `cfg(test)` type in the crate".
///
/// `#[cfg(test)] pub(crate)`, as `src/walk.rs`'s `Counting` and `src/task.rs`'s
/// `Empty` are: the arms that consume it are `src/decode.rs`'s, so nothing
/// outside the unit-test build needs to name it.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Declining<S>(
    /// The source whose loans are being refused.
    pub(crate) S,
);

#[cfg(test)]
impl<S: CompressedSource> CompressedSource for Declining<S> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        self.0.size()
    }

    // `slice_at` is the trait's default, which is the whole point of the type.
}

/// Positioned reads on a file, with no cursor and no `&mut`.
///
/// `FileExt::read_at` is permitted to return short for reasons that are not end
/// of file, so this loops until the buffer is full or a read returns zero. That
/// loop is what makes the trait's fill-or-EOF contract true of a `File`.
///
/// **Unix only.** Windows has the same capability under a different name
/// (`std::os::windows::fs::FileExt::seek_read`) and the impl is eight lines, but
/// nothing here can compile or run it, and shipping a block that has never been
/// built is worse than an absence a compiler names. The crate still builds
/// elsewhere; only this impl is missing.
#[cfg(unix)]
impl CompressedSource for std::fs::File {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match FileExt::read_at(self, &mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

/// A borrowed source is a source, which is what makes `Reader<&File>` compile.
///
/// [`CompressedSource`]'s `&self` says N readers may share one source, and this
/// is the impl that makes the sentence true rather than merely documented: a
/// [`Reader`](crate::Reader) owns its source by value, so two readers over one
/// file are `Reader<&File>` twice over one `File` the caller keeps.
///
/// `?Sized` so that `&dyn CompressedSource` is covered too. It does not overlap
/// the `&[u8]` impl below, because that one is this shape at `T = [u8]` and
/// `[u8]` is not itself a source.
///
/// **`slice_at` forwards like the other two**, or a caller's `&MyMmapSource`
/// would lose the loan without saying so — the provided default is silent where
/// a missing required method is a compile error.
impl<T: CompressedSource + ?Sized> CompressedSource for &T {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        (**self).size()
    }

    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        (**self).slice_at(offset, len)
    }
}

/// An owned share of a source, for the shapes that outlive the call that made
/// them.
///
/// The borrowed impl above cannot serve a value handed to a `'static` thread, so
/// the shared source is an [`Arc`](std::sync::Arc) and the sharing is a cheap
/// clone — `docs/design/roadmap.md`, "The reader owns its decode state".
impl<T: CompressedSource + ?Sized> CompressedSource for std::sync::Arc<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }

    fn size(&self) -> io::Result<u64> {
        (**self).size()
    }

    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        (**self).slice_at(offset, len)
    }
}

/// A file already in memory.
///
/// It holds every byte it has, so it lends every range that fits inside it —
/// which is what makes `&[u8]` the cheapest implementor to check the loan
/// contract against.
impl CompressedSource for &[u8] {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.len());
        let n = (self.len() - start).min(buf.len());
        buf[..n].copy_from_slice(&self[start..start + n]);
        Ok(n)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }

    /// `Some` for every range that lies wholly inside the slice.
    ///
    /// `get` is what states both halves of the contract at once: a range
    /// running past the end is `None` rather than a short loan, and the
    /// `usize`/`checked_add` guards are what stop a 64-bit offset on a 32-bit
    /// target from wrapping into one that appears to fit.
    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let start = usize::try_from(offset).ok()?;
        self.get(start..start.checked_add(len)?)
    }
}

/// Every claim a loan makes, checked against the read over the same range.
///
/// It is the whole of [`CompressedSource::slice_at`]'s contract stated once, so
/// that each implementor is held to the same sentence rather than to its own
/// reading of it: a `Some` is exactly `len` bytes, lies inside the source's own
/// [`size`](CompressedSource::size), and holds what a `read_at` filling the same
/// buffer would have copied — which is also what says the read did not
/// short-return where the loan claimed the range whole.
///
/// It says nothing about a `None`, and cannot: declining is always correct. An
/// implementor that lends is therefore checked by an axis of its own beside
/// this one, or a source that declined everything would pass.
#[cfg(test)]
pub(crate) fn a_loan_agrees_with_a_read<S: CompressedSource>(
    source: &S,
    axis: impl IntoIterator<Item = (u64, usize)>,
) {
    let size = source.size().unwrap();
    for (offset, len) in axis {
        let Some(lent) = source.slice_at(offset, len) else {
            continue;
        };
        assert_eq!(
            lent.len(),
            len,
            "lent {} bytes at {offset}+{len}",
            lent.len()
        );
        assert!(
            offset.saturating_add(len as u64) <= size,
            "lent {offset}+{len} out of a {size}-byte source"
        );
        let mut copied = vec![0xAAu8; len];
        let n = source.read_at(offset, &mut copied).unwrap();
        assert_eq!(
            n, len,
            "read short where the loan was whole, at {offset}+{len}"
        );
        assert_eq!(
            lent,
            &copied[..],
            "the loan and the read disagree at {offset}+{len}"
        );
    }
}

/// The `(offset, len)` axis every loan test runs, over a source of `size`
/// bytes.
///
/// Every edge a loan can land on: the source's start and end, one either side
/// of both, an empty range, and a length that runs past the end from inside.
#[cfg(test)]
pub(crate) fn loan_axis(size: u64) -> Vec<(u64, usize)> {
    let whole = usize::try_from(size).unwrap_or(usize::MAX);
    let mut axis = Vec::new();
    for offset in [
        0u64,
        1,
        size / 2,
        size.saturating_sub(1),
        size,
        size.saturating_add(1),
    ] {
        for len in [0usize, 1, 3, whole, whole.saturating_add(1)] {
            axis.push((offset, len));
        }
    }
    // A 64-bit offset no `usize` can hold, and a range whose end overflows.
    axis.push((u64::MAX, 1));
    axis.push((u64::MAX - 1, 8));
    axis
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slice lends every range inside it and declines every other.
    ///
    /// The declined half is what the assertions are for: `read_at` short-returns
    /// at the end of a slice, and a loan that did the same would be a partial
    /// range handed out as a whole one.
    #[test]
    fn a_slice_lends_what_it_holds_whole_and_declines_the_rest() {
        let data: &[u8] = b"0123456789";
        a_loan_agrees_with_a_read(&data, loan_axis(10));

        assert_eq!(data.slice_at(0, 10).unwrap(), b"0123456789");
        assert_eq!(data.slice_at(6, 4).unwrap(), b"6789");
        assert_eq!(data.slice_at(10, 0).unwrap(), b"");

        // Past the end, straddling it, and offsets no `usize` holds: declined,
        // never short.
        assert_eq!(data.slice_at(8, 4), None);
        assert_eq!(data.slice_at(10, 1), None);
        assert_eq!(data.slice_at(11, 0), None);
        assert_eq!(data.slice_at(u64::MAX, 1), None);
        assert_eq!(data.slice_at(u64::MAX - 1, 8), None);

        let empty: &[u8] = b"";
        assert_eq!(empty.slice_at(0, 0).unwrap(), b"");
        assert_eq!(empty.slice_at(0, 1), None);
    }

    /// `&T` and `Arc<T>` forward the loan rather than taking the default.
    ///
    /// Forwarding is the one part of this contract a compiler cannot notice
    /// going missing: a required method left out is an error, and a provided one
    /// left out is a source that silently stops lending.
    #[test]
    fn a_borrowed_and_a_shared_source_forward_the_loan() {
        let data: &[u8] = b"0123456789";

        let borrowed: &&[u8] = &data;
        assert_eq!(borrowed.slice_at(2, 3).unwrap(), b"234");
        assert_eq!(borrowed.slice_at(8, 4), None);
        a_loan_agrees_with_a_read(&borrowed, loan_axis(10));

        let shared = std::sync::Arc::new(data);
        assert_eq!(shared.slice_at(2, 3).unwrap(), b"234");
        assert_eq!(shared.slice_at(8, 4), None);
        a_loan_agrees_with_a_read(&shared, loan_axis(10));

        // And an erased source keeps lending, which is what `?Sized` buys.
        let erased: &dyn CompressedSource = &data;
        assert_eq!(erased.slice_at(2, 3).unwrap(), b"234");
    }

    /// The wrapper reads like the source it wraps and lends nothing.
    #[test]
    fn the_declining_wrapper_reads_like_its_source_and_never_lends() {
        let data: &[u8] = b"0123456789";
        let declining = Declining(data);

        assert_eq!(declining.size().unwrap(), 10);
        for (offset, len) in loan_axis(10) {
            assert_eq!(declining.slice_at(offset, len), None, "{offset}+{len}");
            let mut ours = vec![0xAAu8; len];
            let mut theirs = vec![0xAAu8; len];
            assert_eq!(
                declining.read_at(offset, &mut ours).unwrap(),
                data.read_at(offset, &mut theirs).unwrap(),
                "{offset}+{len}"
            );
            assert_eq!(ours, theirs, "{offset}+{len}");
        }
    }

    #[test]
    fn a_slice_fills_what_it_can_and_stops_at_its_end() {
        let data: &[u8] = b"0123456789";
        let mut buf = [0u8; 4];
        assert_eq!(data.read_at(0, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"0123");
        assert_eq!(data.read_at(6, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"6789");

        // Short at the end, zero past it, and never an error.
        assert_eq!(data.read_at(8, &mut buf).unwrap(), 2);
        assert_eq!(data.read_at(10, &mut buf).unwrap(), 0);
        assert_eq!(data.read_at(u64::MAX, &mut buf).unwrap(), 0);
        assert_eq!(data.size().unwrap(), 10);

        let empty: &[u8] = b"";
        assert_eq!(empty.read_at(0, &mut buf).unwrap(), 0);
        assert_eq!(empty.size().unwrap(), 0);
    }

    #[test]
    fn a_file_and_a_slice_answer_identically() {
        let dir = std::env::temp_dir().join(format!("xz-seek-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bytes");
        let data: Vec<u8> = (0..=255u8).cycle().take(9999).collect();
        std::fs::write(&path, &data).unwrap();
        let file = std::fs::File::open(&path).unwrap();

        let slice: &[u8] = &data;
        assert_eq!(
            CompressedSource::size(&file).unwrap(),
            CompressedSource::size(&slice).unwrap()
        );
        for (offset, len) in [
            (0u64, 1usize),
            (1, 4096),
            (9998, 8),
            (9999, 8),
            (5000, 4999),
        ] {
            let mut a = vec![0u8; len];
            let mut b = vec![0u8; len];
            let na = CompressedSource::read_at(&file, offset, &mut a).unwrap();
            let nb = CompressedSource::read_at(&slice, offset, &mut b).unwrap();
            assert_eq!(na, nb, "at {offset}+{len}");
            assert_eq!(a[..na], b[..nb], "at {offset}+{len}");

            // A file holds nothing, so it takes the declining default — and so
            // does an `Arc<File>`, which forwards to it.
            assert_eq!(
                CompressedSource::slice_at(&file, offset, len),
                None,
                "at {offset}+{len}"
            );
        }
        let shared = std::sync::Arc::new(file);
        assert_eq!(shared.slice_at(0, 1), None);
        a_loan_agrees_with_a_read(&shared, loan_axis(9999));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
