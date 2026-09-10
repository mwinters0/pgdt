//! The block-payload seam: a filter chain and a payload in, uncompressed bytes
//! out.
//!
//! **It carries no container knowledge**, which is the whole of what makes it a
//! seam — see `docs/design/roadmap.md`, "The decoder backend is `liblzma`". It
//! knows nothing of block headers, checks, block padding, the index or a file
//! offset, so nothing it returns can name one: [`SeamError`] says *what* went
//! wrong and [`crate::decode`] says *where*.
//!
//! What it *is* told is how big the block is — [`PayloadDecoder::new`] takes the
//! payload's length and the block's uncompressed length — because that is the
//! least a backend that decodes *streams* needs in order to find a *block's*
//! end. See `docs/design/architecture.md`, "The block-payload seam".
//!
//! `liblzma` fits it natively and ignores both sizes.
//! `Stream::new_raw_decoder` over a chain built from
//! the header's own property bytes consumes exactly one payload and reports
//! `Status::StreamEnd` at its last byte (`xz-invariants.md`, `I5`), and it does
//! that through the crate's safe API — no `unsafe` here and none in
//! `liblzma`'s signature, which is what keeps `#![forbid(unsafe_code)]` true.
//!
//! `xz4rust` decodes whole streams and has no raw-chain entry point, so the arm
//! **synthesizes a one-block stream around the payload**: a stream header
//! declaring `check=None`, a block header re-emitted from the parsed chain by
//! [`crate::block::encode`], the payload, block padding, a one-record index and
//! a footer. `EndOfStream` then lands on the block's last byte, so
//! [`Progress::finished`] means the same thing under both backends. The
//! structural bytes are this crate's own, which is what the bug discriminator
//! below rests on.
//!
//! # Buildability is declared here, and nowhere else
//!
//! "This crate cannot build that filter" is a statement about a **backend**, so
//! it is made at the seam. The block header parse walks a chain without needing
//! to recognise a single id — each filter's property length is an explicit
//! variable-length integer — and recognises one only to derive LZMA2's
//! dictionary size and to validate a property field's length, both of which are
//! the format's business rather than a backend's. Reasoning:
//! `docs/status/history/2026-09-03.md`, "Buildability is the backend's to
//! declare, not the parse's".
//!
//! Both arms answer the same set, and the `xz4rust` arm checks it **before** it
//! emits a header rather than letting the decoder discover an unknown id while
//! parsing one: `XzError::UnsupportedBlockHeaderOption` carries no id, and
//! [`SeamError::UnsupportedFilter`] has to carry one.
//!
//! # Progress, not the return code
//!
//! [`PayloadDecoder::decode`] reports what actually moved — input consumed,
//! output produced, and whether the payload ended — because the return code
//! cannot be trusted to say. A damaged payload that leaves the decoder wanting
//! more input than the block has arrives as `Ok(Status::MemNeeded)`, a
//! *success* value, since the Rust binding maps `LZMA_BUF_ERROR` onto it and
//! has no error variant for it at all (`xz-invariants.md`, `I8`). The caller
//! knows the payload's length from the index, so "no progress and nothing left
//! to offer" is the conclusion, and it is reached without reading a status.
//!
//! # Neither slice has to be sized to anything, and the `xz4rust` arm pays for it
//!
//! [`crate::decode`] hands the seam an empty *output* slice on the last
//! iteration of every block, and an empty *input* slice whenever the payload is
//! spent. `liblzma` answers both with `Ok(Progress { 0, 0, false })`;
//! `xz4rust` answers an empty input slice with
//! `Err(XzError::NeedsLargerInputBuffer)` unconditionally (`I16`). **The arm
//! absorbs that rather than the seam's contract moving**: it never forwards an
//! empty input slice, because that call is exactly where the payload is spent
//! and so where the arm pushes its own trailer and collects `EndOfStream`.

use crate::block::Filter;

/// What one [`PayloadDecoder::decode`] call moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Progress {
    /// Compressed bytes taken from the input slice.
    pub in_used: usize,
    /// Uncompressed bytes written into the output slice.
    pub out_written: usize,
    /// The payload ended here: every byte of the block's uncompressed data has
    /// been produced.
    pub finished: bool,
}

/// A failure with no coordinates.
///
/// The seam does not know where in a file it is working, so it names the fault
/// and leaves the offset to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeamError {
    /// The chain names a filter id this backend does not build.
    UnsupportedFilter(u64),
    /// The chain is one this backend refuses to assemble: properties a filter
    /// rejects, or a chain shape the format does not permit — it must end in
    /// LZMA2, and every filter before it must preserve size.
    ///
    /// Those rules are left to the backend rather than copied into the header
    /// parse, where they would be a second and weaker statement of them.
    UnusableChain,
    /// The payload is not a decodable block.
    Data,
}

/// Which implementation decodes a block's payload.
///
/// Both variants exist whatever features are on, so a caller's `match` over
/// this compiles in every configuration — which is worth more than making an
/// unavailable request unrepresentable, the thing `cfg`-gated variants would
/// buy at the price of an enum whose shape depends on how someone else
/// configured the build. In a single-backend build the other variant is
/// constructed nowhere here, and that is the point rather than dead code.
///
/// Whether a build *carries* one is [`Backend::is_available`]'s question.
/// Naming one it does not carry is [`crate::Error::BackendUnavailable`], raised
/// by [`crate::Builder`] before any byte is read; the seam itself is only ever
/// reached with a compiled backend.
///
/// It is named at [`crate::Builder::backend`], and nothing else in the crate
/// takes it from a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The C library, through `liblzma`'s safe raw-decoder API.
    ///
    /// Compiled by the `liblzma` feature, which is on by default.
    Liblzma,
    /// The pure-Rust decoder, through a synthesized one-block stream.
    ///
    /// Compiled by the `xz4rust` feature, which is off by default. It is the
    /// backend of the build in which no crate outside the standard library
    /// permits `unsafe`.
    Xz4rust,
}

