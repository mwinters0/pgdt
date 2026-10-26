//! A gzip cache's window section: every checkpoint's window, after the
//! encoded file, in checkpoint order — written by a save out of the provider
//! the saved index carries, and read back positionally by the provider a
//! load attaches, as each unit's decode begins
//! (`docs/design/decisions.md`, "D114").
//!
//! **The section has no table of its own**: a checkpoint's window lengths are
//! the index's, so where each one sits is their running sum from where the
//! encoded file ends, and the file is whole only where it ends with the last.

use std::fs::File;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::sync::Arc;

use pgdt_core::{Result, Unusable};
use pgdt_io::{CompressionIndex, GzipIndex, WindowProvider};

/// Write `compression`'s window section — each checkpoint's window, read
/// through the provider its index carries, in checkpoint order — or nothing,
/// for a source with no gzip layer.
///
/// **A save whose index was loaded from the cache it replaces reads the
/// previous file's windows**, through the handle its provider holds, which a
/// rename over that file's name leaves open on the same bytes.
pub(super) fn write_section(
    writer: &mut impl Write,
    compression: Option<&CompressionIndex>,
) -> Result<()> {
    let Some(CompressionIndex::Gzip(index)) = compression else {
        return Ok(());
    };
    let mut window = Vec::new();
    for checkpoint in 0..index.checkpoint_count() {
        window.resize(index.window_len(checkpoint).expect("a counted checkpoint"), 0);
        index.read_window(checkpoint, &mut window)?;
        writer.write_all(&window)?;
    }
    Ok(())
}

/// `compression` as a load hands it out: a gzip index with the provider
/// reading its windows out of `file`'s section, which begins at `base`, the
/// encoded file's end — or [`Unusable::Unreadable`] where the file does not
/// end with the section's last window, cut short or run on. Any other index
/// is handed back as it was decoded.
pub(super) fn attach_section(
    compression: Option<CompressionIndex>,
    file: File,
    base: u64,
) -> Result<std::result::Result<Option<CompressionIndex>, Unusable>> {
    let Some(CompressionIndex::Gzip(index)) = compression else {
        return Ok(Ok(compression));
    };
    let (offsets, end) = offsets(&index, base);
    if file.metadata()?.len() != end {
        return Ok(Err(Unusable::Unreadable));
    }
    let windows = SectionWindows { file, offsets };
    Ok(Ok(Some(CompressionIndex::Gzip(index.with_windows(Arc::new(windows))))))
}

/// Where each checkpoint's window begins in a section beginning at `base`,
/// and where the section ends.
fn offsets(index: &GzipIndex, base: u64) -> (Vec<u64>, u64) {
    let mut at = base;
    let offsets = (0..index.checkpoint_count())
        .map(|checkpoint| {
            let offset = at;
            at += index.window_len(checkpoint).expect("a counted checkpoint") as u64;
            offset
        })
        .collect();
    (offsets, at)
}

/// The windows of a cache's section, read positionally out of the file a
/// load decoded: a handle of its own, so a later save renaming another file
/// over the cache's name leaves it reading the bytes it was loaded from.
///
/// **Its offsets are resident beside the checkpoints**, one per checkpoint,
/// the index's own term, which no charge bills (`KD26`).
struct SectionWindows {
    file: File,
    offsets: Vec<u64>,
}

impl WindowProvider for SectionWindows {
    fn read_window(&self, checkpoint: usize, out: &mut [u8]) -> std::io::Result<()> {
        let offset = self.offsets.get(checkpoint).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "no such checkpoint")
        })?;
        self.file.read_exact_at(out, *offset)
    }
}

#[cfg(test)]
mod tests;
