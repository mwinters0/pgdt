//! **The central promise this file exists to assert, on the artifact a user
//! keeps.**
//! `--jobs` decides how a dump is read and never what the scan records, so a
//! partitioned `pgdt parse` and a serial one write the same `.dtcache` — byte
//! for byte, over every fixture in the tree
//! (`docs/design/decisions.md`, "D52").
//!
//! **Bytes, not a decoded index.** `crates/pgdt-query/tests/map_file.rs` already
//! compares the in-memory `DumpIndex` a parallel mapping pass builds against an
//! eager one's, over five schemas. What only the binary can say is that the
//! *file* the next command reads is the same file: a field that survives the
//! index comparison and encodes differently — a span vector accumulated in a
//! different order, a census unioned rather than replaced — would leave `pgdt
//! info` reporting one thing after a serial scan and another after a parallel
//! one, and nothing in the library's tests would see it.
//!
//! **The reference is `--jobs 1` stated, never the default**, for the reason
//! `tests/parallelism.rs` states: the default is 1 today, so the two coincide,
//! but a reference that inherited it would follow the default wherever it goes
//! next and could end up comparing one partitioned run against another. The
//! flagless leg is asserted *against* that reference instead, which is where
//! "and both equal what this build produces today" lands.
//!
//! **A cache path per run.** The library refuses to overwrite a cache recorded
//! against a different file (`Error::CacheSourceMismatch`,
//! `docs/design/decisions.md`, "D20"), so one reused path would fail
//! the second fixture rather than assert anything about the first.
//!
//! **Why the small-chunk legs state a chunk size.** `LocalFileSource`'s
//! partition is a fixed multiple of the read chunk, and `pgdt_mapping`'s `scan_region`
//! declines a region with less than one partition left in the file — so at the
//! shipped 1 MiB every fixture here (none reaching one partition) is declined whole
//! and a `--jobs 8` leg would be the serial path compared to itself, which is
//! the trap `map_file.rs` names. 64 bytes cuts nearly every block and 512
//! cuts the larger ones and leaves the rest to the serial scanner, so the
//! mixed scan is covered too. The leg that runs the **shipped** configuration
//! parallel is [`a_region_past_the_shipped_chunk_size_writes_the_serial_cache`],
//! whose dump is generated large enough to clear that floor with no chunk size
//! stated at all.
//!
//! **A compressed source is where `--jobs` changes which decode work happens**
//! — whole units by several readers, or one forward decode — so it is where a
//! determinism defect would live, and
//! [`an_xz_dump_writes_the_serial_cache_at_every_stated_parallelism`] runs the
//! same comparison over `.xz` input, and
//! [`a_gzip_dump_writes_one_cache_however_its_first_pass_ran`] over a single
//! gzip member, whose index a first pass builds — and where that pass was
//! interrupted is the other thing that must not change the cache.

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use pgdt_core::Result;
use pgdt_dump::ReadOptions;
use pgdt_dump::cache::CacheMode;
use pgdt_io::{
    ByteRangeSource, CompressedFileSource, CompressionIndex, Extent, KnownCompression,
    LocalFileSource, Origin, Parallelism, Recognized, SCAN_CHUNK_DEFAULT_SIZE_BYTES, WaitPolicy,
};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::PostgresInvalidValues;
use tokio_util::sync::CancellationToken;

mod common;
use common::{all_fixtures, refused_field, run, stderr_of};

/// `pgdt parse` under `extra`, writing its cache to `out`, and the bytes it
/// wrote.
///
/// The dump is read where it lies rather than copied: the cache records the
/// source's stored size and mtime, so every leg of a comparison has to be
/// looking at one file. Only the cache path moves.
///
/// **These legs are at the metadata level**, so a difference in the map is not
/// hidden among statistics or a census; [`gathered_cache_of`] is the same run
/// at the data level.
fn cache_of(dump: &Path, out: &Path, extra: &[&str]) -> Vec<u8> {
    parse_cache(dump, out, &["--statistics-level", "metadata"], extra)
}

/// `pgdt parse` gathering every statistic at [`TINY_GROUP`] bytes a group, so
/// every fixture block holds many groups and nearly every cut falls inside one.
fn gathered_cache_of(dump: &Path, out: &Path, extra: &[&str]) -> Vec<u8> {
    parse_cache(dump, out, &["--row-group-size", TINY_GROUP], extra)
}

/// The group size the gathering legs state: tens of bytes, where the shipped
/// mebibyte makes nearly every fixture block one group and a join has little
/// to cross.
const TINY_GROUP: &str = "32";

