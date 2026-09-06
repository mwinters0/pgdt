//! The block's integrity check, and the container's own CRC32s, computed in
//! this crate rather than by the backend.
//!
//! `liblzma`'s raw decoder verifies nothing — it consumes a payload and knows
//! nothing of what follows it (`xz-invariants.md`, `I5`) — so the check is ours
//! either way. Computing it here is also what keeps `#![forbid(unsafe_code)]`
//! true under both backends: `lzma_crc32`/`lzma_crc64` exist only as
//! `unsafe extern` in `liblzma-sys`, SHA-256 is not bound at any level, and
//! none of the three exists at all in an `xz4rust`-only build. See
//! `docs/design/architecture.md`, "The check algorithms".
//!
//! # Two implementations, selected by `cfg` rather than by resolution
//!
//! `check::fast` is `crc32fast`, `crc64fast` and `sha2` — the default build, and
//! what the throughput figures were taken over. `check::safe` is the `crc` crate
//! and a SHA-256 written here, for the configuration whose whole dependency
//! graph permits no `unsafe`. Both present the same three types with the same
//! three methods, so the selection is the single `use … as imp` below and
//! nothing downstream of it carries a `cfg`.
//!
//! There is deliberately no `safe-checks` feature: the safe side has no
//! optional dependency to switch on, and gating it on a feature would put it
//! outside `cargo test --workspace`. Its `cfg` includes `test` instead, which
//! is what keeps it inside the standing gate — see
//! `docs/design/architecture.md`, "Two implementations, selected by `cfg`
//! rather than by resolution".
//!
//! **[`crc32`] routes through the same swap**, because the claim being made is
//! that `crc32fast` is *absent* from the unsafe-free build's graph, and the
//! four structural CRC32s — the stream header's, each block header's, the
//! index's and the footer's — are as much a use of that crate as the block
//! check is. `walk.rs`, `block.rs` and `backend.rs` all call it.
//!
//! # How the value is stored
//!
//! CRC32 and CRC64 go into the file **little-endian**; SHA-256 goes in as its
//! ordinary digest bytes. That is `lzma_check_finish` in
//! `src/liblzma/check/check.c`, which is `conv32le`, `conv64le` and
//! `lzma_sha256_finish`'s big-endian word store respectively. So a [`Digest`]
//! compares byte for byte against the bytes read out of the file, with no
//! endianness decision left at the comparison. The two implementations return
//! the algorithm's natural `u32`/`u64` and that decision is made once, in
//! [`Hasher::finish`].

use crate::table::Check;

#[cfg(feature = "fast-checks")]
mod fast;
#[cfg(any(not(feature = "fast-checks"), test))]
mod safe;

#[cfg(feature = "fast-checks")]
use fast as imp;
#[cfg(not(feature = "fast-checks"))]
use safe as imp;

/// The name of the check implementation this build compiled: `"fast"` or
/// `"safe"`.
///
/// `"fast"` is `crc32fast`, `crc64fast` and `sha2`, which the `fast-checks`
/// feature switches in and the default build carries; `"safe"` is the `crc`
/// crate and the SHA-256 written here, which is what the configuration whose
/// graph permits no `unsafe` computes every block check with.
///
/// **It exists because the checks leave no trace in an invocation.** A backend
/// is named on the command line, so a log of the command records it; the check
/// implementation is resolved at build time by a feature's *absence*, so a
/// measurement taken under it has nothing to recover the configuration from
/// unless the program prints this. `harness`'s `corpus` binary does, in its
/// header, in its coverage block beside the backend, and on its verdict line.
///
/// A function returning a word rather than an enum beside [`crate::Backend`]:
/// a caller chooses a backend at runtime through [`crate::Builder::backend`]
/// and can never choose a check implementation, so a type mirroring `Backend`
/// would invite a selector that cannot exist.
pub fn implementation() -> &'static str {
    imp::NAME
}

/// The largest check this crate computes: SHA-256's thirty-two bytes.
///
/// The format reserves ids whose check field is up to sixty-four bytes, but
/// those are never computed — a stream carrying one is refused before any
/// block in it is decoded.
const DIGEST_MAX: usize = 32;

/// A computed check value, in the byte order the file stores it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Digest {
    bytes: [u8; DIGEST_MAX],
    len: usize,
}

impl Digest {
    /// The value, sized to the check that produced it.
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn of(value: &[u8]) -> Digest {
        let mut bytes = [0u8; DIGEST_MAX];
        bytes[..value.len()].copy_from_slice(value);
        Digest {
            bytes,
            len: value.len(),
        }
    }
}

/// The CRC32 of `bytes`, one-shot.
///
/// The format's structural fields — the stream header's flags, every block
/// header, the index, and the footer — each carry a CRC32 over a handful of
/// bytes, and they are checked and (in the synthesized stream and the tests)
/// written through here. Streaming would buy nothing at those sizes; what this
/// buys is that there is exactly one place in the crate that names a CRC32
/// implementation.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut h = imp::Crc32::new();
    h.update(bytes);
    h.finish()
}