impl Backend {
    /// The Cargo feature that compiles this backend.
    ///
    /// It is also the name the error and the docs use, so a caller told a
    /// backend is unavailable is told the flag that would make it available.
    pub fn feature(self) -> &'static str {
        match self {
            Backend::Liblzma => "liblzma",
            Backend::Xz4rust => "xz4rust",
        }
    }

    /// Whether this build compiled this backend.
    ///
    /// [`crate::Builder::open`] asks this for the caller and refuses with
    /// [`crate::Error::BackendUnavailable`], so a caller only needs it to
    /// choose between backends it knows are there. It reads `COMPILED`, the
    /// crate-private list of what this build carries.
    pub fn is_available(self) -> bool {
        COMPILED.contains(&self)
    }

    /// Everything one live payload decoder holds beyond the `D` bytes of
    /// dictionary its block declares, in bytes.
    ///
    /// A **constant per backend**, because it is one: neither figure varies with
    /// the dictionary, the payload or the LZMA2 properties, and each already
    /// covers the worst chain the format allows. It folds two things together —
    /// the decoder's own state, and the overhang the dictionary buffer carries
    /// above `D` — so that a footprint stays the three terms it is described in
    /// (`dictionary + input chunk + backend state`) rather than four, and so
    /// that `liblzma`'s `round16(max(D, 4096)) + 608` is charged rather than
    /// silently dropped.
    ///
    /// Both numbers are traced to source in
    /// `docs/design/xz-invariants.md`, `I24`, which carries the re-verify
    /// command; `tests/footprint.rs` is what holds them against a live decode.
    /// They are struct layouts, so they move with a compiler, a target, a
    /// feature or a release.
    ///
    /// Not public. It is a term of [`crate::Reader::decode_footprint`] and of
    /// [`crate::RangePlan::decoder_bytes`], and both callers take the backend
    /// from the reader that will do the decoding; a caller holding this alone
    /// could pair it with a backend it is not running, which is the mistake the
    /// query's whole shape exists to prevent.
    pub(crate) fn decoder_state_bytes(self) -> u64 {
        match self {
            Backend::Liblzma => 34_592,
            Backend::Xz4rust => 30_680,
        }
    }
}

impl core::fmt::Display for Backend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.feature())
    }
}

/// The backend used when nothing names one.
///
/// `liblzma` wherever it is compiled, so the default build behaves exactly as
/// it did before the second arm existed; `xz4rust` otherwise, so the
/// unsafe-free build needs no selector call.
#[cfg(feature = "liblzma")]
pub(crate) const DEFAULT: Backend = Backend::Liblzma;
#[cfg(not(feature = "liblzma"))]
pub(crate) const DEFAULT: Backend = Backend::Xz4rust;

/// Every backend this build carries, in feature order.
///
/// Never empty: a build with no backend is a `compile_error!` in
/// [`crate`]'s root. This is what a differential test iterates, so that a claim
/// about "the backend" is made about each one the run actually compiled, and it
/// is what [`Backend::is_available`] answers from.
pub(crate) const COMPILED: &[Backend] = &[
    #[cfg(feature = "liblzma")]
    Backend::Liblzma,
    #[cfg(feature = "xz4rust")]
    Backend::Xz4rust,
];

/// Delta.
const FILTER_DELTA: u64 = 0x03;
/// The BCJ family, x86 first.
const FILTER_X86: u64 = 0x04;
const FILTER_POWERPC: u64 = 0x05;
const FILTER_IA64: u64 = 0x06;
const FILTER_ARM: u64 = 0x07;
const FILTER_ARMTHUMB: u64 = 0x08;
const FILTER_SPARC: u64 = 0x09;
const FILTER_ARM64: u64 = 0x0a;
const FILTER_RISCV: u64 = 0x0b;
/// LZMA2, which every valid block chain ends in.
const FILTER_LZMA2: u64 = 0x21;

/// A live decoder over exactly one block's compressed payload.
pub(crate) enum PayloadDecoder {
    #[cfg(feature = "liblzma")]
    Liblzma(Liblzma),
    #[cfg(feature = "xz4rust")]
    Xz4rust(Xz4rust),
}

impl PayloadDecoder {
    /// Build the chain and start a decoder over it.
    ///
    /// `filters` is in header order, which is filter-*input* order: the last
    /// element is the one that produced the compressed bytes. Each filter's
    /// properties go in **verbatim**, exactly as the header carried them —
    /// nothing here interprets them, which is why both arms can be handed the
    /// same [`Filter`] list.
    ///
    /// `payload_size` is the Compressed Data field's length and
    /// `uncompressed_size` the block's output length, both from the index. The
    /// `liblzma` arm ignores them; the `xz4rust` arm declares both in the header
    /// it synthesizes, so the decoder *enforces* them.
    ///
    /// `backend` must be one this build compiled — see [`COMPILED`].
    pub(crate) fn new(
        backend: Backend,
        filters: &[Filter],
        payload_size: u64,
        uncompressed_size: u64,
    ) -> Result<PayloadDecoder, SeamError> {
        match backend {
            #[cfg(feature = "liblzma")]
            Backend::Liblzma => {
                let _ = (payload_size, uncompressed_size);
                Ok(PayloadDecoder::Liblzma(Liblzma::new(filters)?))
            }
            #[cfg(feature = "xz4rust")]
            Backend::Xz4rust => Ok(PayloadDecoder::Xz4rust(Xz4rust::new(
                filters,
                payload_size,
                uncompressed_size,
            )?)),
            // Unreachable: nothing inside this crate names a backend it did not
            // compile, and a caller who does is refused before a block is
            // started. In a build carrying both backends this arm is dead, and
            // in a build carrying one it catches the other's name.
            #[allow(unreachable_patterns)]
            absent => unreachable!("{absent:?} is not compiled into this build"),
        }
    }

    /// Push `input` through the chain into `output`.
    ///
    /// Neither slice has to be sized to anything: the payload is offered in
    /// whatever chunks the caller has read, and `output` may legitimately be
    /// empty when the caller is checking whether the block ends without wanting
    /// bytes. An empty `input` means the payload is spent.
    pub(crate) fn decode(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<Progress, SeamError> {
        match self {
            #[cfg(feature = "liblzma")]
            PayloadDecoder::Liblzma(d) => d.decode(input, output),
            #[cfg(feature = "xz4rust")]
            PayloadDecoder::Xz4rust(d) => d.decode(input, output),
        }
    }

    /// Compressed payload bytes consumed since the decoder was built.
    ///
    /// Payload bytes only: the structural bytes the `xz4rust` arm pushes around
    /// them are not the file's and are not counted. Exact after a failure as
    /// well as after a success under `liblzma`, which is what
    /// [`crate::Error::BlockDataError`]'s offset is built from; the `xz4rust`
    /// arm can only be exact to the failing call's first byte, since its
    /// decoder discards a failed call's progress.
    pub(crate) fn input_consumed(&self) -> u64 {
        match self {
            #[cfg(feature = "liblzma")]
            PayloadDecoder::Liblzma(d) => d.input_consumed(),
            #[cfg(feature = "xz4rust")]
            PayloadDecoder::Xz4rust(d) => d.input_consumed(),
        }
    }
}

// ---------------------------------------------------------------- liblzma

#[cfg(feature = "liblzma")]
pub(crate) struct Liblzma {
    stream: liblzma::stream::Stream,
}

#[cfg(feature = "liblzma")]
impl Liblzma {
    fn new(filters: &[Filter]) -> Result<Liblzma, SeamError> {
        use liblzma::stream::{Filters, Stream};

        let mut chain = Filters::new();
        for f in filters {
            let added = match f.id {
                FILTER_DELTA => chain.delta_properties(&f.props),
                FILTER_X86 => chain.x86_properties(&f.props),
                FILTER_POWERPC => chain.powerpc_properties(&f.props),
                FILTER_IA64 => chain.ia64_properties(&f.props),
                FILTER_ARM => chain.arm_properties(&f.props),
                FILTER_ARMTHUMB => chain.arm_thumb_properties(&f.props),
                FILTER_SPARC => chain.sparc_properties(&f.props),
                FILTER_ARM64 => chain.arm64_properties(&f.props),
                FILTER_RISCV => chain.riscv_properties(&f.props),
                FILTER_LZMA2 => chain.lzma2_properties(&f.props),
                id => return Err(SeamError::UnsupportedFilter(id)),
            };
            added.map_err(|_| SeamError::UnusableChain)?;
        }
        let stream = Stream::new_raw_decoder(&chain).map_err(|_| SeamError::UnusableChain)?;
        Ok(Liblzma { stream })
    }

