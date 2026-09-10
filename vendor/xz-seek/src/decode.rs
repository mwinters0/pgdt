//! Decoding one block: the header, the memory limit, the payload through the
//! seam, and the check.
//!
//! This is where the container knowledge the seam refuses to hold lives. It
//! turns a [`BlockEntry`] into uncompressed bytes, streaming — a block is never
//! materialized, which is what makes the 128 MiB blocks of the real multiblock
//! corpus file cost nothing to read a kilobyte out of.
//!
//! [`crate::Reader`] holds a [`BlockDecode`] as its live decode state: it reads
//! into the caller's buffer where the caller wants bytes and into a discard
//! buffer where it is skipping forward, and it decides between continuing this
//! one and starting a new one from the seek table's arithmetic.
//!
//! # The verification state decides what a block does, not how it decodes
//!
//! [`Verify`] reaches this module in exactly one place: whether a [`Hasher`] is
//! built at all, and what a check id nothing implements means. Everything after
//! that is the same loop. The seek-away escape — completing a partly-decoded
//! block before leaving it — is not here, because a block cannot know it is
//! being abandoned; it is [`crate::Reader`]'s, driving `read` into its discard
//! buffer until the block ends.
//!
//! # The memory limit is checked before a decoder exists
//!
//! LZMA2's dictionary size is one property byte the header parse has already
//! turned into a number, so the limit is a comparison — made in [`BlockDecode::start`]
//! **before** [`crate::backend::PayloadDecoder::new`] is called, so a hostile
//! file never gets an allocation attempted on its behalf. The limit is stated
//! in dictionary bytes because the dictionary is the only part of the footprint
//! the file dictates; see `docs/design/roadmap.md`, "The memory limit is
//! enforced from our own parse, in dictionary bytes".
//!
//! # One read fetches the payload, the padding and the check together
//!
//! A block is `header · compressed data · block padding · check`, in that
//! order — **the padding is between the payload and the check**, not after it,
//! so the check's last byte is at `compressed_offset + total_size - 1` and the
//! padding is `total_size - unpadded_size` null bytes. So the region this
//! module reads after the header is `total_size - header_size`, and the chunk
//! that carries the payload's last bytes carries the at most thirty-five bytes
//! behind them with it. Over the ranged-HTTP source R2 exists to serve that is
//! one request rather than two, and it is free.
//!
//! The padding is checked for null as it goes by, since the bytes are in hand
//! anyway; non-null padding is [`Error::IndexInconsistent`], the same verdict
//! the header parse reaches for its own padding.
//!
//! **A source that holds the tail lends it instead**, whole and in one go
//! ([`CompressedSource::slice_at`]), so a decode out of a `Window` or a memory
//! map copies none of the compressed bytes and the chunk above is the size of a
//! copy that is not being made. Only the stored check, at most 64 bytes, is
//! copied out of a loan, because it has to outlive the bytes it was cut from.
//!
//! The field order and the exclusion of the padding from Unpadded Size are
//! `docs/design/xz-invariants.md`'s `I15`.
//!
//! **The header is a read of its own**, before that one and overlapping it —
//! deficiency: KD3, whose detail is in `docs/design/architecture.md`. So a block
//! is two source reads, which is free on a local file and a second range request
//! per block over a remote one.
//!
//! # What a failure is called
//!
//! Four variants come out of this module, and the lines between them are the
//! ones the taxonomy was grown to draw:
//!
//! * [`Error::MemoryLimitExceeded`] — the block declares a dictionary larger
//!   than the reader allows. Raised before anything is built.
//! * [`Error::UnsupportedCheck`] — the stream declares a reserved check id.
//!   `xz` decodes such a file with a warning (`xz-invariants.md`, `I2`) and so
//!   does this crate under the weaker verification states; under full
//!   verification returning the bytes would hand a caller who asked for
//!   verification a block that was never verified, so the block is refused
//!   before it is started.
//! * [`Error::BlockCheckFailed`] — the block decoded to its declared length and
//!   its check does not match.
//! * [`Error::BlockDataError`] — the payload did not decode to a complete
//!   block. Its offset is inside the payload, exact to the byte the decoder
//!   stopped at under `liblzma` and to the failing call's first byte under
//!   `xz4rust`, whose decoder discards a failed call's progress.
//!
//! And one that arrives from beneath: [`Error::IndexInconsistent`], for a block
//! that decodes cleanly to a size the index does not agree with. That is the
//! format's own obligation on a random-access decoder — a fully decoded block's
//! unpadded and uncompressed sizes must match the index, and a partial decode
//! must exceed neither — and both are counters this loop already keeps.
//!
//! **The line between that and `BlockDataError` is who stopped.** A payload the
//! decoder *refuses* is `BlockDataError`; a payload the decoder accepts and
//! completes at a length the index did not record is the index disagreeing with
//! the blocks, which is `IndexInconsistent`'s own narrow case.
//!
//! **Where the index overstates a block the two backends draw that line
//! differently** — deficiency: KD6, whose detail is in
//! `docs/design/architecture.md`.

use crate::backend::{Backend, PayloadDecoder, SeamError};
use crate::block;
use crate::check::Hasher;
use crate::error::{Error, Result};
use crate::reader::Verify;
use crate::source::CompressedSource;
use crate::table::{BlockEntry, Check};
use crate::walk::inconsistent;

/// The default memory limit, in dictionary bytes.
///
/// 64 MiB is `-9`'s dictionary, so the default admits every preset the `xz` CLI
/// produces rather than the format's ceiling — a refusal means hostile input
/// and never a real file.
pub(crate) const DEFAULT_MEMLIMIT: u64 = 64 << 20;

/// The compressed input chunk, in bytes.
///
/// Not configurable, and the size is not arbitrary: the payload has to land
/// somewhere for the seam to be handed a slice, and a small chunk against the
/// real corpus file's 1.29 MB block payload is twenty `read_at` calls per
/// block — nothing on a local file, and twenty range requests where one would
/// do over a ranged-HTTP source. Revisit only with a measurement.
///
/// **It sizes a copy, so a lending source never reaches it**: a loan is taken
/// over the block's whole remaining tail, since there is nothing to amortise
/// when no bytes move.
pub(crate) const INPUT_CHUNK: usize = 1 << 20;

/// Where the payload bytes a fill put in hand live in the source.
///
/// A loan is an **extent**, not a slice, because a [`BlockDecode`] holds no
/// lifetime: the bytes are re-derived through
/// [`CompressedSource::slice_at`](crate::CompressedSource::slice_at) on each
/// call that needs them, which is a bounds check.
#[derive(Clone, Copy)]
struct Loan {
    at: u64,
    len: usize,
}

