//! The check algorithms in a form whose whole dependency graph permits no
//! `unsafe`: the `crc` crate for both CRCs, and SHA-256 written out here.
//!
//! This is what `--no-default-features --features xz4rust` compiles instead of
//! `check::fast`, and it is the last of the three things that get that
//! configuration's graph to "nothing outside the standard library permits
//! `unsafe`" — the claim `scripts/unsafe_free.py` checks, and the rows it
//! answers are in `docs/design/architecture.md`, "The default tree is not
//! `unsafe`-free, and no feature makes it so".
//!
//! The module is compiled under `any(not(feature = "fast-checks"), test)`, so
//! it is **absent from a default release build** — nobody pays 48 KiB of
//! `.rodata` for tables they never read — and **present in every `cargo test`**,
//! where the tests below diff it against the three crates it replaces.
//!
//! # Why the diff is against the reference crates, not against `check::fast`
//!
//! Under the unsafe-free configuration `crc32fast`, `crc64fast` and `sha2` are
//! absent from the normal graph — that absence *is* the deliverable — so
//! `check::fast` does not exist there, and a diff written against it would
//! compile only in the configuration that never runs this code. The three
//! crates are unconditional `dev-dependencies` instead, which are outside the
//! claim's scope and present in every configuration's test run.
//!
//! # Why SHA-256 is written here
//!
//! No published crate has the property. `sha2` pulls `cpufeatures`
//! unconditionally on x86 and aarch64 whatever `force-soft` says,
//! `hmac-sha256` carries an `unsafe read_volatile`, and `rs_sha256` is one
//! maintainer at an 0.1 version. The algorithm is fully specified in FIPS
//! 180-4 and fits in a hundred and twenty lines.

use crc::{Crc, Table};

/// What [`super::implementation`] returns in a build that selected this module.
///
/// The word is `"safe"` rather than the name of a feature, because there is no
/// `safe-checks` feature to name — this module is selected by `fast-checks`
/// being *off*, and a word that looked like a flag would invite someone to pass
/// one that does not exist.
pub(crate) const NAME: &str = "safe";

/// CRC-32/ISO-HDLC — the format's CRC32, and the catalogue entry whose `check`
/// value is `0xcbf43926`.
///
/// `Table<16>` is the sixteen-lane table-driven implementation, one of the
/// three `crc` seals; its tables are built by a `const fn`, so 16 KiB lands in
/// `.rodata` with no runtime initialization. A `static` rather than a `const`
/// because a `const` is substituted at each use, which would copy the table
/// into every caller.
static CRC32: Crc<u32, Table<16>> = Crc::<u32, Table<16>>::new(&crc::CRC_32_ISO_HDLC);

/// CRC-64/XZ — poly `0x42f0e1eba9ea3693`, init and xorout all-ones, reflected
/// both ways, and 32 KiB of table.
static CRC64: Crc<u64, Table<16>> = Crc::<u64, Table<16>>::new(&crc::CRC_64_XZ);