    fn decode(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, SeamError> {
        use liblzma::stream::{Action, Status};

        let before_in = self.stream.total_in();
        let before_out = self.stream.total_out();
        let status = self.stream.process(input, output, Action::Run);
        // `lzma_code` updates both totals before it decides its return code, so
        // these are accurate on the failing call too — which is what makes
        // `input_consumed` exact at the byte a bad payload stopped on.
        let progress = Progress {
            in_used: (self.stream.total_in() - before_in) as usize,
            out_written: (self.stream.total_out() - before_out) as usize,
            finished: matches!(status, Ok(Status::StreamEnd)),
        };
        match status {
            Ok(_) => Ok(progress),
            Err(_) => Err(SeamError::Data),
        }
    }

    fn input_consumed(&self) -> u64 {
        self.stream.total_in()
    }
}

// ---------------------------------------------------------------- xz4rust

/// The stream flags of the synthesized stream: reserved zero, `check=None`.
///
/// `check=None` is what makes the construction cheap and keeps verification
/// honest — there is no check field to fabricate, and integrity stays in
/// [`crate::check`] under both backends rather than being computed twice under
/// one of them. It is also what makes `xz4rust`'s own check code dead, which is
/// why the crate is configured with `crc64` and `sha256` off.
#[cfg(feature = "xz4rust")]
const STREAM_FLAGS: [u8; 2] = [0x00, 0x00];

// The thread's parked decoder and the dictionary size it was built for.
//
// **Compiled only under `--cfg xz_seek_decoder_reuse`, which no Cargo feature
// can turn on.** It exists so that `F4` has a second arm to measure the shipped
// one against; the seam builds a decoder per block and that is not in question
// here. See "The decoder is built per block, and one `cfg` build reuses it" in
// `docs/design/architecture.md`.
//
// A *spare* rather than a shared decoder, because two `Xz4rust` arms can be
// alive on one thread — a long-lived reader and a second one over the same
// source — so each has to own its decoder exclusively while it runs. The slot
// holds whichever finished last.
#[cfg(all(feature = "xz4rust", xz_seek_decoder_reuse))]
thread_local! {
    /// The slot itself. See the comment above it.
    static SPARE: core::cell::RefCell<Option<(usize, Box<xz4rust::XzDecoder<'static>>)>> =
        const { core::cell::RefCell::new(None) };
}

/// The thread's spare decoder if it was built for `dict`, reset; a fresh one
/// otherwise.
///
/// `XzDecoder::reset` keeps the dictionary buffer and `alloc_dict` returns
/// early when the buffer in hand is big enough (`xz-invariants.md`, `I19`), so a
/// reused decoder asks the allocator for nothing. What that skips is a `calloc`
/// of the dictionary — an 8 MiB memset on the koji shapes, and not a page fault,
/// because nothing is unmapped between blocks. It is the whole of what `F4`
/// prices.
#[cfg(all(feature = "xz4rust", xz_seek_decoder_reuse))]
fn take_spare(dict: usize) -> Box<xz4rust::XzDecoder<'static>> {
    let spare = SPARE.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_ref() {
            Some((d, _)) if *d == dict => slot.take().map(|(_, decoder)| decoder),
            _ => None,
        }
    });
    match spare {
        Some(mut decoder) => {
            decoder.reset();
            decoder
        }
        None => xz4rust::XzDecoder::in_heap_with_alloc_dict_size(dict, dict),
    }
}

/// Park a finished decoder in the thread's spare slot.
///
/// A failed decode parks too: [`take_spare`] resets whatever it takes, so a
/// poisoned decoder cannot outlive the block it failed on.
#[cfg(all(feature = "xz4rust", xz_seek_decoder_reuse))]
impl Drop for Xz4rust {
    fn drop(&mut self) {
        if let Some(decoder) = self.decoder.take() {
            let dict = self.dict;
            SPARE.with(|slot| *slot.borrow_mut() = Some((dict, decoder)));
        }
    }
}

/// The decoder out of the slot the build shapes it into.
///
/// A free function over the field rather than a method on the arm, because
/// [`Xz4rust::push_trailer`] decodes *out of* `self.trailer` while it holds the
/// decoder: a method would borrow the whole arm and the two field borrows would
/// stop being disjoint.
#[cfg(all(feature = "xz4rust", not(xz_seek_decoder_reuse)))]
#[inline]
fn decoder_mut<'a>(
    slot: &'a mut Box<xz4rust::XzDecoder<'static>>,
) -> &'a mut xz4rust::XzDecoder<'static> {
    slot
}

/// The parked decoder, which is present for the arm's whole life.
#[cfg(all(feature = "xz4rust", xz_seek_decoder_reuse))]
#[inline]
fn decoder_mut<'a>(
    slot: &'a mut Option<Box<xz4rust::XzDecoder<'static>>>,
) -> &'a mut xz4rust::XzDecoder<'static> {
    slot.as_mut()
        .expect("the decoder is present for the arm's whole life")
}