/// A live decode of one block, from its header to its check.
///
/// Holds the compressed chunk in hand — copied out of the source, or borrowed
/// from it where the source lends — the backend's decoder, and the check
/// being computed as the bytes are produced. It does not hold the uncompressed
/// bytes: they go straight into whatever buffer [`BlockDecode::read`] was given.
///
/// **It does not hold the source either**, which is why every method that
/// touches bytes takes one. [`crate::Reader`] owns its source and owns the live
/// decode over it, and a struct holding both would be self-referential; passing
/// the source in per call is what keeps the reader an ordinary value. A
/// `BlockDecode` is only ever driven over the source it was started against,
/// which is what makes that safe — and what lets borrowed input be kept as a
/// [`Loan`] and re-derived per call rather than held as a slice.
pub(crate) struct BlockDecode {
    block: BlockEntry,
    check: Check,
    /// Where the payload starts, relative to the block's own offset.
    header_size: u64,
    /// The Compressed Data field's length, from the index.
    payload_size: u64,
    /// The compressed chunk in hand, payload bytes only, where the source did
    /// not lend it. Empty while [`BlockDecode::loan`] is `Some`.
    input: Vec<u8>,
    /// The payload bytes in hand, where the source lent them instead.
    loan: Option<Loan>,
    input_pos: usize,
    /// How much of the payload-plus-check region has been read from the source.
    tail_read: u64,
    /// The stored check, as the reads run past the payload's end.
    stored_check: Vec<u8>,
    chunk: usize,
    decoder: PayloadDecoder,
    hasher: Option<Hasher>,
    /// Payload bytes the backend has taken.
    in_used: u64,
    /// Uncompressed bytes produced.
    out_written: u64,
    finished: bool,
}

impl BlockDecode {
    /// Read `block`'s header and start decoding its payload.
    ///
    /// `check` is the enclosing stream's, which is where a block's check type
    /// is written; `memlimit` is in dictionary bytes.
    pub(crate) fn start<S: CompressedSource>(
        source: &S,
        block: &BlockEntry,
        check: Check,
        verify: Verify,
        memlimit: u64,
        backend: Backend,
    ) -> Result<BlockDecode> {
        Self::start_chunked(source, block, check, verify, memlimit, backend, INPUT_CHUNK)
    }

    /// The same, over a chosen input chunk size.
    ///
    /// Exists so that the multi-chunk path is reachable from a test: no fixture
    /// payload comes near [`INPUT_CHUNK`], and the real corpus file — whose
    /// every block needs two chunks — is deliberately out of `cargo test`. It
    /// is not a knob; [`BlockDecode::start`] is the only caller outside this
    /// module's tests, and it passes the constant.
    pub(crate) fn start_chunked<S: CompressedSource>(
        source: &S,
        block: &BlockEntry,
        check: Check,
        verify: Verify,
        memlimit: u64,
        backend: Backend,
        chunk: usize,
    ) -> Result<BlockDecode> {
        debug_assert!(chunk > 0, "an input chunk of zero would make no progress");
        let at = block.compressed_offset;
        let header = block::read_header(source, block, check)?;

        // Before any backend object is constructed: the file dictates the
        // dictionary and nothing else about the footprint.
        if header.dict_size > memlimit {
            return Err(Error::MemoryLimitExceeded {
                compressed_offset: at,
                required: header.dict_size,
                limit: memlimit,
            });
        }

        // The verification state decides what a check nothing implements means.
        // [`Verify::Full`] cannot be delivered over one, and this crate does not
        // decode a block while pretending otherwise; the two weaker states
        // decode it unverified, which is what `xz` itself does (`I2`).
        let hasher = match verify {
            Verify::Off => None,
            Verify::Streaming => Hasher::new(check),
            Verify::Full => match Hasher::new(check) {
                Some(h) => Some(h),
                None => {
                    return Err(Error::UnsupportedCheck {
                        compressed_offset: at,
                        check_id: check.id(),
                    });
                }
            },
        };

        let decoder = PayloadDecoder::new(
            backend,
            &header.filters,
            header.payload_size,
            block.uncompressed_size,
        )
        .map_err(|e| chain_error(e, at))?;

        Ok(BlockDecode {
            block: *block,
            check,
            header_size: header.header_size,
            payload_size: header.payload_size,
            input: Vec::new(),
            loan: None,
            input_pos: 0,
            tail_read: 0,
            stored_check: Vec::new(),
            chunk,
            decoder,
            hasher,
            in_used: 0,
            out_written: 0,
            finished: false,
        })
    }

