//! Where a first pass keeps the windows it places: a file of its own with no
//! name, made in the directory [`open`](crate::open)'s caller names, written as
//! each window is placed and read back positionally, as a loaded cache's
//! window section is (`docs/design/decisions.md`, "D115").

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use pgdt_core::{Error, Result};

use super::super::WindowProvider;

/// Every window the pass has placed, in its spill file for as long as an
/// index carrying it lives — the one the pass hands over included — and
/// before them those of the index it was resumed over, read through that
/// index's own provider: what a saved index's windows are read through.
///
/// **Only where each window sits is resident**, one entry a checkpoint, the
/// index's own term (`docs/design/decisions.md`, "D4"). The file is made at
/// the first window holding any bytes, so a pass that places none — a file
/// under the spacing, or one resumed into its last unit — makes none.
pub(super) struct PassWindows {
    /// The resumed index's provider, for the checkpoints before `base`.
    earlier: Option<Arc<dyn WindowProvider>>,
    /// The first checkpoint whose window is held here.
    base: usize,
    /// The directory the spill file is made in.
    dir: PathBuf,
    spilled: RwLock<Spilled>,
}

/// The spill file and where each window the pass placed sits in it.
#[derive(Default)]
struct Spilled {
    /// Made at the first window with any bytes.
    file: Option<File>,
    /// Each placed checkpoint's window: its offset in `file` and its length.
    at: Vec<(u64, u32)>,
    /// Where the next window is written.
    end: u64,
}

impl PassWindows {
    /// The windows of a pass from the file's start, spilled into `dir`: none
    /// yet.
    pub(super) fn fresh(dir: &Path) -> PassWindows {
        Self::after(None, 0, dir)
    }

    /// The windows of a pass resumed over an index of `base` checkpoints,
    /// whose own are read through `earlier`, spilled into `dir`.
    pub(super) fn after(
        earlier: Option<Arc<dyn WindowProvider>>,
        base: usize,
        dir: &Path,
    ) -> PassWindows {
        PassWindows { earlier, base, dir: dir.to_path_buf(), spilled: RwLock::default() }
    }

    /// Checkpoint `checkpoint` was placed with `window`: written to the spill
    /// file, unless it is one the resumed index already held — a member start
    /// placed again.
    ///
    /// # Errors
    ///
    /// A spill file that cannot be made, naming its directory — never
    /// answered by holding the window in memory instead — or a write to it
    /// that fails.
    pub(super) fn place(&self, checkpoint: usize, window: &[u8]) -> Result<()> {
        if checkpoint < self.base {
            return Ok(());
        }
        let mut guard = self.spilled.write().unwrap_or_else(|e| e.into_inner());
        let spilled = &mut *guard;
        debug_assert_eq!(
            checkpoint,
            self.base + spilled.at.len(),
            "checkpoints are placed in order"
        );
        let offset = spilled.end;
        if !window.is_empty() {
            let file = match &mut spilled.file {
                Some(file) => file,
                none => none.insert(self.make()?),
            };
            file.write_all_at(window, offset)?;
        }
        let len = u32::try_from(window.len()).expect("a window is at most 32 KiB");
        spilled.at.push((offset, len));
        spilled.end += u64::from(len);
        Ok(())
    }

    /// The spill file, made in `dir` with no name, so nothing outlives the
    /// descriptors on it (`docs/design/runtime-invariants.md`, "RT24").
    fn make(&self) -> Result<File> {
        tempfile::tempfile_in(&self.dir).map_err(|e| {
            Error::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "a gzip first pass cannot make the file it holds its windows in, in {}: {e}",
                    self.dir.display()
                ),
            ))
        })
    }
}

impl WindowProvider for PassWindows {
    fn read_window(&self, checkpoint: usize, out: &mut [u8]) -> std::io::Result<()> {
        if checkpoint < self.base {
            let earlier = self.earlier.as_ref().ok_or_else(|| {
                std::io::Error::other("a resumed gzip index's window has no provider")
            })?;
            return earlier.read_window(checkpoint, out);
        }
        let spilled = self.spilled.read().unwrap_or_else(|e| e.into_inner());
        let &(offset, len) = spilled
            .at
            .get(checkpoint - self.base)
            .ok_or_else(|| std::io::Error::other("a gzip checkpoint the pass has not placed"))?;
        if len as usize != out.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a window is read whole, into a buffer of its length",
            ));
        }
        match &spilled.file {
            Some(file) if len > 0 => file.read_exact_at(out, offset),
            _ => Ok(()),
        }
    }
}
