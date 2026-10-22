//! Fixtures more than one module's tests build: a file to read, a source
//! that implements nothing it need not, an `.xz` file and its seek table, a
//! gzip and a BGZF file and a fake filesystem root.

use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::pin::Pin;
use std::time::SystemTime;

use bytes::Bytes;

use pgdt_core::Result;

use crate::{ByteRangeSource, CompressionIndex, LocalFileSource, SeekTable};

pub(crate) fn source_of(bytes: &[u8]) -> (tempfile::NamedTempFile, LocalFileSource) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    let source = LocalFileSource::open(file.path()).unwrap();
    (file, source)
}

/// A source implementing nothing but the three required methods, to pin
/// what the *defaults* answer apart from any shipped source,
/// `CompressedFileSource` overriding them and `LocalFileSource` free to.
pub(crate) struct BareSource;

impl ByteRangeSource for BareSource {
    fn read_range(
        &self,
        _offset: u64,
        _len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async { Ok(Bytes::new()) })
    }
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async { Ok(0) })
    }
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async { Ok(None) })
    }
}

/// The `.xz` seek table a compressed source answers, for a test reading what
/// only that layout states — streams, block ranges, the compressed length.
pub(crate) fn seek_table_of(source: &dyn ByteRangeSource) -> SeekTable {
    match source.compression_index() {
        Some(CompressionIndex::Xz(table)) => table,
        None => panic!("an `.xz` source always has a table"),
    }
}

/// `xz` is not `mise`-pinned (`docs/design/decisions.md`, "D73"), so a
/// missing binary fails with the remedy rather than skipping silently
/// (`docs/design/roadmap.md`, "A test may assume the tools `mise`
/// pins").
pub(crate) fn require_xz() {
    let ok = std::process::Command::new("xz")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(ok, "`xz` is not runnable, so this test cannot build its fixture; install it.");
}

/// Compress `bytes` with `xz`, passing `extra_args` before the input path
/// (e.g. `&["--block-size=4096"]` for a seekable multi-block file, or
/// `&[]` for a single-stream, single-block one), and return the temp
/// file holding the compressed bytes.
pub(crate) fn xz_compress(bytes: &[u8], extra_args: &[&str]) -> tempfile::NamedTempFile {
    require_xz();
    let mut input = tempfile::NamedTempFile::new().unwrap();
    input.write_all(bytes).unwrap();
    input.flush().unwrap();
    let out = std::process::Command::new("xz")
        .args(extra_args)
        .arg("-c")
        .arg(input.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "xz {extra_args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut compressed = tempfile::NamedTempFile::new().unwrap();
    compressed.write_all(&out.stdout).unwrap();
    compressed.flush().unwrap();
    compressed
}

/// A payload long enough that a small `--block-size` reliably splits it
/// into several blocks — a throwaway pattern for pinning the compressed
/// source's own wiring, not a fixture.
pub(crate) fn xz_test_payload() -> Vec<u8> {
    (0..20_000u32).map(|i| (i % 251) as u8).collect()
}

/// A payload of many lines, long enough that BGZF cuts it into several
/// members and deflate into many blocks — a throwaway pattern, not a fixture,
/// but shaped like a `COPY` block's rows so that it compresses as text does.
pub(crate) fn gzip_test_payload() -> Vec<u8> {
    (0..20_000u32)
        .flat_map(|i| format!("{i}\tvalue {}\n", i.wrapping_mul(7919) % 10_007).into_bytes())
        .collect()
}

/// Compress `bytes` as one gzip member with `flate2`, whose header states no
/// name, comment or time — the shape `pg_dump -Fp -Z` writes — and return the
/// temp file holding it.
pub(crate) fn gzip_compress(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    written(&encoder.finish().unwrap())
}

/// Compress `bytes` as BGZF with `noodles-bgzf`'s writer — a member to each
/// block of at most 64 KiB, each stating its own length in a `BC` subfield,
/// and the empty member that ends the file — and return the temp file
/// holding it.
pub(crate) fn bgzf_compress(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut writer = noodles_bgzf::io::Writer::new(Vec::new());
    writer.write_all(bytes).unwrap();
    written(&writer.finish().unwrap())
}

fn written(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file
}

/// A filesystem root shaped like a Linux one, for driving discovery
/// against something other than the machine the test is running on.
///
/// **Everything the reader consults is under the root** — `proc/self/cgroup`,
/// `proc/meminfo`, and the cgroup mount — which is what makes the v1 arm
/// executable at all: the memory controller lives in exactly one hierarchy
/// (`RT6`), so no v1 shape can exist beside this machine's v2 one.
pub(crate) struct FakeRoot(tempfile::TempDir);

impl FakeRoot {
    pub(crate) fn new() -> FakeRoot {
        FakeRoot(tempfile::tempdir().unwrap())
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    /// Write `body` at `rel`, creating the directories above it.
    pub(crate) fn write(&self, rel: &str, body: &str) -> &FakeRoot {
        let at = self.0.path().join(rel);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, body).unwrap();
        self
    }

    /// The `0::` line for a v2 process at `cgroup`.
    pub(crate) fn v2(&self, cgroup: &str) -> &FakeRoot {
        self.write("proc/self/cgroup", &format!("0::{cgroup}\n"))
    }

    /// A v2 cgroup directory's two limit files, each `None` for absent.
    pub(crate) fn v2_limits(
        &self,
        cgroup: &str,
        max: Option<&str>,
        high: Option<&str>,
    ) -> &FakeRoot {
        let dir = format!("sys/fs/cgroup{cgroup}");
        let dir = dir.trim_end_matches('/');
        if let Some(max) = max {
            self.write(&format!("{dir}/memory.max"), &format!("{max}\n"));
        }
        if let Some(high) = high {
            self.write(&format!("{dir}/memory.high"), &format!("{high}\n"));
        }
        self
    }
}

#[cfg(test)]
mod tests;
