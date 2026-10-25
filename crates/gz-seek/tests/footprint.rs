//! **What one decode holds is under [`gz_seek::DECODER_BYTES`]**, counted on
//! the engine whose allocations the Rust allocator sees — and a forward pass,
//! a static block's walk included, under that and the window it copies out.
//!
//! A test binary of its own because it installs a counting global allocator:
//! zlib-rs, built with its default `rust-allocator`, takes its inflate state
//! and window from Rust's allocator, so every byte one decode holds passes
//! through [`Counting`]. The C engines allocate through `calloc`, which this
//! cannot see, so the count is asserted on zlib-rs alone.
#![cfg(feature = "zlib-rs")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};

use gz_seek::{DECODER_BYTES, Forward, Start, WINDOW_SIZE};

/// The system allocator, counting what is live and the most that has been.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded to `System` unchanged; the counters are
// bookkeeping beside it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as `System::alloc`, whose contract the caller meets.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as `System::dealloc`, whose contract the caller meets.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Text shaped like a `COPY` block's rows, long enough for several units.
fn payload() -> Vec<u8> {
    (0..120_000u32)
        .flat_map(|i| {
            format!("{i}\tvalue {}\t{}\n", i.wrapping_mul(7919) % 10_007, i % 97).into_bytes()
        })
        .collect()
}

#[test]
fn one_decode_holds_no_more_than_the_stated_charge() {
    let plaintext = payload();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&plaintext).unwrap();
    let file = encoder.finish().unwrap();

    // The first pass, every window kept, so a unit inside the member can be
    // resumed with its history.
    let mut pass = Forward::new(NonZeroU64::new(256 * 1024).unwrap()).unwrap();
    let (mut taken, mut out, mut windows) = (0, vec![0u8; 64 * 1024], Vec::new());
    loop {
        let step = pass.read(&file[taken..], &mut out).unwrap();
        taken += step.progress.consumed;
        if step.checkpoint.is_some() {
            windows.push(pass.window().to_vec());
        }
        if step.progress == Default::default() && step.checkpoint.is_none() {
            break;
        }
    }
    let index = pass.finish().unwrap();
    let unit = (0..index.unit_count())
        .map(|i| index.unit(i).unwrap())
        .find(|task| matches!(task.checkpoint().start, Start::Block { .. }))
        .expect("a unit resumed inside the member, its writer ending blocks");
    let range = unit.compressed_range();
    let compressed = &file[range.start as usize..range.end as usize];
    let plain = unit.plaintext_range();
    let mut decoded = vec![0u8; (plain.end - plain.start) as usize];
    let window = &windows[unit.unit()];

    // Everything the caller hands the decode is held before the count starts.
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    unit.decode(window, compressed, &mut decoded).unwrap();
    let held = PEAK.load(Ordering::SeqCst) - before;

    assert_eq!(decoded, plaintext[plain.start as usize..plain.end as usize]);
    assert!(held > WINDOW_SIZE, "the count sees the engine's own window: {held} bytes");
    assert!(held as u64 <= DECODER_BYTES, "one decode held {held} bytes");
}

/// **A forward pass holds its decode and the window it copies out**, a
/// static block's walk among what the decode holds: over a member level 1
/// writes as one static block, walked to every mark.
#[test]
fn a_forward_pass_walking_a_static_block_holds_no_more_than_its_charge() {
    let plaintext = payload();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
    encoder.write_all(&plaintext).unwrap();
    let file = encoder.finish().unwrap();
    let mut out = vec![0u8; 64 * 1024];

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let mut pass = Forward::new(NonZeroU64::new(256 * 1024).unwrap()).unwrap();
    let (mut taken, mut symbols) = (0, 0);
    loop {
        let step = pass.read(&file[taken..], &mut out).unwrap();
        taken += step.progress.consumed;
        symbols +=
            usize::from(matches!(step.checkpoint.map(|c| c.start), Some(Start::Symbol { .. })));
        if step.progress == Default::default() && step.checkpoint.is_none() {
            break;
        }
    }
    let held = PEAK.load(Ordering::SeqCst) - before;
    drop(pass);

    assert!(symbols > 2, "the member's one static block checkpointed inside: {symbols}");
    let charge = DECODER_BYTES + WINDOW_SIZE as u64;
    assert!(held as u64 <= charge, "a forward pass held {held} bytes against {charge}");
}