/// A data-level leg of a fixture holding a field PostgreSQL refuses keeps that
/// column at the metadata level, a data-level parse failing on it
/// (`pgdt_fixtures::sweep_request`).
fn parse_cache(dump: &Path, out: &Path, statistics: &[&str], extra: &[&str]) -> Vec<u8> {
    let mut args = vec!["parse", "--source", dump.to_str().unwrap(), "--dtcache"];
    args.push(out.to_str().unwrap());
    args.extend_from_slice(statistics);
    let past_refused = refused_field(dump).map(|r| format!("data,{}", r.statistics_level()));
    if let Some(level) =
        past_refused.as_deref().filter(|_| !statistics.contains(&"--statistics-level"))
    {
        args.extend_from_slice(&["--statistics-level", level]);
    }
    args.extend_from_slice(extra);
    let output = run(&args);
    assert!(
        output.status.success(),
        "pgdt parse {extra:?} on {}: {}",
        dump.display(),
        stderr_of(&output)
    );
    let bytes = std::fs::read(out).expect("parse writes a cache at the path it was given");
    assert!(!bytes.is_empty(), "{}: {extra:?} wrote an empty cache", dump.display());
    bytes
}

/// Byte equality with a failure message that names the leg and the first
/// disagreeing offset, rather than printing two caches at each other.
fn assert_same_cache(got: &[u8], reference: &[u8], dump: &Path, leg: &[&str]) {
    if got == reference {
        return;
    }
    let at = got.iter().zip(reference).position(|(a, b)| a != b);
    panic!(
        "{}: {leg:?} wrote a different cache — {} bytes against the serial {}, \
         first difference at {at:?}",
        dump.display(),
        got.len(),
        reference.len(),
    );
}

/// **Every fixture, every stated parallelism, one cache.** The sweep is read
/// off the tree, so a schema or flag set the generator gains is covered the
/// moment it is written.
///
/// The flagless leg is what makes this say "and what this build produces
/// today": it is the invocation a person types, and it must agree with the
/// serial path it currently resolves to.
#[test]
fn every_fixture_parses_to_the_same_cache_at_every_stated_parallelism() {
    let dir = tempfile::tempdir().unwrap();
    let legs: [&[&str]; 5] = [
        // What a person who states nothing gets.
        &[],
        // The stated flag at the shipped chunk size: every region declined,
        // so this is the flag proving it changes nothing on its own.
        &["--jobs", "8"],
        // A chunk size small enough to cut, serially — so a difference here
        // would be the chunk size's doing rather than the workers'.
        &["--jobs", "1", "--chunk-size", "64"],
        // The same cut, run by workers: the leg that actually splits block
        // interiors.
        &["--jobs", "8", "--chunk-size", "64"],
        // The mixed scan — larger blocks cut, smaller ones left serial.
        &["--jobs", "8", "--chunk-size", "512"],
    ];

    for (n, fixture) in all_fixtures().iter().enumerate() {
        let reference =
            cache_of(fixture, &dir.path().join(format!("{n}-serial.dtcache")), &["--jobs", "1"]);
        for (l, leg) in legs.iter().enumerate() {
            let got = cache_of(fixture, &dir.path().join(format!("{n}-{l}.dtcache")), leg);
            assert_same_cache(&got, &reference, fixture, leg);
        }
    }
}

/// **Statistics gathered by workers are the serial pass's, byte for byte.**
/// The same sweep with every statistic gathered at a group size of tens of
/// bytes: a cut the leader makes inside a group joins the two halves' bounds,
/// dictionaries and row order, and a mistake there is a different cache.
#[test]
fn every_fixture_gathers_the_same_statistics_at_every_stated_parallelism() {
    let dir = tempfile::tempdir().unwrap();
    let legs: [&[&str]; 4] = [
        &["--jobs", "1", "--chunk-size", "64"],
        &["--jobs", "8", "--chunk-size", "64"],
        &["--jobs", "3", "--chunk-size", "64"],
        &["--jobs", "8", "--chunk-size", "512"],
    ];
    for (n, fixture) in all_fixtures().iter().enumerate() {
        let reference = gathered_cache_of(
            fixture,
            &dir.path().join(format!("{n}-serial.dtcache")),
            &["--jobs", "1"],
        );
        for (l, leg) in legs.iter().enumerate() {
            let got = gathered_cache_of(fixture, &dir.path().join(format!("{n}-{l}.dtcache")), leg);
            assert_same_cache(&got, &reference, fixture, leg);
        }
    }
}