/// A live `xz4rust` decoder over a one-block stream synthesized around the
/// payload.
///
/// The decoder is driven in three phases: the structural prefix in
/// [`Xz4rust::new`], the payload as the caller offers it, and the trailer at the
/// moment the payload is spent. The counters are what separate the file's bytes
/// from this crate's, both for [`Xz4rust::input_consumed`] and for the bug
/// discriminator in [`Xz4rust::push_trailer`].
#[cfg(feature = "xz4rust")]
pub(crate) struct Xz4rust {
    #[cfg(not(xz_seek_decoder_reuse))]
    decoder: Box<xz4rust::XzDecoder<'static>>,
    /// An `Option` only so that [`Drop`] can move the decoder into the thread's
    /// spare slot; it is `Some` for the whole of the arm's life. The shipped
    /// build carries the `Box` above and no `Drop` impl at all.
    #[cfg(xz_seek_decoder_reuse)]
    decoder: Option<Box<xz4rust::XzDecoder<'static>>>,
    /// The dictionary this decoder was built for, which is the spare slot's key.
    #[cfg(xz_seek_decoder_reuse)]
    dict: usize,
    /// Block padding, the one-record index and the footer, built at
    /// construction and pushed once the payload is spent.
    trailer: Vec<u8>,
    /// How far into [`Xz4rust::trailer`] the push has got. The push is
    /// resumable because it is also the block's drain — see
    /// [`Xz4rust::push_trailer`] — so it can run out of the caller's output
    /// room and be re-entered.
    trailer_at: usize,
    payload_size: u64,
    uncompressed_size: u64,
    /// Payload bytes taken. Structural bytes are not counted.
    in_used: u64,
    out_written: u64,
    finished: bool,
}

#[cfg(feature = "xz4rust")]
impl Xz4rust {
    /// Build the decoder and push the synthesized stream header and block
    /// header through it.
    ///
    /// **Parsing eagerly is what keeps `new` fallible**, which is the seam's
    /// contract: `xz4rust` learns a chain only by parsing a header, so an arm
    /// that deferred would have to move [`SeamError::UnsupportedFilter`] and
    /// [`SeamError::UnusableChain`] to the first `decode` call — changing the
    /// seam to suit one crate. It also satisfies `NeedsLargerInputBuffer` by
    /// construction: the structural bytes are offered contiguously and are never
    /// split across a caller's chunk.
    fn new(
        filters: &[Filter],
        payload_size: u64,
        uncompressed_size: u64,
    ) -> Result<Xz4rust, SeamError> {
        // Before a header is emitted, so that the id reaches the caller. See
        // the module docs, "Buildability is declared here".
        for f in filters {
            // Id by id rather than by range, so that this list and the
            // `liblzma` arm's are the same list read twice — which is what
            // "both arms answer the same set" has to mean if it is to be
            // checkable by eye.
            if !matches!(
                f.id,
                FILTER_DELTA
                    | FILTER_X86
                    | FILTER_POWERPC
                    | FILTER_IA64
                    | FILTER_ARM
                    | FILTER_ARMTHUMB
                    | FILTER_SPARC
                    | FILTER_ARM64
                    | FILTER_RISCV
                    | FILTER_LZMA2
            ) {
                return Err(SeamError::UnsupportedFilter(f.id));
            }
        }

        // The header-derived dictionary as both the initial and the maximum
        // allocation: `crate::decode` has already refused anything over the
        // reader's memory limit, so the backend can never allocate beyond what
        // this crate approved, and `XzError::DictionaryTooLarge` is unreachable
        // because `needed_size > max` cannot hold.
        //
        // `xz4rust` clamps both to its own 3 GiB `DICT_SIZE_MAX`, one property
        // byte below what `liblzma` admits — deficiency: KD4, whose detail is in
        // `docs/design/architecture.md`.
        let dict = dict_size(filters);
        #[cfg(not(xz_seek_decoder_reuse))]
        let decoder = xz4rust::XzDecoder::in_heap_with_alloc_dict_size(dict, dict);
        // The measurement build takes the thread's spare when it was built for
        // this dictionary and `reset()`s it, which skips the allocation.
        #[cfg(xz_seek_decoder_reuse)]
        let decoder = Some(take_spare(dict));

        let header = crate::block::encode(filters, Some(payload_size), Some(uncompressed_size));
        // `check=None`, so the check field contributes nothing to Unpadded Size
        // — and this is the *synthesized* block's, which is generally longer
        // than the one in the file because it declares both sizes.
        let unpadded_size = header.len() as u64 + payload_size;

        let mut prefix = Vec::with_capacity(12 + header.len());
        prefix.extend_from_slice(&stream_header());
        prefix.extend_from_slice(&header);

        let mut arm = Xz4rust {
            decoder,
            #[cfg(xz_seek_decoder_reuse)]
            dict,
            trailer: trailer(unpadded_size, uncompressed_size),
            trailer_at: 0,
            payload_size,
            uncompressed_size,
            in_used: 0,
            out_written: 0,
            finished: false,
        };
        arm.push_prefix(&prefix)?;
        Ok(arm)
    }

    /// Push the stream header and block header, which produce no output.
    ///
    /// A failure here is the chain's — those are the only bytes in the prefix
    /// the file supplied — so it goes through the ordinary mapping rather than
    /// the bug class.
    fn push_prefix(&mut self, prefix: &[u8]) -> Result<(), SeamError> {
        let mut at = 0;
        let mut sink = [0u8; 1];
        while at < prefix.len() {
            let r = decoder_mut(&mut self.decoder)
                .decode(&prefix[at..], &mut sink)
                .map_err(|e| seam_error(&e, Phase::Chain))?;
            if !r.made_progress() || r.output_produced() != 0 || r.is_end_of_stream() {
                return Err(our_bug_at("the synthesized prefix did not parse cleanly"));
            }
            at += r.input_consumed();
        }
        Ok(())
    }