    /// Produce up to `out.len()` more of the block's uncompressed bytes.
    ///
    /// **Zero means the block is exhausted**, and by then the check has been
    /// computed and compared.
    ///
    /// The check is compared on the call that produces the block's **last
    /// byte**, not on a later one: once the index's count of uncompressed bytes
    /// has been delivered, this loops once more with no output room so that the
    /// decoder can take the payload's own terminating marker, and completes the
    /// block there. Without that, a caller reading a file straight through would
    /// leave the last block of every file unverified — nothing ever seeks away
    /// from it — which is R6 defeated by the commonest access pattern there is.
    ///
    /// `out` must not be empty: a zero-length buffer cannot be told from the end
    /// of the block, and offering the decoder no room forever is the one way
    /// this loop could fail to terminate.
    pub(crate) fn read<S: CompressedSource>(
        &mut self,
        source: &S,
        out: &mut [u8],
    ) -> Result<usize> {
        debug_assert!(!out.is_empty(), "read needs somewhere to put the bytes");
        if self.finished {
            return Ok(0);
        }
        let mut produced = 0usize;
        loop {
            self.fill(source)?;
            // The loan, re-derived, or nothing where the input was copied.
            let lent = self.take_loan(source)?;
            // Never write past what the index says this block holds, so that a
            // partial decode cannot exceed the index either.
            let room = ((self.block.uncompressed_size - self.out_written) as usize)
                .min(out.len() - produced);
            // The two clamps, stated where they are spent rather than where
            // they are computed. Together they are why the `xz4rust` arm calls
            // "more data in the block body than the header indicated" this
            // crate's bug and not the file's — see `backend.rs`'s `fault`: the
            // seam is never offered a byte past the payload, nor room past what
            // the index says the block holds, so the backend cannot be carried
            // past either declaration by anything a file contains. The sweep's
            // 532,228 pairs run through here, which is what makes this the pin.
            debug_assert!(
                self.in_used + (self.input_len() - self.input_pos) as u64 <= self.payload_size,
                "the seam was offered a byte past the payload"
            );
            debug_assert!(
                self.out_written + room as u64 <= self.block.uncompressed_size,
                "the seam was offered room past the block"
            );
            // One or the other, and nothing below this line knows which: a
            // borrowed input is the source's own bytes, a copied one this
            // struct's. `lent` borrows the source, not `self`, so the decoder
            // is still reachable mutably beside it.
            let input = lent.unwrap_or(&self.input[..]);
            let progress = match self.decoder.decode(
                &input[self.input_pos..],
                &mut out[produced..produced + room],
            ) {
                Ok(p) => p,
                Err(_) => return Err(self.data_error()),
            };
            self.input_pos += progress.in_used;
            self.in_used += progress.in_used as u64;
            let n = progress.out_written;
            self.out_written += n as u64;
            if let Some(h) = self.hasher.as_mut() {
                h.update(&out[produced..produced + n]);
            }
            produced += n;

            if progress.finished {
                self.complete(source)?;
                return Ok(produced);
            }
            if n == 0 && progress.in_used == 0 {
                // Nothing moved. The whole payload's length was known before
                // this started, so there is no more input to be had.
                return Err(if self.in_used == self.payload_size {
                    // `xz-invariants.md`, `I8`: this is the conclusion, and it
                    // is reached from progress rather than from a status that
                    // may be `Ok(MemNeeded)` — a success value.
                    self.data_error()
                } else {
                    // Input remains and the decoder wants output room the
                    // index does not allow: the block holds more than the
                    // index recorded.
                    Error::IndexInconsistent {
                        compressed_offset: self.block.compressed_offset,
                    }
                });
            }
            if produced > 0 && self.out_written < self.block.uncompressed_size {
                // Bytes in hand and the block still owes more: hand them over
                // rather than looping, so a read costs one decoder call in the
                // common case. Filling a buffer that spans blocks is the
                // reader's loop, not this one's.
                return Ok(produced);
            }
            // Either nothing was produced and there is more input to try, or
            // the block's last byte is out and the decoder still has its
            // terminating marker to take.
        }
    }

    /// The uncompressed offset of the next byte this will produce.
    pub(crate) fn position(&self) -> u64 {
        self.block.uncompressed_offset + self.out_written
    }

    /// Whether the block has been decoded to its end and its check compared.
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Fetch the next chunk of the block's tail — payload, then block padding,
    /// then the stored check — when the current one is spent.
    ///
    /// Only the payload survives in [`BlockDecode::input`]; the padding is
    /// checked and dropped and the check is stashed, so the seam is never
    /// offered a byte that is not the block's compressed data.
    fn fill<S: CompressedSource>(&mut self, source: &S) -> Result<()> {
        if self.input_pos < self.input_len() {
            return Ok(());
        }
        let tail = self.block.total_size() - self.header_size;
        let start = self.tail_read;
        let remaining = tail - start;
        if remaining == 0 {
            self.input.clear();
            self.loan = None;
            self.input_pos = 0;
            return Ok(());
        }
        let at = self.block.compressed_offset + self.header_size + start;

        // A source that holds the whole remaining tail lends it in one go, and
        // one that does not is read `chunk` bytes at a time as before. **The
        // loan is not chunked**, because there is nothing to amortise when no
        // bytes move: `chunk` is the size of a copy, and a knob on a loan would
        // be a term with nothing behind it.
        let lent = match usize::try_from(remaining)
            .ok()
            .and_then(|len| source.slice_at(at, len))
        {
            // The half of `slice_at`'s contract a consumer can hold an
            // implementor to for free; the dev profile keeps debug assertions
            // on, so it fires over every fixture decode. With them off a
            // mis-sized loan is refused rather than trusted, which costs a copy
            // and cannot mis-address a decode.
            Some(s) => {
                debug_assert_eq!(
                    s.len() as u64,
                    remaining,
                    "a loan of {remaining} bytes at {at} came back {} long",
                    s.len()
                );
                (s.len() as u64 == remaining).then_some(s)
            }
            None => None,
        };
        let want = match lent {
            Some(s) => s.len(),
            None => remaining.min(self.chunk as u64) as usize,
        };
        let end = start + want as u64;

        // The tail bytes this fill has in hand, whichever way they arrived.
        let bytes: &[u8] = match lent {
            Some(s) => s,
            None => {
                self.input.resize(want, 0);
                read_whole(source, at, &mut self.input)?;
                &self.input
            }
        };

        let check_at = tail - self.check.size();

        // Block padding must be null, and the bytes are already in hand.
        let pad_lo = self.payload_size.max(start);
        let pad_hi = check_at.min(end);
        if pad_lo < pad_hi {
            let (lo, hi) = ((pad_lo - start) as usize, (pad_hi - start) as usize);
            if let Some(i) = bytes[lo..hi].iter().position(|b| *b != 0) {
                return inconsistent(at + (lo + i) as u64);
            }
        }

        // The stored check is the one thing a loan still copies: 64 bytes at
        // most, and it has to outlive the bytes it was cut from.
        if check_at < end {
            let lo = (check_at.max(start) - start) as usize;
            self.stored_check.extend_from_slice(&bytes[lo..]);
        }

        let payload_here = self.payload_size.saturating_sub(start).min(want as u64) as usize;
        match lent {
            Some(_) => {
                self.input.clear();
                self.loan = Some(Loan {
                    at,
                    len: payload_here,
                });
            }
            None => {
                self.input.truncate(payload_here);
                self.loan = None;
            }
        }
        self.input_pos = 0;
        self.tail_read += want as u64;
        Ok(())
    }

