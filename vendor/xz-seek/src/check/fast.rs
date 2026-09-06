//! The check algorithms as the SIMD crates compute them: `crc32fast`,
//! `crc64fast` and `sha2`.
//!
//! This is the default build's implementation, and `F2`'s denominator in
//! `docs/design/measurements.md`. It exists as a module rather than inline in
//! [`super`] so that the two implementations present the *same* three types
//! with the same three methods, and the selection is one `use` rather than a
//! `cfg` at every call site.
//!
//! Each type is a newtype over the crate's own hasher. `finish` returns the
//! algorithm's natural value — a `u32`, a `u64`, thirty-two bytes — and the
//! decision that the two CRCs are stored little-endian is made once, in
//! [`Hasher::finish`](super::Hasher::finish).

/// What [`super::implementation`] returns in a build that selected this module.
pub(crate) const NAME: &str = "fast";

pub(crate) struct Crc32(crc32fast::Hasher);

impl Crc32 {
    pub(crate) fn new() -> Crc32 {
        Crc32(crc32fast::Hasher::new())
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub(crate) fn finish(self) -> u32 {
        self.0.finalize()
    }
}

pub(crate) struct Crc64(crc64fast::Digest);

impl Crc64 {
    pub(crate) fn new() -> Crc64 {
        Crc64(crc64fast::Digest::new())
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    pub(crate) fn finish(self) -> u64 {
        self.0.sum64()
    }
}

pub(crate) struct Sha256(sha2::Sha256);

impl Sha256 {
    pub(crate) fn new() -> Sha256 {
        Sha256(<sha2::Sha256 as sha2::Digest>::new())
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        sha2::Digest::update(&mut self.0, bytes);
    }

    pub(crate) fn finish(self) -> [u8; 32] {
        sha2::Digest::finalize(self.0).into()
    }
}