    fn decode(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, SeamError> {
        if self.finished {
            return Ok(Progress {
                in_used: 0,
                out_written: 0,
                finished: true,
            });
        }
        if input.is_empty() {
            if self.in_used < self.payload_size {
                // The payload is not spent, so this is a caller with nothing in
                // hand rather than the end of the block — the same nothing the
                // `liblzma` arm reports. Forwarding it would be
                // `NeedsLargerInputBuffer` (`I16`).
                return Ok(Progress {
                    in_used: 0,
                    out_written: 0,
                    finished: false,
                });
            }
            return self.push_trailer(output);
        }

        let r = decoder_mut(&mut self.decoder)
            .decode(input, output)
            .map_err(|e| seam_error(&e, Phase::Payload))?;
        if r.is_end_of_stream() {
            // Only the payload has been offered, so a stream that ended inside
            // it means the trailer this crate wrote is not where it belongs.
            return Err(our_bug_at("the stream ended inside the payload"));
        }
        self.in_used += r.input_consumed() as u64;
        self.out_written += r.output_produced() as u64;
        Ok(Progress {
            in_used: r.input_consumed(),
            out_written: r.output_produced(),
            finished: false,
        })
    }

    /// The payload is spent: push the block padding, index and footer, collect
    /// whatever the chain still owes the block, and end on `EndOfStream`.
    ///
    /// **The push is also the block's drain, so it is resumable.** A BCJ filter
    /// holds a bounded tail back and releases it only to a call carrying output
    /// room (`I18`), and the trailer's bytes are the only input left to push
    /// that release along (`I16`). So a push can exhaust the caller's output
    /// buffer mid-trailer, which is not a failure: it returns
    /// `finished: false`, and [`Xz4rust::trailer_at`] is where the next call
    /// picks it up. `decode.rs`'s loop re-enters with an empty input slice for
    /// as long as the block still owes bytes, and offers no room at all once it
    /// does not — by which point nothing is held back, so the last stretch of
    /// trailer needs none.
    ///
    /// **This is where the bug discriminator lives.** Past
    /// `in_used == payload_size && out_written == uncompressed_size` the payload
    /// is spent *and* the block is complete, so nothing that fails afterwards
    /// can be the file's fault — the remaining bytes are ones this crate wrote.
    /// Short of it the block did not finish, and a decoder that refuses the
    /// padding behind a payload which never terminated is refusing the file.
    /// `complete` is therefore recomputed as the drain goes, since the bytes
    /// this call is releasing are what finish the block.
    ///
    /// **The discriminator is positional and it is total**, which is why short
    /// of it the variant is not consulted at all. A payload that never
    /// terminates leaves the decoder still counting the trailer's bytes as
    /// block body, so it answers `MoreDataInBlockBodyThanHeaderIndicated` — a
    /// variant [`fault`] calls ours, because in the payload phase only a broken
    /// clamp reaches it. Routing this branch through [`seam_error`] would let
    /// that classification override the position and fire the assertion on a
    /// corrupt file, which is the failure the discriminator exists to prevent.
    ///
    /// The no-progress branch is where the two states the position alone cannot
    /// separate are told apart: a block still owing bytes with the output spent
    /// is the drain waiting for room, and a block still owing bytes with room
    /// left over is a payload that never terminated.
    fn push_trailer(&mut self, output: &mut [u8]) -> Result<Progress, SeamError> {
        let mut written = 0usize;
        while !self.finished {
            let complete = self.out_written + written as u64 == self.uncompressed_size;
            if self.trailer_at == self.trailer.len() {
                return Err(if complete {
                    our_bug_at("the trailer ran out before the stream ended")
                } else {
                    SeamError::Data
                });
            }
            let r = match decoder_mut(&mut self.decoder)
                .decode(&self.trailer[self.trailer_at..], &mut output[written..])
            {
                Ok(r) => r,
                // Short of the discriminator this is the payload's failure to
                // terminate, which is the file's; past it, ours.
                Err(e) if complete => return Err(our_bug(&e)),
                Err(_) => return Err(SeamError::Data),
            };
            self.trailer_at += r.input_consumed();
            written += r.output_produced();
            if r.is_end_of_stream() {
                self.finished = true;
                break;
            }
            if !r.made_progress() {
                if !complete && written == output.len() {
                    // The drain filled the caller's buffer and the block is
                    // still owed bytes. Hand back what came out; the trailer
                    // resumes from `trailer_at` on the next call.
                    break;
                }
                return Err(if complete {
                    our_bug_at("the trailer made no progress")
                } else {
                    SeamError::Data
                });
            }
        }
        // Everything the trailer produces is the block's held-back tail, so it
        // can never exceed what the block still owed. The assertion that the
        // trailer produces *nothing* was only ever true of a chain with no BCJ
        // filter in it.
        debug_assert!(
            self.out_written + written as u64 <= self.uncompressed_size,
            "the trailer produced more than the block still owed"
        );
        self.out_written += written as u64;
        Ok(Progress {
            in_used: 0,
            out_written: written,
            finished: self.finished,
        })
    }

    fn input_consumed(&self) -> u64 {
        self.in_used
    }
}

/// The dictionary the chain declares, as `xz4rust` wants it.
///
/// One derivation, [`crate::block::lzma2_dict_size`]'s, shared with the header
/// parse that enforces the memory limit. A chain naming no LZMA2 filter is not
/// one the format permits and the decoder will say so when it reads the header;
/// the floor keeps the constructor from being asked for less than the crate's
/// own minimum in the meantime.
#[cfg(feature = "xz4rust")]
fn dict_size(filters: &[Filter]) -> usize {
    filters
        .iter()
        .find(|f| f.id == FILTER_LZMA2)
        .and_then(|f| f.props.first().copied())
        .map_or(xz4rust::DICT_SIZE_MIN, |code| {
            usize::try_from(crate::block::lzma2_dict_size(code)).unwrap_or(usize::MAX)
        })
        .max(xz4rust::DICT_SIZE_MIN)
}

/// The twelve-byte stream header of the synthesized stream.
#[cfg(feature = "xz4rust")]
fn stream_header() -> [u8; 12] {
    let mut h = [0u8; 12];
    h[..6].copy_from_slice(b"\xfd7zXZ\x00");
    h[6..8].copy_from_slice(&STREAM_FLAGS);
    h[8..12].copy_from_slice(&crate::check::crc32(&STREAM_FLAGS).to_le_bytes());
    h
}

/// Block padding, a one-record index and the stream footer.
///
/// `unpadded_size` is the **synthesized** block's — the re-emitted header plus
/// the payload, with nothing for the check because the stream declares
/// `check=None`. Handing this the file's own header size is a mistake that
/// surfaces as `XzError::CorruptedData` out of the trailer, a phase away from
/// the fault.
#[cfg(feature = "xz4rust")]
fn trailer(unpadded_size: u64, uncompressed_size: u64) -> Vec<u8> {
    // Block padding, to a multiple of four counted over the whole block.
    let mut out = vec![0u8; ((4 - unpadded_size % 4) % 4) as usize];

    // The index: the indicator, one record, that record's two sizes, null
    // padding to a multiple of four, and a CRC32 over all of it.
    let mut index = vec![0u8];
    crate::block::push_vli(&mut index, 1);
    crate::block::push_vli(&mut index, unpadded_size);
    crate::block::push_vli(&mut index, uncompressed_size);
    while !index.len().is_multiple_of(4) {
        index.push(0);
    }
    let index_size = index.len() + 4;
    index.extend_from_slice(&crate::check::crc32(&index).to_le_bytes());
    out.extend_from_slice(&index);

    // The footer: a CRC32 over the six bytes that follow it, then the backward
    // size, the same stream flags, and the magic.
    let mut body = Vec::with_capacity(6);
    body.extend_from_slice(&((index_size / 4 - 1) as u32).to_le_bytes());
    body.extend_from_slice(&STREAM_FLAGS);
    out.extend_from_slice(&crate::check::crc32(&body).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(b"YZ");
    out
}

/// Where in the synthesized stream a failure arrived.
///
/// **The variant alone does not say whose bytes are at fault, and this is what
/// resolves it.** One `xz4rust` decoder parses both the block header this crate
/// wrote and the payload the file supplied, and the variants that *read* as
/// chain faults are raised from the payload: the whole `LzmaProperties*` family
/// comes from inside an LZMA2 chunk header, while the block header's own
/// properties check raises `UnsupportedLzmaProperties`. So the arm records which
/// bytes it is offering rather than trying to read that off the error.
#[cfg(feature = "xz4rust")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The synthesized stream header and block header. The only thing in them
    /// the file supplied is the chain, so a fault the decoder is entitled to
    /// find there is the chain's.
    Chain,
    /// The payload, and the trailer behind a payload that never terminated.
    Payload,
}

/// Whose bytes an `XzError` variant can be about.
///
/// Written out variant by variant rather than by class. `XzError` is
/// `#[non_exhaustive]`, so a wildcard arm is mandatory whatever we do; naming
/// every variant that exists under this crate's feature configuration is what
/// keeps that arm reachable only from a future `xz4rust` release rather than
/// from a variant nobody read.
#[cfg(feature = "xz4rust")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The file's — the chain the block header named, or the payload behind it.
    /// [`Phase`] says which.
    Theirs,
    /// This crate's. The synthesized stream header, the block header's framing,
    /// the index and the footer are bytes `xz-seek` wrote, so a decoder that
    /// refuses them is refusing us; so is a decoder-state or check-type
    /// complaint, since the synthesized stream declares `check=None` and is
    /// never reset mid-block; and so is a complaint that the block body
    /// overruns the sizes that header declared, which only a broken clamp in
    /// [`crate::decode`] can produce.
    Ours,
}