/// **A cache back-filled with statistics is the cache one gathering `parse`
/// writes, byte for byte.** Each fixture is parsed at the metadata level, or
/// at the default group size, and then parsed again asking for every statistic at
/// [`TINY_GROUP`] bytes into the same cache, which re-reads every block —
/// serially, and split by workers at a chunk the leader cuts at.
#[test]
fn every_fixture_backfills_to_the_cache_one_gathering_parse_writes() {
    let dir = tempfile::tempdir().unwrap();
    let legs: [(&[&str], &[&str]); 2] = [
        (&["--statistics-level", "metadata"], &["--jobs", "1"]),
        (&[], &["--jobs", "8", "--chunk-size", "64"]),
    ];
    for (n, fixture) in all_fixtures().iter().enumerate() {
        let reference = gathered_cache_of(
            fixture,
            &dir.path().join(format!("{n}-gathered.dtcache")),
            &["--jobs", "1"],
        );
        for (l, (first, second)) in legs.iter().enumerate() {
            let out = dir.path().join(format!("{n}-{l}-backfilled.dtcache"));
            parse_cache(fixture, &out, first, &["--jobs", "1"]);
            let got = gathered_cache_of(fixture, &out, second);
            assert_same_cache(&got, &reference, fixture, second);
        }
    }
}