    /// The payload bytes in hand, or `None` where they were copied into
    /// [`BlockDecode::input`].
    ///
    /// A loan is kept as an extent and re-derived here, because this struct
    /// holds no lifetime. **A source that lent a range and then declines it is
    /// not an error case**, per
    /// [`slice_at`](crate::CompressedSource::slice_at)'s contract: the range is
    /// read instead and the decode carries on copying, so a loan can still only
    /// ever skip a copy.
    fn take_loan<'a, S: CompressedSource>(&mut self, source: &'a S) -> Result<Option<&'a [u8]>> {
        let Some(loan) = self.loan else {
            return Ok(None);
        };
        if let Some(s) = source.slice_at(loan.at, loan.len) {
            debug_assert_eq!(
                s.len(),
                loan.len,
                "a loan of {} bytes at {} came back {} long",
                loan.len,
                loan.at,
                s.len()
            );
            if s.len() == loan.len {
                return Ok(Some(s));
            }
        }
        self.input.resize(loan.len, 0);
        read_whole(source, loan.at, &mut self.input)?;
        self.loan = None;
        Ok(None)
    }

    /// How many payload bytes this fill put in hand.
    fn input_len(&self) -> usize {
        match self.loan {
            Some(loan) => loan.len,
            None => self.input.len(),
        }
    }

    /// Read whatever of the padding and the check the payload's own reads did
    /// not already carry.
    ///
    /// In the common case this reads nothing: the chunk that fetched the
    /// payload's last bytes fetched the thirty-five bytes that can follow them
    /// too. It has work only where the payload ended on a chunk boundary.
    fn finish_tail<S: CompressedSource>(&mut self, source: &S) -> Result<()> {
        debug_assert_eq!(self.input_pos, self.input_len(), "the payload is spent");
        let tail = self.block.total_size() - self.header_size;
        while self.tail_read < tail {
            self.fill(source)?;
        }
        Ok(())
    }

    /// The payload ended. Reconcile it with the index, then compare the check.
    fn complete<S: CompressedSource>(&mut self, source: &S) -> Result<()> {
        self.finished = true;
        let at = self.block.compressed_offset;
        if self.in_used != self.payload_size || self.out_written != self.block.uncompressed_size {
            return inconsistent(at);
        }
        let Some(hasher) = self.hasher.take() else {
            return Ok(());
        };
        self.finish_tail(source)?;
        if hasher.finish().as_slice() != self.stored_check {
            return Err(Error::BlockCheckFailed {
                compressed_offset: at,
                uncompressed_range: self.block.uncompressed_range(),
            });
        }
        Ok(())
    }

    /// The payload is not a complete block, at the byte the decoder stopped on.
    fn data_error(&self) -> Error {
        Error::BlockDataError {
            compressed_offset: self.block.compressed_offset
                + self.header_size
                + self.decoder.input_consumed(),
            uncompressed_range: self.block.uncompressed_range(),
        }
    }
}

/// Fill `buf` from `at`, or say the source ended inside a block.
///
/// The one place a copied read of a block's tail happens, so that the fetch a
/// loan replaces and the fallback a declined loan takes give a short read the
/// same name. Every block the walk placed lies inside the file it measured, so
/// a short read is the source having changed under us.
fn read_whole<S: CompressedSource>(source: &S, at: u64, buf: &mut [u8]) -> Result<()> {
    let got = source.read_at(at, buf).map_err(|e| Error::io(at, e))?;
    if got < buf.len() {
        return Err(Error::Truncated {
            compressed_offset: at + got as u64,
        });
    }
    Ok(())
}