/// `XzError`, placed in the taxonomy.
#[cfg(feature = "xz4rust")]
fn seam_error(e: &xz4rust::XzError, phase: Phase) -> SeamError {
    match (fault(e), phase) {
        // A chain the format does not permit, or properties a filter refuses —
        // and, where the block header's *framing* is what the decoder refused,
        // this crate's own bug reported as the file's — deficiency: KD7, whose
        // detail is in `docs/design/architecture.md`.
        //
        // Never [`SeamError::UnsupportedFilter`]: the arm's own allowlist has
        // already established that every id in the chain is one this crate
        // builds, and the variants that survive it — `UnsupportedBcjFilter` for
        // a non-prefilter in a non-final position, `UnsupportedBlockHeaderOption`
        // for a chain that does not end in LZMA2 — carry nothing that could
        // name a filter anyway.
        (Fault::Theirs, Phase::Chain) => SeamError::UnusableChain,
        (Fault::Theirs, Phase::Payload) => SeamError::Data,
        (Fault::Ours, _) => our_bug(e),
    }
}

#[cfg(feature = "xz4rust")]
fn fault(e: &xz4rust::XzError) -> Fault {
    use xz4rust::XzError as X;
    match e {
        X::UnsupportedBcjFilter(_)
        | X::UnsupportedBlockHeaderOption
        | X::LzmaPropertiesInvalid
        | X::LzmaPropertiesTooLarge
        | X::LzmaPropertiesMissing
        | X::UnsupportedLzmaProperties(_)
        | X::DictionaryTooLarge(_)
        | X::CorruptedData
        | X::CorruptedDataInLzma
        | X::DictionaryOverflow
        | X::LzmaDictionaryResetExcepted
        | X::LessDataInBlockBodyThanHeaderIndicated => Fault::Theirs,

        // **`MoreDataInBlockBodyThanHeaderIndicated` is ours, not the file's**,
        // and it is the one member of this list no input can reach.
        // `xz4rust` raises it when the decoder consumes more compressed bytes,
        // or produces more uncompressed ones, than the synthesized header
        // declared — and [`crate::decode`] clamps both before the seam is
        // offered anything, so a file cannot provoke it. If it arrives, one of
        // those clamps has broken. Its reachable sibling above stays theirs.
        X::MoreDataInBlockBodyThanHeaderIndicated
        | X::NeedsReset
        | X::NeedsLargerInputBuffer
        | X::CorruptedDataInBlockIndex
        | X::BlockHeaderTooSmall
        | X::CorruptedCompressedLengthVliInBlockHeader
        | X::CorruptedUncompressedLengthVliInBlockHeader
        | X::UnsupportedStreamHeaderOption
        | X::Crc64NotSupported
        | X::Sha256NotSupported
        | X::UnsupportedCheckType(_)
        | X::ContentCrc32Mismatch(..)
        | X::IndexCrc32Mismatch(..)
        | X::StreamHeaderMagicNumberMismatch
        | X::StreamHeaderCrc32Mismatch(..)
        | X::BlockHeaderCrc32Mismatch(..)
        | X::FooterMagicNumberMismatch
        | X::FooterCheckTypeMismatch(..)
        | X::FooterCrc32Mismatch(..)
        | X::FooterDecoderIndexMismatch(..) => Fault::Ours,

        // Deprecated upstream and documented as never returned.
        #[allow(deprecated)]
        X::BcjFilterWithOffsetNotSupported => Fault::Ours,

        // The `#[non_exhaustive]` arm: a variant a later `xz4rust` added, which
        // nobody here has read. Calling it ours is what makes it loud.
        _ => Fault::Ours,
    }
}

/// A failure that can only be about bytes this crate wrote.
///
/// **Loud in every `cargo test` and every debug build**, which is where a bug in
/// an encoder this crate owns will actually be found, and a plain
/// [`SeamError::Data`] in a release build: a released binary handed a hostile
/// file must not abort on a decoder's error variant, and one wrong error name is
/// the smaller failure. It is not a taxonomy variant of its own either — that
/// would spend an entry on a case that is a bug by construction.
#[cfg(feature = "xz4rust")]
#[allow(clippy::assertions_on_constants)]
fn our_bug(e: &xz4rust::XzError) -> SeamError {
    debug_assert!(
        false,
        "xz4rust refused bytes xz-seek wrote, or returned a variant \
         src/backend.rs does not map: {e}"
    );
    SeamError::Data
}

