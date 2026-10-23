//! A gzip file's index built by `gz_seek`'s forward pass over the file held
//! in memory, its windows held beside it: what a test reads a gzip file
//! through, in this crate and above it under `test-support`, at a spacing it
//! states where a first pass places checkpoints at the shipped one alone.

use std::num::NonZeroU64;
use std::sync::Arc;

use super::checkpoints::{GzipIndex, WindowProvider};

/// Each checkpoint's window, held in memory: a test's provider, standing in
/// for the cache's window section.
pub(crate) struct HeldWindows(pub(crate) Vec<Vec<u8>>);

impl WindowProvider for HeldWindows {
    fn read_window(&self, checkpoint: usize, out: &mut [u8]) -> std::io::Result<()> {
        out.copy_from_slice(&self.0[checkpoint]);
        Ok(())
    }
}

/// The complete index of the gzip file `file` holds, checkpoints `spacing`
/// plaintext bytes apart, and every checkpoint's window, as a forward pass
/// over it hands them out — built by `gz_seek` directly.
///
/// # Panics
///
/// Where `file` is not a whole gzip file, or `spacing` is zero: a test's
/// fixture, not a reader's input.
pub(crate) fn index_and_windows(file: &[u8], spacing: u64) -> (gz_seek::Index, Vec<Vec<u8>>) {
    let spacing = NonZeroU64::new(spacing).expect("a nonzero spacing");
    let mut pass = gz_seek::Forward::new(spacing).expect("a forward pass begins");
    let (mut out, mut windows) = (vec![0u8; 16 * 1024], Vec::new());
    loop {
        let at = pass.compressed_position() as usize;
        let step = pass.read(&file[at.min(file.len())..], &mut out).expect("a whole gzip file");
        if step.checkpoint.is_some() {
            windows.push(pass.window().to_vec());
        }
        if step.progress == Default::default() && step.checkpoint.is_none() {
            break;
        }
    }
    (pass.finish().expect("a whole gzip file"), windows)
}

/// [`index_and_windows`] as this crate hands an index out: its windows read
/// through a provider holding them.
pub fn indexed_in_memory(file: &[u8], spacing: u64) -> GzipIndex {
    let (index, windows) = index_and_windows(file, spacing);
    GzipIndex { index, windows: Some(Arc::new(HeldWindows(windows))) }
}