/// One block's check, computed over the uncompressed bytes as they are
/// produced.
///
/// Streaming rather than over a retained block, because the roadmap's
/// "Verification streams with the block" forbids retaining one.
pub(crate) enum Hasher {
    /// `--check=none`: nothing to compute, and an empty value to compare.
    None,
    Crc32(imp::Crc32),
    Crc64(imp::Crc64),
    Sha256(imp::Sha256),
}

impl Hasher {
    /// The hasher for `check`, or `None` where this crate has no algorithm for
    /// it.
    ///
    /// The `None` case is a reserved check id. What the caller does about it is
    /// the verification state's business, not this module's.
    pub(crate) fn new(check: Check) -> Option<Hasher> {
        match check {
            Check::None => Some(Hasher::None),
            Check::Crc32 => Some(Hasher::Crc32(imp::Crc32::new())),
            Check::Crc64 => Some(Hasher::Crc64(imp::Crc64::new())),
            Check::Sha256 => Some(Hasher::Sha256(imp::Sha256::new())),
            Check::Unsupported(_) => None,
        }
    }

    /// Fold the next run of uncompressed bytes in.
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::None => {}
            Hasher::Crc32(h) => h.update(bytes),
            Hasher::Crc64(h) => h.update(bytes),
            Hasher::Sha256(h) => h.update(bytes),
        }
    }

    /// The value to compare against the bytes stored after the payload.
    pub(crate) fn finish(self) -> Digest {
        match self {
            Hasher::None => Digest::of(&[]),
            Hasher::Crc32(h) => Digest::of(&h.finish().to_le_bytes()),
            Hasher::Crc64(h) => Digest::of(&h.finish().to_le_bytes()),
            Hasher::Sha256(h) => Digest::of(&h.finish()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word [`implementation`] prints names the module that actually
    /// computes a check — which matters most in the build where both are
    /// compiled.
    ///
    /// `check::safe` is compiled under `cfg(test)` whatever the feature says,
    /// so in a default `cargo test` run both modules exist and only one of them
    /// is what a figure was measured over. A word that named the compiled
    /// module rather than the selected one would mislabel every measurement
    /// taken under the default build.
    #[test]
    fn the_word_names_the_module_that_computes_the_checks() {
        #[cfg(feature = "fast-checks")]
        {
            assert_eq!(implementation(), "fast");
            assert_ne!(implementation(), safe::NAME);
        }
        #[cfg(not(feature = "fast-checks"))]
        assert_eq!(implementation(), "safe");
    }

    /// The three algorithms against published vectors, in the byte order the
    /// file stores them in.
    ///
    /// The CRC64 the format names is specifically CRC-64/XZ — `crc64fast`'s own
    /// README pins `"hello world!"` at `0x8483_c0fa_3260_7d61` — and this is
    /// what holds whichever implementation is selected to that: a CRC-64 with
    /// any other polynomial fails here rather than at a fixture three modules
    /// away.
    #[test]
    fn each_check_is_the_algorithm_the_format_names() {
        let mut h = Hasher::new(Check::Crc32).unwrap();
        h.update(b"123456789");
        assert_eq!(h.finish().as_slice(), &0xcbf4_3926u32.to_le_bytes());

        let mut h = Hasher::new(Check::Crc64).unwrap();
        h.update(b"hello ");
        h.update(b"world!");
        assert_eq!(
            h.finish().as_slice(),
            &0x8483_c0fa_3260_7d61u64.to_le_bytes()
        );

        // The SHA-256 of the empty string.
        let h = Hasher::new(Check::Sha256).unwrap();
        assert_eq!(
            h.finish()
                .as_slice()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// A check's computed value is exactly as long as the field the file holds
    /// it in, which is what lets the comparison be a plain slice equality.
    #[test]
    fn a_digest_is_as_long_as_the_check_field_it_is_compared_against() {
        for check in [Check::None, Check::Crc32, Check::Crc64, Check::Sha256] {
            let mut h = Hasher::new(check).unwrap();
            h.update(b"some bytes");
            assert_eq!(
                h.finish().as_slice().len() as u64,
                check.size(),
                "{check:?}"
            );
        }
    }

    /// Splitting the input changes nothing: the check streams with the block,
    /// so it is fed whatever run of bytes each decode call happened to produce.
    #[test]
    fn feeding_the_same_bytes_in_pieces_gives_the_same_value() {
        let data: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        for check in [Check::None, Check::Crc32, Check::Crc64, Check::Sha256] {
            let mut whole = Hasher::new(check).unwrap();
            whole.update(&data);

            let mut split = Hasher::new(check).unwrap();
            for piece in data.chunks(37) {
                split.update(piece);
            }
            assert_eq!(whole.finish(), split.finish(), "{check:?}");
        }
    }

    /// A reserved id has no algorithm here, and this module says so rather than
    /// substituting one.
    #[test]
    fn a_reserved_check_id_has_no_hasher() {
        assert!(Hasher::new(Check::Unsupported(0x05)).is_none());
    }
}