pub(crate) struct Crc32(crc::Digest<'static, u32, Table<16>>);

impl Crc32 {
    pub(crate) fn new() -> Crc32 {
        Crc32(CRC32.digest())
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub(crate) fn finish(self) -> u32 {
        self.0.finalize()
    }
}

pub(crate) struct Crc64(crc::Digest<'static, u64, Table<16>>);

impl Crc64 {
    pub(crate) fn new() -> Crc64 {
        Crc64(CRC64.digest())
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub(crate) fn finish(self) -> u64 {
        self.0.finalize()
    }
}

/// FIPS 180-4's sixty-four round constants: the first thirty-two bits of the
/// fractional parts of the cube roots of the first sixty-four primes.
#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The eight initial hash values: the fractional parts of the square roots of
/// the first eight primes.
#[rustfmt::skip]
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// SHA-256 over a stream of arbitrary chunks.
///
/// The state is the eight words plus a partial 64-byte block, because
/// `decode.rs` feeds whatever run of bytes each decode call happened to
/// produce and the compression function only ever sees whole blocks. `len`
/// counts the message in bytes; the padding turns it into the big-endian bit
/// count the last block ends with.
pub(crate) struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    len: u64,
}

impl Sha256 {
    pub(crate) fn new() -> Sha256 {
        Sha256 {
            state: H0,
            block: [0u8; 64],
            filled: 0,
            len: 0,
        }
    }

    pub(crate) fn update(&mut self, mut bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);

        if self.filled > 0 {
            let take = (64 - self.filled).min(bytes.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled < 64 {
                return;
            }
            let block = self.block;
            compress(&mut self.state, &block);
            self.filled = 0;
        }

        let (whole, tail) = bytes.as_chunks::<64>();
        for block in whole {
            compress(&mut self.state, block);
        }

        self.block[..tail.len()].copy_from_slice(tail);
        self.filled = tail.len();
    }

    /// The digest, in the byte order the file stores it in — which for SHA-256
    /// is the ordinary big-endian word store, so no decision is left at the
    /// comparison.
    ///
    /// Padding is one `0x80` byte, zeros, and the 64-bit big-endian bit length
    /// in the last eight. Where the `0x80` leaves fewer than eight bytes of
    /// room, the length spills into a **second** block — which is the case the
    /// tests below pin at input lengths congruent to 55, 56 and 57 mod 64.
    pub(crate) fn finish(mut self) -> [u8; 32] {
        let bits = self.len.wrapping_mul(8);

        self.block[self.filled] = 0x80;
        self.filled += 1;
        if self.filled > 56 {
            self.block[self.filled..].fill(0);
            let block = self.block;
            compress(&mut self.state, &block);
            self.filled = 0;
        }
        self.block[self.filled..56].fill(0);
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        let block = self.block;
        compress(&mut self.state, &block);

        let mut out = [0u8; 32];
        let (slots, _) = out.as_chunks_mut::<4>();
        for (word, slot) in self.state.iter().zip(slots) {
            *slot = word.to_be_bytes();
        }
        out
    }
}

/// One application of FIPS 180-4's compression function, over one 64-byte
/// block.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    let (words, _) = block.as_chunks::<4>();
    for (word, chunk) in w.iter_mut().zip(words) {
        *word = u32::from_be_bytes(*chunk);
    }
    for i in 16..64 {
        let a = w[i - 15];
        let b = w[i - 2];
        let s0 = a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3);
        let s1 = b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (k, wi) in K.iter().zip(w.iter()) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(*k)
            .wrapping_add(*wi);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    for (slot, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(add);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference implementations, reached directly rather than through
    /// [`super::super::fast`] — see this module's docs for why that distinction
    /// is not pedantry.
    fn reference(bytes: &[u8]) -> (u32, u64, [u8; 32]) {
        (
            crc32fast::hash(bytes),
            {
                let mut d = crc64fast::Digest::new();
                d.write(bytes);
                d.sum64()
            },
            {
                use sha2::Digest as _;
                sha2::Sha256::digest(bytes).into()
            },
        )
    }

    /// The same three, from this module, in one call.
    fn ours(chunks: &[&[u8]]) -> (u32, u64, [u8; 32]) {
        let mut a = Crc32::new();
        let mut b = Crc64::new();
        let mut c = Sha256::new();
        for chunk in chunks {
            a.update(chunk);
            b.update(chunk);
            c.update(chunk);
        }
        (a.finish(), b.finish(), c.finish())
    }

    fn hex(digest: &[u8; 32]) -> String {
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Every intact fixture's plaintext, decoded once, so the two corpus tests
    /// below pay for `xz -dc` a single time each.
    ///
    /// `bulk-blocks.xz` is in it: its 16 MiB costs 0.23 s across both tests,
    /// which is not what the byte sweep excludes it over — `harness.md`,
    /// "`bulk-blocks.xz` is out of the byte sweep and in everything else".
    fn corpus() -> Vec<(&'static str, Vec<u8>)> {
        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        fixtures_gen::FIXTURES
            .iter()
            .filter(|f| f.intact)
            .map(|f| {
                let bytes = harness::oracle::plaintext_for(&dir.join(f.name))
                    .unwrap_or_else(|e| panic!("{}: {e}", f.name));
                (f.name, bytes)
            })
            .collect()
    }

    /// The published vectors, the empty input included.
    ///
    /// A wrong round constant or a mistyped initial word fails here
    /// immediately, which is exactly why this is not the interesting test: what
    /// survives a one-shot vector and fails on real input is the incremental
    /// buffering, and the three tests after this one are the ones that find
    /// that.
    ///
    /// The CRC lines are the catalogue's own `check` values, which is what
    /// holds `crc` to the two algorithms the format names rather than to two
    /// others with the same widths.
    #[test]
    fn the_published_vectors() {
        assert_eq!(ours(&[b"123456789"]).0, 0xcbf4_3926, "CRC-32/ISO-HDLC");
        assert_eq!(ours(&[b"123456789"]).1, 0x995d_c9bb_df19_39fa, "CRC-64/XZ");

        assert_eq!(
            hex(&ours(&[]).2),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "SHA-256 of the empty input"
        );
        assert_eq!(
            hex(&ours(&[b"abc"]).2),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-256(\"abc\")"
        );
        assert_eq!(
            hex(&ours(&[b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"]).2),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            "SHA-256 of FIPS 180-4's 448-bit message"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&ours(&[&million]).2),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            "SHA-256 of a million 'a'"
        );
    }

    /// Byte for byte against `crc32fast`, `crc64fast` and `sha2`, over every
    /// intact fixture's full uncompressed bytes.
    ///
    /// This runs in **every** configuration, because the three reference crates
    /// are unconditional dev-dependencies and this module is compiled under
    /// `test` whether `fast-checks` is on or off.
    #[test]
    fn the_corpus_hashes_to_what_the_reference_crates_say() {
        let corpus = corpus();
        assert!(corpus.len() > 5, "the corpus is smaller than expected");
        for (name, plaintext) in &corpus {
            assert_eq!(ours(&[plaintext]), reference(plaintext), "{name}");
        }
    }

    /// The same bytes in random-sized pieces must give the digest the one-shot
    /// call gives.
    ///
    /// This is the failure mode the module is actually exposed to: `check.rs`
    /// is fed whatever run of bytes each decode call happened to produce, so a
    /// carry mishandled between calls, or length padding misplaced at a block
    /// boundary, survives every test that hashes a whole input at once.
    ///
    /// The generator is `harness`'s xorshift, seeded per fixture, so a failure
    /// reproduces on every machine.
    #[test]
    fn random_chunking_changes_no_digest() {
        for (name, plaintext) in &corpus() {
            let whole = ours(&[plaintext]);
            let mut rng = harness::sweep::Rng(0x5ec7_0000_0000_0001);
            for round in 0..4 {
                let mut pieces: Vec<&[u8]> = Vec::new();
                let mut rest = &plaintext[..];
                while !rest.is_empty() {
                    // Sizes across the whole range that matters: below one
                    // block, exactly one, and several.
                    let n = (rng.next() % 200) as usize;
                    let n = n.min(rest.len());
                    let (head, tail) = rest.split_at(n);
                    pieces.push(head);
                    rest = tail;
                }
                assert_eq!(ours(&pieces), whole, "{name}, round {round}");
            }
        }
    }

    /// The padding boundary, explicitly.
    ///
    /// At 55 bytes mod 64 the `0x80` and the eight length bytes just fit; at 56
    /// and 57 they do not, and the length spills into a second padded block.
    /// The lengths either side are there so a mistake in the comparison itself
    /// — `>` for `>=` — is not silently symmetric.
    #[test]
    fn the_padding_boundary_spills_into_a_second_block() {
        let noise: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        for base in [0usize, 64, 128, 512] {
            for offset in [54, 55, 56, 57, 58, 63] {
                let len = base + offset;
                let input = &noise[..len];
                assert_eq!(ours(&[input]).2, reference(input).2, "length {len}");
            }
        }
    }
}