/// The same, for a condition rather than an error variant.
#[cfg(feature = "xz4rust")]
#[allow(clippy::assertions_on_constants)]
fn our_bug_at(what: &str) -> SeamError {
    debug_assert!(false, "the synthesized stream is wrong: {what}");
    SeamError::Data
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam is a `Send` value under every compiled backend.
    ///
    /// Parallel block decode has to decide what crosses a thread boundary, and
    /// the answer it inherits is that a `BlockDecode` is a self-contained owned
    /// value whose only question mark was the backend's decoder — see
    /// `docs/design/architecture.md`, "The block-payload seam". `liblzma`'s `Stream`
    /// declares the impls; `xz4rust`'s decoder owns its dictionary and borrows
    /// nothing, so it holds by inference and this is what notices if that stops
    /// being true.
    ///
    /// A test rather than a bound: nothing in this crate requires `Send` today,
    /// and asserting it in the type system would be that phase's decision made
    /// early.
    #[test]
    fn the_seam_is_send_under_every_compiled_backend() {
        fn assert_send<T: Send>() {}
        assert_send::<PayloadDecoder>();
        assert_send::<crate::decode::BlockDecode>();
    }

    fn filter(id: u64, props: &[u8]) -> Filter {
        Filter {
            id,
            props: props.to_vec(),
        }
    }

    /// `--lzma2=dict=4MiB`, the chain almost every fixture block carries.
    fn lzma2() -> Vec<Filter> {
        vec![filter(FILTER_LZMA2, &[20])]
    }

    /// A decoder over a chain, with sizes that describe no real block.
    ///
    /// Every test below is about what the seam *says*, not about bytes, so any
    /// pair of sizes will do — the `xz4rust` arm only needs them to be the ones
    /// its synthesized header declares.
    fn decoder(backend: Backend, filters: &[Filter]) -> Result<PayloadDecoder, SeamError> {
        PayloadDecoder::new(backend, filters, 4096, 65536)
    }

    /// Every id the format assigns to a block chain builds here, under every
    /// backend, so the parse never has to hold an opinion about which ones do.
    ///
    /// The BCJ filters are asked for with no properties, which is how `xz`
    /// writes them when the start offset is zero, and delta with the one byte
    /// it always has.
    #[test]
    fn every_filter_the_format_assigns_can_be_built() {
        for &backend in COMPILED {
            for id in [
                FILTER_X86,
                FILTER_POWERPC,
                FILTER_IA64,
                FILTER_ARM,
                FILTER_ARMTHUMB,
                FILTER_SPARC,
                FILTER_ARM64,
                FILTER_RISCV,
            ] {
                let chain = vec![filter(id, &[]), filter(FILTER_LZMA2, &[20])];
                assert!(
                    decoder(backend, &chain).is_ok(),
                    "{backend:?}: filter id {id:#x} did not build"
                );
            }
            let chain = vec![filter(FILTER_DELTA, &[3]), filter(FILTER_LZMA2, &[20])];
            assert!(decoder(backend, &chain).is_ok(), "{backend:?}: delta");
            assert!(decoder(backend, &lzma2()).is_ok(), "{backend:?}: lzma2");
        }
    }

    /// An id the backend does not build is *named*, not called corruption. The
    /// header was well-formed and its CRC32 matched; the file is asking for
    /// something we do not have.
    ///
    /// Buildability lives here rather than in the header parse: it is a
    /// statement about a backend, and both arms are held to the same set — which
    /// is what keeps the id in the error rather than losing it to a decoder
    /// variant that carries none.
    #[test]
    fn an_id_this_backend_does_not_build_is_named_rather_than_called_corrupt() {
        // LZMA1's id, which a block chain may never name; an unassigned id
        // below the reserved range; and the reserved range itself.
        for &backend in COMPILED {
            for id in [0x02u64, 0x0c, 0x20, 0x22, 0x4000_0000_0000_0001] {
                let chain = vec![filter(id, &[]), filter(FILTER_LZMA2, &[20])];
                assert_eq!(
                    decoder(backend, &chain).err(),
                    Some(SeamError::UnsupportedFilter(id)),
                    "{backend:?}: filter id {id:#x}"
                );
            }
        }
    }

    /// A chain the format does not permit is the backend's to refuse, so that
    /// this crate does not carry a second, weaker copy of the rule.
    #[test]
    fn a_chain_that_does_not_end_in_lzma2_is_refused_by_the_backend() {
        for &backend in COMPILED {
            // BCJ alone: nothing in the chain produces the compressed bytes.
            let chain = vec![filter(FILTER_X86, &[])];
            assert_eq!(
                decoder(backend, &chain).err(),
                Some(SeamError::UnusableChain),
                "{backend:?}: bcj alone"
            );

            // LZMA2 ahead of a filter that is not size-preserving in that
            // position.
            let chain = vec![filter(FILTER_LZMA2, &[20]), filter(FILTER_X86, &[])];
            assert_eq!(
                decoder(backend, &chain).err(),
                Some(SeamError::UnusableChain),
                "{backend:?}: lzma2 first"
            );
        }
    }

    /// A BCJ start offset that is not a multiple of the filter's alignment:
    /// well-formed bytes of the right length, and options `liblzma` itself
    /// rejects.
    ///
    /// **`xz4rust` does not check it** — deficiency: KD5, whose detail is in
    /// `docs/design/architecture.md`. `xz` refuses such a file, so this is a
    /// chain on which the pure-Rust backend and the oracle disagree; no fixture
    /// carries one, because `xz` will not write one.
    ///
    /// Pinned in both directions so that an `xz4rust` release which adds the
    /// check fails here rather than silently closing an entry nobody struck.
    #[test]
    fn a_misaligned_bcj_start_offset_is_refused_only_by_liblzma() {
        let chain = vec![
            filter(FILTER_ARM, &[1, 0, 0, 0]),
            filter(FILTER_LZMA2, &[20]),
        ];
        #[cfg(feature = "liblzma")]
        assert_eq!(
            decoder(Backend::Liblzma, &chain).err(),
            Some(SeamError::UnusableChain)
        );
        #[cfg(feature = "xz4rust")]
        assert!(
            decoder(Backend::Xz4rust, &chain).is_ok(),
            "KD5: xz4rust has started checking BCJ start-offset alignment"
        );
        let _ = &chain;
    }

    /// Progress is reported per call, and the count behind it survives a
    /// failure — which is the property `BlockDataError`'s offset rests on.
    #[test]
    fn a_payload_that_is_not_lzma2_at_all_fails_with_its_consumption_recorded() {
        for &backend in COMPILED {
            let mut d = decoder(backend, &lzma2()).unwrap();
            let mut out = [0u8; 64];
            let e = d.decode(&[0xff; 16], &mut out).unwrap_err();
            assert_eq!(e, SeamError::Data, "{backend:?}");
            // Whatever it read before refusing is counted, and it stopped inside
            // what it was given rather than past it.
            assert!(
                d.input_consumed() <= 16,
                "{backend:?}: {}",
                d.input_consumed()
            );
        }
    }

    /// **Both arms must pass this**, and it is what keeps a third backend from
    /// rediscovering the empty-slice question by failing the sweep.
    ///
    /// On a live decoder whose payload is not yet spent, an empty input slice
    /// makes no progress and is not finished — which is exactly the signal
    /// [`crate::decode`]'s loop concludes from, with no status consulted.
    /// `xz4rust` answers the same call `Err(NeedsLargerInputBuffer)` (`I16`), so
    /// the arm absorbs it rather than the seam's contract moving.
    #[test]
    fn an_exhausted_payload_reports_no_progress_rather_than_an_error() {
        for &backend in COMPILED {
            let mut d = decoder(backend, &lzma2()).unwrap();
            let mut out = [0u8; 64];
            assert_eq!(
                d.decode(&[], &mut out).unwrap(),
                Progress {
                    in_used: 0,
                    out_written: 0,
                    finished: false
                },
                "{backend:?}: empty input, room to write"
            );

            let mut d = decoder(backend, &lzma2()).unwrap();
            assert_eq!(
                d.decode(&[], &mut []).unwrap(),
                Progress {
                    in_used: 0,
                    out_written: 0,
                    finished: false
                },
                "{backend:?}: both slices empty"
            );
        }
    }

    /// `I16` and `I17`, asserted against `xz4rust` itself.
    ///
    /// The absorption above rests on what the *decoder* answers, so a test
    /// through the seam would pass whatever `xz4rust` did — it is the arm's
    /// answer either way. This reaches past the arm and asks the crate, over a
    /// real fixture block with a quarter of its payload in, which is the
    /// position the arm's own confirmation was measured at.
    #[cfg(feature = "xz4rust")]
    #[test]
    fn xz4rust_refuses_an_empty_input_slice_and_tolerates_an_empty_output_one() {
        use crate::table::SeekTable;

        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let bytes = std::fs::read(dir.join("many-blocks.xz")).expect("the fixture reads");
        let source: &[u8] = &bytes;
        let table = SeekTable::from_source(&source).expect("the fixture walks");
        let block = table.blocks[0];
        let header = crate::block::read_header(&source, &block, table.streams[0].check)
            .expect("the header parses");
        let payload = &bytes[(block.compressed_offset + header.header_size) as usize
            ..(block.compressed_offset + header.header_size + header.payload_size) as usize];

        let live = |bytes_in: usize| {
            let mut arm = Xz4rust::new(
                &header.filters,
                header.payload_size,
                block.uncompressed_size,
            )
            .expect("the chain builds");
            let mut scratch = vec![0u8; 64 * 1024];
            let mut at = 0;
            while at < bytes_in {
                at += super::decoder_mut(&mut arm.decoder)
                    .decode(&payload[at..bytes_in], &mut scratch)
                    .expect("the payload decodes")
                    .input_consumed();
            }
            arm
        };

        let quarter = payload.len() / 4;
        let mut scratch = vec![0u8; 64 * 1024];

        // `I16`: an empty input slice is an error whatever the output slice is,
        // because `XzInnerDecoder::decode` refuses one before it looks at
        // anything else.
        let mut arm = live(quarter);
        assert!(matches!(
            super::decoder_mut(&mut arm.decoder).decode(&[], &mut scratch),
            Err(xz4rust::XzError::NeedsLargerInputBuffer)
        ));
        let mut arm = live(quarter);
        assert!(matches!(
            super::decoder_mut(&mut arm.decoder).decode(&[], &mut []),
            Err(xz4rust::XzError::NeedsLargerInputBuffer)
        ));

        // `I17`: an empty *output* slice is not. Mid-block it makes no progress
        // rather than the input progress a first reading might expect, which is
        // why the arm passes it through instead of intercepting it.
        let mut arm = live(quarter);
        let r = super::decoder_mut(&mut arm.decoder)
            .decode(&payload[quarter..], &mut [])
            .expect("an empty output slice is not an error");
        assert!(!r.is_end_of_stream());
        assert_eq!((r.input_consumed(), r.output_produced()), (0, 0));
    }

    /// **`MoreDataInBlockBodyThanHeaderIndicated` is unreachable from a file**,
    /// which is what puts it in [`Fault::Ours`].
    ///
    /// The variant fires on `block.compressed > header.compressed ||
    /// block.uncompressed > header.uncompressed`, and the synthesized header's
    /// two sizes are the index's. [`crate::decode`] clamps the seam against both
    /// before it is offered anything — `room` caps the output at
    /// `uncompressed_size - out_written`, and `fill`'s `input.truncate` means no
    /// byte past `payload_size` is ever passed in — so no file provokes it, and a
    /// decoder that raises it has caught one of those clamps breaking.
    ///
    /// This drives a real block's payload against a header that undercuts each
    /// size in turn, which is what a broken clamp looks like from the decoder's
    /// side, and asserts that the honest declaration reaches neither. It reads
    /// the raw `XzError` off `xz4rust` rather than going through
    /// [`Xz4rust::decode`], because the arm's answer to this variant is now a
    /// `debug_assert` and a test through the seam would only observe the panic.
    #[cfg(feature = "xz4rust")]
    #[test]
    fn only_an_undercut_declared_size_reaches_more_data_than_the_header_indicated() {
        use crate::table::SeekTable;

        let dir = fixtures_gen::ensure_corpus().expect("the corpus builds");
        let bytes = std::fs::read(dir.join("many-blocks.xz")).expect("the fixture reads");
        let source: &[u8] = &bytes;
        let table = SeekTable::from_source(&source).expect("the fixture walks");
        let block = table.blocks[0];
        let header = crate::block::read_header(&source, &block, table.streams[0].check)
            .expect("the header parses");
        let payload = &bytes[(block.compressed_offset + header.header_size) as usize
            ..(block.compressed_offset + header.header_size + header.payload_size) as usize];

        // Offer the whole payload against a synthesized header declaring the
        // given sizes, and report the first refusal.
        let drive = |compressed: u64, uncompressed: u64| -> Option<xz4rust::XzError> {
            let mut arm =
                Xz4rust::new(&header.filters, compressed, uncompressed).expect("the chain builds");
            let mut scratch = vec![0u8; 64 * 1024];
            let mut at = 0;
            while at < payload.len() {
                match super::decoder_mut(&mut arm.decoder).decode(&payload[at..], &mut scratch) {
                    Ok(r) => {
                        assert!(r.made_progress(), "the payload stalled at {at}");
                        at += r.input_consumed();
                    }
                    Err(e) => return Some(e),
                }
            }
            None
        };

        let honest = (header.payload_size, block.uncompressed_size);
        assert!(
            drive(honest.0, honest.1).is_none(),
            "the block's own sizes refuse its own payload"
        );

        for (compressed, uncompressed, clamp) in [
            (honest.0 - 1, honest.1, "the input clamp"),
            (honest.0, honest.1 - 1, "the output clamp"),
        ] {
            let e = drive(compressed, uncompressed)
                .unwrap_or_else(|| panic!("{clamp}: undercutting the declaration was tolerated"));
            assert!(
                matches!(e, xz4rust::XzError::MoreDataInBlockBodyThanHeaderIndicated),
                "{clamp}: {e}"
            );
            assert_eq!(fault(&e), Fault::Ours, "{clamp}");
        }
    }
}