/// What one partition costs a plain source at the shipped chunk size — the
/// floor `pgdt_mapping`'s `scan_region` applies.
///
/// **Read off the source rather than restated as a chunk count.** A
/// partition is a multiple of the read chunk and the multiple is the
/// library's to choose, so a test that spelled the product out would go
/// quietly vacuous the next time it moved: the dump below would stop clearing
/// the floor and every leg would be the serial path compared to itself.
fn plain_partition_bytes(dir: &Path) -> u64 {
    let probe = dir.join("probe.bin");
    std::fs::write(&probe, b"x").unwrap();
    let source = LocalFileSource::open(&probe).unwrap();
    source.hint_read_size(SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    source.partitions(0..1).partition_bytes()
}

/// A dump whose single `COPY` region runs well past one of the source's own
/// partitions, with the offset its data starts at.
///
/// Rows are generated until the file clears four partitions, so the leader's
/// floor — less than one `partition_bytes()` left in the file below the
/// region's data offset — is cleared by a margin rather than exactly, and four
/// workers have a partition each.
fn dump_past_the_chunk_size(dir: &Path, partition: u64) -> (PathBuf, u64) {
    let header = "SET client_encoding = 'UTF8';\n\n\
         CREATE TABLE public.wide (\n    id integer,\n    name text\n);\n\n\
         COPY public.wide (id, name) FROM stdin;\n";
    let mut file = String::from(header);
    let want = header.len() + 4 * partition as usize;
    let mut id: u64 = 1;
    while file.len() < want {
        file.push_str(&format!("{id}\trow number {id}\n"));
        id += 1;
    }
    file.push_str("\\.\n\n");
    let path = dir.join("past_the_chunk.sql");
    std::fs::write(&path, &file).unwrap();
    (path, header.len() as u64)
}

/// **The shipped configuration, run parallel.** Every other leg in this file
/// states a chunk size to get the leader to cut at all; this one states only
/// `--jobs`, over a dump big enough that `scan_region` cuts it at the shipped
/// defaults — which is the arrangement a person raising `--jobs` on a real
/// dump actually runs.
///
/// The precondition is asserted rather than assumed: if the source's own
/// partition ever grows past what this dump clears, the test fails saying so
/// instead of quietly degrading into the serial path compared to itself.
#[test]
fn a_region_past_the_shipped_chunk_size_writes_the_serial_cache() {
    let dir = tempfile::tempdir().unwrap();
    let partition = plain_partition_bytes(dir.path());
    let (dump, data_offset) = dump_past_the_chunk_size(dir.path(), partition);
    let size = std::fs::metadata(&dump).unwrap().len();
    assert!(
        size - data_offset >= 4 * partition,
        "the generated dump no longer clears the leader's floor: {} data bytes against a \
         {partition}-byte partition",
        size - data_offset,
    );

    let reference = cache_of(&dump, &dir.path().join("serial.dtcache"), &["--jobs", "1"]);
    let legs: [&[&str]; 4] = [&[], &["--jobs", "4"], &["--jobs", "8"], &["--jobs", "24"]];
    for (l, leg) in legs.iter().enumerate() {
        let got = cache_of(&dump, &dir.path().join(format!("leg{l}.dtcache")), leg);
        assert_same_cache(&got, &reference, &dump, leg);
    }
}

/// `dump` compressed by `xz` with `args` beside it, as `<name>.xz`. `xz` is
/// not `mise`-pinned, so a missing binary fails with the remedy rather than
/// the test skipping (`docs/design/roadmap.md`, "A test may assume the tools
/// `mise` pins").
fn xz_beside(dump: &Path, args: &[&str]) -> PathBuf {
    let out = Command::new("xz")
        .args(args)
        .arg("-c")
        .arg(dump)
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let path = dump.with_extension("sql.xz");
    std::fs::write(&path, &out.stdout).unwrap();
    path
}

/// **A compressed dump writes one cache at every stated parallelism**, byte
/// for byte: the serial reference decodes forward, the parallel legs decode
/// whole blocks on several readers and cut at their boundaries, and the
/// budget-starved leg reads every block in pieces through one live decode —
/// three ways of reaching the plaintext, one cache. The gathering legs
/// check the statistics a cut joins across a block boundary.
///
/// The cut is asserted rather than assumed: the file must hold more blocks
/// than any leg's workers and clear the leader's floor at the source's own
/// partition, or every leg would be the serial path compared to itself.
#[test]
fn an_xz_dump_writes_the_serial_cache_at_every_stated_parallelism() {
    let dir = tempfile::tempdir().unwrap();
    let partition = plain_partition_bytes(dir.path());
    let (dump, data_offset) = dump_past_the_chunk_size(dir.path(), partition);
    let xz = xz_beside(&dump, &["-0", "--block-size=1MiB"]);

    let source = CompressedFileSource::open(&xz).unwrap();
    let index = source.compression_index().expect("an `.xz` source has an index");
    source.hint_read_size(SCAN_CHUNK_DEFAULT_SIZE_BYTES);
    let advice = source.partitions(0..index.uncompressed_size());
    assert!(index.unit_count() > 8, "{} blocks", index.unit_count());
    assert!(
        index.uncompressed_size() - data_offset >= 2 * advice.partition_bytes(),
        "the generated dump no longer clears the leader's floor on `.xz`: {} data bytes \
         against a {}-byte partition",
        index.uncompressed_size() - data_offset,
        advice.partition_bytes(),
    );

    let reference = cache_of(&xz, &dir.path().join("xz-serial.dtcache"), &["--jobs", "1"]);
    let legs: [&[&str]; 4] = [
        // The recommendation: the cores, capped at the block count.
        &[],
        &["--jobs", "4"],
        &["--jobs", "8"],
        // A budget below one block: the piecewise arm, one partition.
        &["--jobs", "8", "--memory", "1048576"],
    ];
    for (l, leg) in legs.iter().enumerate() {
        let got = cache_of(&xz, &dir.path().join(format!("xz-{l}.dtcache")), leg);
        assert_same_cache(&got, &reference, &xz, leg);
    }

    let reference =
        gathered_cache_of(&xz, &dir.path().join("xz-gathered.dtcache"), &["--jobs", "1"]);
    let got = gathered_cache_of(&xz, &dir.path().join("xz-gathered-8.dtcache"), &["--jobs", "8"]);
    assert_same_cache(&got, &reference, &xz, &["--jobs", "8"]);
}

/// A dump of `tables` tables of about `table_bytes` each, compressed as one
/// gzip member by `flate2`, flushed every 256 KiB so its deflate blocks end
/// whatever backend `flate2` is built over — a member of many units at the
/// shipped spacing.
fn gzip_dump_of(dir: &Path, tables: usize, table_bytes: usize) -> PathBuf {
    let mut plain = String::from("SET client_encoding = 'UTF8';\n\n");
    for t in 0..tables {
        plain.push_str(&format!(
            "CREATE TABLE public.t{t} (\n    id integer,\n    name text\n);\n\n\
             COPY public.t{t} (id, name) FROM stdin;\n"
        ));
        let start = plain.len();
        let mut id: u64 = 1;
        while plain.len() - start < table_bytes {
            plain.push_str(&format!("{id}\trow {id} of table {t}\n"));
            id += 1;
        }
        plain.push_str("\\.\n\n");
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for piece in plain.as_bytes().chunks(256 * 1024) {
        encoder.write_all(piece).unwrap();
        encoder.flush().unwrap();
    }
    let path = dir.join("many_units.sql.gz");
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    path
}

/// A source that asks its run to stop once a read reaches `after`: an
/// interruption at a place the test chooses, everything else the source's
/// own.
struct StopAfter {
    inner: Arc<dyn ByteRangeSource>,
    after: u64,
    cancel: CancellationToken,
}

impl StopAfter {
    fn note(&self, offset: u64) {
        if offset >= self.after {
            self.cancel.cancel();
        }
    }
}

impl ByteRangeSource for StopAfter {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        self.note(offset);
        self.inner.read_range(offset, len)
    }
    fn read_at_most(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        self.note(offset);
        self.inner.read_at_most(offset, len)
    }
    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        self.inner.extent()
    }
    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.inner.stored_size()
    }
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }
    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }
    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.inner.hint_wait_policy(policy);
    }
    fn hint_cancellation(&self, cancel: CancellationToken) {
        self.inner.hint_cancellation(cancel);
    }
    fn compression_index(&self) -> Option<CompressionIndex> {
        self.inner.compression_index()
    }
    fn partitions(&self, range: std::ops::Range<u64>) -> pgdt_io::Partitioning {
        self.inner.partitions(range)
    }
    fn complete_reads(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        self.inner.complete_reads()
    }
}