/// Give a chain the seam refused a place in the taxonomy and an offset.
///
/// `at` is the block header's own offset, because that is where the chain was
/// read from and the only coordinate a chain failure has.
fn chain_error(e: SeamError, at: u64) -> Error {
    match e {
        SeamError::UnsupportedFilter(filter_id) => Error::UnsupportedFilter {
            compressed_offset: at,
            filter_id,
        },
        // A chain the backend will not assemble is a header that is well-formed
        // and describes something the format does not permit — the same verdict
        // the parse reaches for the rest of that class.
        SeamError::UnusableChain | SeamError::Data => Error::IndexInconsistent {
            compressed_offset: at,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::COMPILED;
    use crate::source::Declining;
    use crate::table::SeekTable;
    use crate::window::Window;
    use std::fs::File;
    use std::path::Path;

    /// The "deliberately small" input chunk the corpus is decoded at a second
    /// time.
    ///
    /// Sixty-one is under `xz4rust`'s own 63-byte temp buffer and coprime with
    /// every LZMA2 framing length, so a source read boundary lands inside the
    /// decoder's buffering rather than beside it. That is the hazard the
    /// standing sweep cannot reach — no fixture payload comes near
    /// [`INPUT_CHUNK`], so the multi-chunk path is only ever entered from here.
    const SMALL_CHUNK: usize = 61;

    /// Decode a whole block into a `Vec`: start to finish, one block at a time.
    fn whole_block<S: CompressedSource>(
        source: &S,
        block: &BlockEntry,
        check: Check,
        memlimit: u64,
        backend: Backend,
        chunk: usize,
    ) -> Result<Vec<u8>> {
        whole_block_into(source, block, check, memlimit, backend, chunk, 8192)
    }

    /// The same, with the caller's output buffer sized too.
    ///
    /// `out_len` is what a narrow `read_at` looks like from here: the block is
    /// asked for its bytes a handful at a time, which is the only way to reach
    /// a decoder that has more to give than the call can carry.
    fn whole_block_into<S: CompressedSource>(
        source: &S,
        block: &BlockEntry,
        check: Check,
        memlimit: u64,
        backend: Backend,
        chunk: usize,
        out_len: usize,
    ) -> Result<Vec<u8>> {
        let mut d = BlockDecode::start_chunked(
            source,
            block,
            check,
            Verify::Full,
            memlimit,
            backend,
            chunk,
        )?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; out_len];
        loop {
            let n = d.read(source, &mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            assert_eq!(d.position(), block.uncompressed_offset + out.len() as u64);
        }
        assert!(d.is_finished());
        Ok(out)
    }

    /// The error a call was expected to fail with. A plain `unwrap_err` would
    /// need `BlockDecode` to be `Debug`, which it cannot be: it owns the
    /// backend's decoder.
    fn failure<T>(r: Result<T>) -> Error {
        match r {
            Ok(_) => panic!("expected a failure"),
            Err(e) => e,
        }
    }

    /// The stream a block belongs to, since a block's check type is written in
    /// its stream's flags and nowhere else.
    fn check_of(table: &SeekTable, i: usize) -> Check {
        table
            .streams
            .iter()
            .find(|s| i >= s.first_block && i < s.first_block + s.block_count)
            .expect("every block belongs to a stream")
            .check
    }

    fn walked(path: &Path) -> (File, SeekTable) {
        let source = File::open(path).expect("the fixture opens");
        let table = SeekTable::from_source(&source).expect("the fixture walks");
        (source, table)
    }

    /// **The differential evidence for this slice.** Every block of every intact
    /// fixture, decoded alone, against `xz -dc`'s bytes at that block's range.
    ///
    /// "Alone" is the property that matters: each block is started from its own
    /// header with no decoder state carried in from the block before it, which
    /// is exactly what a positioned read does and what a whole-file decode
    /// never tests.
    ///
    /// It lives here rather than in `tests/` because the decode is internal:
    /// `Reader` is the public way to these bytes and it never decodes a block
    /// alone, which is the property under test. `harness::oracle::plaintext_for`
    /// returns plain bytes, which is what lets a `src/` unit test call across
    /// the dev-dependency cycle — see `harness`'s crate docs.
    ///
    /// `reserved-check.xz` is intact and excluded, and it is the only
    /// exclusion: its stream declares a check id nothing implements, so full
    /// verification refuses it, and the test below is what asserts that
    /// instead. `bulk-blocks.xz` is in, at +1.50 s, and it is the one fixture
    /// whose blocks are numerous enough for the chunk axis below to be run 512
    /// times over one recipe — `harness.md`, "`bulk-blocks.xz` is out of the
    /// byte sweep and in everything else".
    #[test]
    fn every_block_in_the_corpus_decodes_to_the_bytes_xz_produces() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let mut blocks = 0usize;
        let mut decodes = 0usize;
        let mut files = 0;

        for f in fixtures_gen::FIXTURES
            .iter()
            .filter(|f| f.intact && f.name != "reserved-check.xz")
        {
            let path = dir.join(f.name);
            let (source, table) = walked(&path);
            let plaintext =
                harness::oracle::plaintext_for(&path).unwrap_or_else(|e| panic!("{}: {e}", f.name));
            assert_eq!(
                plaintext.len() as u64,
                table.uncompressed_size(),
                "{}",
                f.name
            );
            files += 1;

            for (i, block) in table.blocks.iter().enumerate() {
                let r = block.uncompressed_range();
                for &backend in COMPILED {
                    for chunk in [INPUT_CHUNK, SMALL_CHUNK] {
                        let got = whole_block(
                            &source,
                            block,
                            check_of(&table, i),
                            DEFAULT_MEMLIMIT,
                            backend,
                            chunk,
                        )
                        .unwrap_or_else(|e| {
                            panic!("{} block {i} on {backend:?} at chunk {chunk}: {e}", f.name)
                        });
                        assert_eq!(
                            got.len() as u64,
                            block.uncompressed_size,
                            "{} block {i} on {backend:?} at chunk {chunk}",
                            f.name
                        );
                        assert!(
                            got == plaintext[r.start as usize..r.end as usize],
                            "{} block {i} on {backend:?} at chunk {chunk}: \
                             bytes differ from xz -dc",
                            f.name
                        );
                        decodes += 1;
                    }
                }
                blocks += 1;
            }
        }

        assert_eq!(files, 19, "every intact fixture but the reserved-check one");
        assert!(blocks >= 86, "{blocks} blocks decoded");
        assert_eq!(
            decodes,
            blocks * COMPILED.len() * 2,
            "every block, under every compiled backend, at both chunk sizes"
        );
    }

    /// The same block, decoded out of a [`Window`] holding exactly its own
    /// compressed range.
    ///
    /// This is the parallel path's `(block, window)` handoff rehearsed with
    /// what exists: a worker is handed the block and the bytes someone else
    /// fetched for it, and pulls out of them exactly as it pulls out of a file.
    /// **Two things have to be true and neither is visible from one side
    /// alone** — the decode must read no byte outside the block's own
    /// `total_size()` bytes at `compressed_offset`, and the window must refuse
    /// it if it does. Over a file an overrun is served silently from the page
    /// cache; here it is an error, so this is what says the fetch size the
    /// parallel path will use is the right one.
    ///
    /// Four fixtures rather than the corpus: the property is about the decode's
    /// *addressing*, which does not vary with the filter chain, and the
    /// corpus-wide arm above already decodes every block. `filter-x86.xz` is in
    /// because a BCJ chain is the one that holds bytes back at the block's end,
    /// and `mixed-checks.xz` because its blocks span streams whose checks
    /// differ, so a window over the wrong block would not merely mis-address.
    ///
    /// **It is also the arm that runs both branches of `fill` over the same
    /// bytes.** A window lends, so the borrowing pass takes the block's whole
    /// tail in one loan; the two chunked passes go through [`Declining`], which
    /// is the same window with the loan refused. So *a loan cannot change an
    /// outcome* is a gated claim rather than a design one — and without the
    /// wrapper the chunk axis would have gone quiet here, since a whole-tail
    /// loan never re-enters `fill` and `SMALL_CHUNK` would cost the time
    /// without reaching the multi-chunk path.
    #[test]
    fn a_block_decodes_out_of_a_window_over_its_own_compressed_range() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let mut decodes = 0usize;
        let mut blocks = 0usize;

        for name in [
            "many-blocks.xz",
            "filter-x86.xz",
            "check-sha256.xz",
            "mixed-checks.xz",
        ] {
            let path = dir.join(name);
            let (source, table) = walked(&path);
            let file_size = CompressedSource::size(&source).expect("the fixture measures");
            let bytes = std::fs::read(&path).expect("the fixture reads");
            let plaintext = harness::oracle::plaintext_for(&path).expect("xz -dc");

            for (i, block) in table.blocks.iter().enumerate() {
                let lo = block.compressed_offset as usize;
                let hi = lo + block.total_size() as usize;
                let window =
                    Window::new(block.compressed_offset, file_size, bytes[lo..hi].to_vec());
                let r = block.uncompressed_range();

                for &backend in COMPILED {
                    // The chunked passes decline, the whole-tail pass borrows.
                    // `chunk` is what the borrowing pass ignores, so it is named
                    // in the failure message either way.
                    for (chunk, lends) in [
                        (INPUT_CHUNK, false),
                        (SMALL_CHUNK, false),
                        (INPUT_CHUNK, true),
                    ] {
                        let got = if lends {
                            whole_block(
                                &window,
                                block,
                                check_of(&table, i),
                                DEFAULT_MEMLIMIT,
                                backend,
                                chunk,
                            )
                        } else {
                            whole_block(
                                &Declining(&window),
                                block,
                                check_of(&table, i),
                                DEFAULT_MEMLIMIT,
                                backend,
                                chunk,
                            )
                        }
                        .unwrap_or_else(|e| {
                            panic!(
                                "{name} block {i} on {backend:?} at chunk {chunk}, \
                                 lending {lends}: {e}"
                            )
                        });
                        assert!(
                            got == plaintext[r.start as usize..r.end as usize],
                            "{name} block {i} on {backend:?} at chunk {chunk}, \
                             lending {lends}: bytes differ from xz -dc"
                        );
                        decodes += 1;
                    }
                }
                blocks += 1;
            }
        }

        assert!(blocks >= 8, "{blocks} blocks decoded out of a window");
        assert_eq!(
            decodes,
            blocks * COMPILED.len() * 3,
            "every block, under every compiled backend, declining at both chunk \
             sizes and borrowing once"
        );
    }

    /// A source that lends a range and then declines it decodes the same bytes.
    ///
    /// `fill` takes a loan and keeps it as an extent, so every later call
    /// re-derives it — and a source is free to decline the second time, since
    /// declining is always a correct answer. What must not happen is that the
    /// decode fails: the contract says a loan can skip a copy and cannot
    /// introduce an error case, and `take_loan`'s fallback is where that is
    /// made true. Nothing in the crate lends inconsistently, so this is the
    /// only thing that runs the fallback at all.
    #[test]
    fn a_source_that_stops_lending_mid_block_decodes_identically() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        for name in ["many-blocks.xz", "filter-x86.xz", "check-sha256.xz"] {
            let path = dir.join(name);
            let (source, table) = walked(&path);
            let file_size = CompressedSource::size(&source).expect("the fixture measures");
            let bytes = std::fs::read(&path).expect("the fixture reads");
            let plaintext = harness::oracle::plaintext_for(&path).expect("xz -dc");

            for (i, block) in table.blocks.iter().enumerate() {
                let lo = block.compressed_offset as usize;
                let hi = lo + block.total_size() as usize;
                let window =
                    Window::new(block.compressed_offset, file_size, bytes[lo..hi].to_vec());
                let fickle = LendsOnce {
                    inner: window,
                    left: std::cell::Cell::new(1),
                };
                let r = block.uncompressed_range();

                for &backend in COMPILED {
                    fickle.left.set(1);
                    let got = whole_block(
                        &fickle,
                        block,
                        check_of(&table, i),
                        DEFAULT_MEMLIMIT,
                        backend,
                        INPUT_CHUNK,
                    )
                    .unwrap_or_else(|e| panic!("{name} block {i} on {backend:?}: {e}"));
                    assert_eq!(
                        fickle.left.get(),
                        0,
                        "{name} block {i} on {backend:?}: the loan was never taken"
                    );
                    assert!(
                        got == plaintext[r.start as usize..r.end as usize],
                        "{name} block {i} on {backend:?}: bytes differ from xz -dc"
                    );
                }
            }
        }
    }

    /// A source that lends its first `left` ranges and declines every one after.
    ///
    /// It is what a decode re-deriving an extent has to survive, and nothing
    /// real behaves this way — a `Window` and a `&[u8]` are both pure functions
    /// of what they hold.
    struct LendsOnce<S> {
        inner: S,
        left: std::cell::Cell<usize>,
    }

    impl<S: CompressedSource> CompressedSource for LendsOnce<S> {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read_at(offset, buf)
        }

        fn size(&self) -> std::io::Result<u64> {
            self.inner.size()
        }

        fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
            let left = self.left.get();
            if left == 0 {
                return None;
            }
            let lent = self.inner.slice_at(offset, len)?;
            self.left.set(left - 1);
            Some(lent)
        }
    }

    /// The same bytes come out when the payload arrives in many small reads.
    ///
    /// No fixture payload comes near the 1 MiB chunk and the corpus cannot
    /// carry one that does, so the multi-chunk path is reached by shrinking the
    /// chunk instead of by growing the file — the same move the footer walk's
    /// window fallback is reached by.
    #[test]
    fn a_payload_read_in_many_chunks_decodes_identically() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        for name in ["many-blocks.xz", "check-sha256.xz", "filter-delta.xz"] {
            let path = dir.join(name);
            let (source, table) = walked(&path);
            let plaintext = harness::oracle::plaintext_for(&path).expect("xz -dc");
            let block = table.blocks[1];
            let check = check_of(&table, 1);
            let r = block.uncompressed_range();

            for &backend in COMPILED {
                for chunk in [1usize, 3, 64, 4096] {
                    let got = whole_block(&source, &block, check, DEFAULT_MEMLIMIT, backend, chunk)
                        .unwrap_or_else(|e| panic!("{name} on {backend:?} at {chunk}: {e}"));
                    assert!(
                        got == plaintext[r.start as usize..r.end as usize],
                        "{name} on {backend:?} at chunk {chunk}"
                    );
                }
            }
        }
    }

    /// A block whose chain holds bytes back still decodes when the caller's
    /// buffer is narrower than the held tail.
    ///
    /// `filter-x86.xz` is the corpus's only BCJ chain and so the only fixture
    /// that reaches this at all: under `xz4rust` a BCJ filter withholds up to
    /// 16 bytes of the block's output and releases them only to a later call
    /// carrying room (`xz-invariants.md`, `I18`), and the trailer push is the
    /// only thing that can carry the release along (`I16`). So the block's last
    /// bytes come out of a push that has to survive running out of the caller's
    /// buffer and being re-entered — which is what the sizes below are for. At
    /// 8 KiB, the buffer `whole_block` uses everywhere else, one push drains the
    /// whole tail and the resume point is never exercised.
    ///
    /// Under `liblzma` the tail is released inside the call that ends the block,
    /// so this is an ordinary narrow read; running it under both backends is
    /// what makes the sizes a property of the block rather than of one decoder.
    #[test]
    fn a_chain_that_holds_bytes_back_drains_through_a_buffer_narrower_than_the_tail() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let path = dir.join("filter-x86.xz");
        let (source, table) = walked(&path);
        let plaintext = harness::oracle::plaintext_for(&path).expect("xz -dc");

        for (i, block) in table.blocks.iter().enumerate() {
            let r = block.uncompressed_range();
            let check = check_of(&table, i);
            for &backend in COMPILED {
                // Straddling the 16-byte bound on both sides: below it every
                // read is short of the tail, above it the first push drains
                // what is left in one go.
                for out_len in [1usize, 2, 7, 15, 16, 17, 64] {
                    let got = whole_block_into(
                        &source,
                        block,
                        check,
                        DEFAULT_MEMLIMIT,
                        backend,
                        INPUT_CHUNK,
                        out_len,
                    )
                    .unwrap_or_else(|e| {
                        panic!("block {i} on {backend:?} into {out_len} bytes: {e}")
                    });
                    assert!(
                        got == plaintext[r.start as usize..r.end as usize],
                        "block {i} on {backend:?} into {out_len} bytes: \
                         bytes differ from xz -dc"
                    );
                }
            }
        }
    }

    /// The limit is a comparison against the dictionary the header declares, and
    /// it is made before a decoder exists.
    #[test]
    fn a_dictionary_over_the_limit_is_refused_before_anything_is_built() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let path = dir.join("large-dict.xz");
        let (source, table) = walked(&path);
        let block = table.blocks[0];
        let check = table.streams[0].check;

        // Under every compiled backend: the limit does not move, so the verdict
        // is the header's and not the decoder's.
        for &backend in COMPILED {
            // `-9` declares 64 MiB whatever the input size (`I3`), which is
            // exactly the default: the default admits every preset the CLI
            // produces.
            assert!(
                BlockDecode::start(
                    &source,
                    &block,
                    check,
                    Verify::Full,
                    DEFAULT_MEMLIMIT,
                    backend
                )
                .is_ok(),
                "{backend:?}"
            );

            let e = failure(BlockDecode::start(
                &source,
                &block,
                check,
                Verify::Full,
                (64 << 20) - 1,
                backend,
            ));
            assert!(
                matches!(
                    e,
                    Error::MemoryLimitExceeded {
                        compressed_offset,
                        required: 67_108_864,
                        limit,
                    } if compressed_offset == block.compressed_offset && limit == (64 << 20) - 1
                ),
                "{backend:?}: {e}"
            );
        }
    }

    /// A stream whose check nothing implements is refused under full
    /// verification, before a byte of it is decoded, and decoded unverified
    /// under the two weaker states — which is `xz`'s own warning-and-continue
    /// (`I2`).
    #[test]
    fn a_reserved_check_id_is_refused_only_under_full_verification() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let path = dir.join("reserved-check.xz");
        let (source, table) = walked(&path);
        let block = table.blocks[0];
        let check = table.streams[0].check;
        assert_eq!(check, Check::Unsupported(0x05));

        let plaintext = harness::oracle::plaintext_for(&path).expect("xz -dc");
        for &backend in COMPILED {
            let e = failure(BlockDecode::start(
                &source,
                &block,
                check,
                Verify::Full,
                DEFAULT_MEMLIMIT,
                backend,
            ));
            assert!(
                matches!(
                    e,
                    Error::UnsupportedCheck {
                        compressed_offset,
                        check_id: 0x05
                    } if compressed_offset == block.compressed_offset
                ),
                "{backend:?}: {e}"
            );

            // The bytes are `xz -dc`'s, which is what "decodes it unverified"
            // has to mean if the weaker states are to match the oracle.
            for verify in [Verify::Streaming, Verify::Off] {
                let mut d =
                    BlockDecode::start(&source, &block, check, verify, DEFAULT_MEMLIMIT, backend)
                        .unwrap_or_else(|e| panic!("{backend:?} {verify:?}: {e}"));
                let mut got = Vec::new();
                let mut buf = vec![0u8; 8192];
                loop {
                    let n = d
                        .read(&source, &mut buf)
                        .unwrap_or_else(|e| panic!("{verify:?}: {e}"));
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                let r = block.uncompressed_range();
                assert!(
                    got == plaintext[r.start as usize..r.end as usize],
                    "{backend:?} {verify:?}"
                );
            }
        }
    }

    /// Each damaged payload is refused, and the flip point decides which way.
    ///
    /// The pair shares a base and differs only in which byte is flipped.
    /// `corrupt-payload-check.xz` damages a **stored** LZMA2 chunk, which is
    /// copied verbatim: the block still decodes to its declared length and
    /// consumes its payload exactly, so the integrity check is the only thing
    /// that can notice. `corrupt-payload-derail.xz` damages a **range-coded**
    /// one, where the format requires the coder's code word to reach zero, so
    /// the payload never becomes a complete block.
    ///
    /// `xz` reports the two identically (`xz-invariants.md`, `I9`), so this is
    /// the only place the distinction is visible at all.
    #[test]
    fn each_corrupt_payload_fixture_is_refused_the_way_its_flip_point_requires() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");

        for (name, damaged) in [
            ("corrupt-payload-check.xz", 0),
            ("corrupt-payload-derail.xz", 1),
        ] {
            let (source, table) = walked(&dir.join(name));
            assert_eq!(table.blocks.len(), 2, "{name}: the base is two blocks");
            let block = table.blocks[damaged];
            let check = table.streams[0].check;
            // Under every compiled backend: the split these two exercise is the
            // *format's* and not one decoder's (`I13`), so any conforming
            // backend must reach both verdicts, which is what makes this an
            // acceptance criterion for a new arm rather than a `liblzma`
            // fingerprint.
            for &backend in COMPILED {
                let e = failure(whole_block(
                    &source,
                    &block,
                    check,
                    DEFAULT_MEMLIMIT,
                    backend,
                    INPUT_CHUNK,
                ));
                if damaged == 0 {
                    assert!(
                        matches!(
                            &e,
                            Error::BlockCheckFailed { compressed_offset, uncompressed_range }
                                if *compressed_offset == block.compressed_offset
                                    && *uncompressed_range == block.uncompressed_range()
                        ),
                        "{name} on {backend:?}: {e}"
                    );
                } else {
                    // Inside the payload rather than at the block's start.
                    // `liblzma` puts it at the byte the decoder stopped on;
                    // `xz4rust` discards a failed call's progress, so it can
                    // only be exact to that call's first byte.
                    let payload =
                        block.compressed_offset..block.compressed_offset + block.unpadded_size;
                    assert!(
                        matches!(
                            &e,
                            Error::BlockDataError { compressed_offset, uncompressed_range }
                                if payload.contains(compressed_offset)
                                    && *uncompressed_range == block.uncompressed_range()
                        ),
                        "{name} on {backend:?}: {e}"
                    );
                }

                // The other block still decodes: the fault is one payload's,
                // not the file's shape.
                assert!(
                    whole_block(
                        &source,
                        &table.blocks[1 - damaged],
                        check,
                        DEFAULT_MEMLIMIT,
                        backend,
                        INPUT_CHUNK
                    )
                    .is_ok(),
                    "{name} on {backend:?}"
                );
            }
        }
    }

    /// The check comparison itself, over all three algorithms: a block whose
    /// payload decodes perfectly and whose stored check has one bit flipped.
    ///
    /// Synthetic, in memory, and the sharpest form of the test: nothing but the
    /// check field differs, so a failure here can only be the comparison.
    /// `corrupt-payload-check.xz` is the corpus's producer of the same variant
    /// and damages the *payload* instead, which is the case a caller meets.
    #[test]
    fn a_flipped_bit_in_the_stored_check_is_a_check_failure() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        for name in ["check-crc32.xz", "check-crc64.xz", "check-sha256.xz"] {
            let good = std::fs::read(dir.join(name)).expect("the fixture reads");
            let whole: &[u8] = &good;
            let table = SeekTable::from_source(&whole).expect("the fixture walks");
            let block = table.blocks[1];
            let check = table.streams[0].check;
            assert!(check.size() > 0);

            let mut bad = good.clone();
            // The check is the last bytes of the block, after the padding.
            let at = (block.compressed_offset + block.total_size() - check.size()) as usize;
            bad[at] ^= 0x01;
            let source: &[u8] = &bad;

            for &backend in COMPILED {
                let e = failure(whole_block(
                    &source,
                    &block,
                    check,
                    DEFAULT_MEMLIMIT,
                    backend,
                    INPUT_CHUNK,
                ));
                assert!(
                    matches!(
                        &e,
                        Error::BlockCheckFailed { compressed_offset, uncompressed_range }
                            if *compressed_offset == block.compressed_offset
                                && *uncompressed_range == block.uncompressed_range()
                    ),
                    "{name} on {backend:?}: {e}"
                );

                // The same block over the untouched bytes passes, so the flip
                // is the only thing under test.
                assert!(
                    whole_block(
                        &whole,
                        &block,
                        check,
                        DEFAULT_MEMLIMIT,
                        backend,
                        INPUT_CHUNK
                    )
                    .is_ok(),
                    "{name} on {backend:?}"
                );
            }
        }
    }

    /// A payload that runs out with the block unfinished is a data error,
    /// concluded from progress rather than from a return code.
    ///
    /// This is `xz-invariants.md`, `I8`'s other entrance: the decoder starves
    /// rather than erroring, so it answers `Ok(Status::MemNeeded)` — a success
    /// value — and a loop matching on the error would spin or short-read. The
    /// index entry is shortened rather than the file cut, because a short
    /// *source* is [`Error::Truncated`] and this is a payload that is all
    /// there and does not finish the block.
    ///
    /// **It is also the pin on the `xz4rust` arm's positional discriminator.**
    /// The starved payload leaves that decoder still counting the trailer this
    /// crate pushes as block body, so it answers
    /// `MoreDataInBlockBodyThanHeaderIndicated` — the one variant `fault` calls
    /// ours. `push_trailer` short of `complete` must therefore not consult the
    /// variant at all, and this is what fails, loudly, if it starts to.
    #[test]
    fn a_payload_that_runs_out_before_the_block_does_is_a_data_error() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let (source, table) = walked(&dir.join("many-blocks.xz"));
        let check = table.streams[0].check;

        let mut starved = table.blocks[1];
        // Keep the block four-byte aligned so the bytes the padding check looks
        // at are still padding, and shorten only the payload.
        starved.unpadded_size = (starved.unpadded_size - 64) & !3;
        let payload_end = starved.compressed_offset + starved.unpadded_size - check.size();

        for &backend in COMPILED {
            let e = failure(whole_block(
                &source,
                &starved,
                check,
                DEFAULT_MEMLIMIT,
                backend,
                INPUT_CHUNK,
            ));
            assert!(
                matches!(
                    &e,
                    Error::BlockDataError { compressed_offset, .. }
                        if *compressed_offset == payload_end
                ),
                "{backend:?}: {e}"
            );
        }
    }

    /// A source that ends inside a payload the index placed is a truncation, not
    /// damage — the same rule the header parse and the walk's window follow.
    #[test]
    fn a_source_that_ends_inside_a_payload_is_a_truncation() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let bytes = std::fs::read(dir.join("many-blocks.xz")).expect("the fixture reads");
        let whole: &[u8] = &bytes;
        let table = SeekTable::from_source(&whole).expect("the fixture walks");
        let block = table.blocks[2];
        let check = table.streams[0].check;

        let cut = &bytes[..(block.compressed_offset + block.unpadded_size / 2) as usize];
        let short: &[u8] = cut;
        for &backend in COMPILED {
            let e = whole_block(
                &short,
                &block,
                check,
                DEFAULT_MEMLIMIT,
                backend,
                INPUT_CHUNK,
            )
            .unwrap_err();
            assert!(
                matches!(e, Error::Truncated { compressed_offset } if compressed_offset == cut.len() as u64),
                "{backend:?}: {e}"
            );
        }
    }

    /// An index entry whose uncompressed size is not the block's is caught by
    /// the counters the loop already keeps, either way it is wrong.
    ///
    /// **Only the first half holds under every backend** — deficiency: KD6,
    /// whose detail is in `docs/design/architecture.md`. Claiming *fewer* bytes
    /// is caught by this loop's own output clamp, which is backend-independent.
    /// Claiming *more* is caught by whoever notices the block ended early, and
    /// they do not agree: `liblzma` decodes a raw chain that knows no declared
    /// length, so the payload ends cleanly and `complete` finds the shortfall,
    /// while `xz4rust` is handed a synthesized header carrying the index's own
    /// numbers and refuses the payload against them.
    #[test]
    fn a_block_whose_size_the_index_contradicts_is_index_inconsistent() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let (source, table) = walked(&dir.join("many-blocks.xz"));
        let check = table.streams[0].check;

        for &backend in COMPILED {
            // Claiming fewer uncompressed bytes than the block holds: the
            // decoder asks for room the index does not allow.
            let mut small = table.blocks[1];
            small.uncompressed_size -= 1024;
            let e = whole_block(
                &source,
                &small,
                check,
                DEFAULT_MEMLIMIT,
                backend,
                INPUT_CHUNK,
            )
            .unwrap_err();
            assert!(
                matches!(e, Error::IndexInconsistent { compressed_offset }
                    if compressed_offset == small.compressed_offset),
                "{backend:?}: {e}"
            );
        }
    }

    /// Claiming *more* uncompressed bytes than the block holds, which only
    /// `liblzma` calls `IndexInconsistent`.
    ///
    /// The other half of the test above, split off because it is
    /// backend-dependent — deficiency: KD6.
    #[cfg(feature = "liblzma")]
    #[test]
    fn a_block_shorter_than_the_index_recorded_is_index_inconsistent_under_liblzma() {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let (source, table) = walked(&dir.join("many-blocks.xz"));
        let check = table.streams[0].check;

        let mut large = table.blocks[1];
        large.uncompressed_size += 1024;
        let e = whole_block(
            &source,
            &large,
            check,
            DEFAULT_MEMLIMIT,
            Backend::Liblzma,
            INPUT_CHUNK,
        )
        .unwrap_err();
        assert!(
            matches!(e, Error::IndexInconsistent { compressed_offset }
                if compressed_offset == large.compressed_offset),
            "{e}"
        );
    }
}