/// `dump`'s first pass run in the library to the first read at or past
/// `after`, where its run is asked to stop and saves what it holds to
/// `cache` — the state an interrupted `pgdt parse` leaves — and the
/// compression index it saved.
async fn interrupted_at(dump: &Path, cache: &Path, after: u64) -> pgdt_io::GzipIndex {
    let origin = Origin::local(dump);
    let Recognized::Source(source) =
        pgdt_io::open(&origin, KnownCompression::Unknown, &std::env::temp_dir()).await.unwrap()
    else {
        unreachable!("claiming nothing is never a mismatch");
    };
    let cancel = CancellationToken::new();
    let stopping = Arc::new(StopAfter { inner: source, after, cancel: cancel.clone() });
    let run = map_file(
        CacheMode::enabled(cache).open_source(Arc::clone(&stopping) as _).await.unwrap(),
        ReadOptions::default(),
        &pgdt_fixtures::arranged(1),
        PostgresInvalidValues::Default,
        &StatisticsRequest::METADATA,
        &cancel,
    )
    .await
    .unwrap();
    assert!(run.interrupted, "the pass stops where it was asked to");
    match stopping.compression_index() {
        Some(CompressionIndex::Gzip(index)) => index,
        other => panic!("a gzip first pass answers a gzip index, not {other:?}"),
    }
}

/// **A gzip dump writes one cache however its first pass ran**, byte for
/// byte: serially as every first pass is, under a stated `--jobs` that the
/// open extent leaves serial, at a chunk that ends nowhere in particular, and
/// resumed by `pgdt parse` from a pass interrupted after its first checkpoint
/// past the start — before the interrupted map's watermark and after it.
/// Checkpoints are placed by the file alone, so the index, its windows and
/// the map are the uninterrupted pass's.
#[tokio::test]
async fn a_gzip_dump_writes_one_cache_however_its_first_pass_ran() {
    let dir = tempfile::tempdir().unwrap();
    let dump = gzip_dump_of(dir.path(), 6, 10 << 20);
    let reference = cache_of(&dump, &dir.path().join("gz-serial.dtcache"), &["--jobs", "1"]);
    let saved = pgdt_dump::cache::OfflineCache::new(dir.path().join("gz-serial.dtcache"));
    let pgdt_dump::cache::CacheStatus::Loaded(saved) = saved.load().await.unwrap() else {
        panic!("a parse saves a cache");
    };
    let shape = saved.compression.expect("a gzip cache records its compression");
    assert!(
        saved.is_complete() && shape.blocks > 2,
        "{} units: {}",
        shape.blocks,
        serde_json::to_string(&saved.envelope).unwrap()
    );

    let legs: [&[&str]; 3] = [&[], &["--jobs", "8"], &["--jobs", "1", "--chunk-size", "4099"]];
    for (l, leg) in legs.iter().enumerate() {
        let got = cache_of(&dump, &dir.path().join(format!("gz-{l}.dtcache")), leg);
        assert_same_cache(&got, &reference, &dump, leg);
    }

    for (n, after) in [(25u64 << 20), (45 << 20)].into_iter().enumerate() {
        let cache = dir.path().join(format!("gz-resumed-{n}.dtcache"));
        let partial = interrupted_at(&dump, &cache, after).await;
        assert!(!partial.is_complete() && partial.unit_count() >= 1, "stopped at {after}");
        let got = cache_of(&dump, &cache, &["--jobs", "1"]);
        assert_same_cache(&got, &reference, &dump, &["resumed", &after.to_string()]);
    }
}
