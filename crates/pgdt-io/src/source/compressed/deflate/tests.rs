use std::sync::Arc;

use crate::memory::DEFAULT_MEMORY_BUDGET;
use crate::source::compressed::blocks::BlockCache;
use crate::test_support::*;
use crate::{
    ByteRangeSource, CompressedFileSource, Extent, FetchedCompressedSource, KnownCompression,
    LocalFileSource, Origin, Parallelism, Recognized,
};

use super::*;

/// The spacing the tests index at: a few dozen units over the payload, each
/// past several deflate blocks.
const SPACING: u64 = 32 * 1024;

/// **A unit of the spacing is declined whole-block decode under the
/// default budget, as an ordinary `.xz` block is** (`docs/design/
/// decisions.md`, "D3", "D113"): a library caller stating no budget
/// reads a single member's units piecewise. Asserted with no decoder
/// charged, so it holds whatever the inflater's footprint; every unit
/// but a file's last is at least the spacing. A unit of the power of two
/// below the spacing is refused only for the chunk, so this pins the
/// spacing's room from the line, not only the refusal (D113).
#[test]
fn the_default_budget_declines_a_unit_of_the_spacing() {
    let cache = BlockCache::for_unit(CHECKPOINT_SPACING.get()).expect("a nonzero unit");
    let chunk = crate::SCAN_CHUNK_DEFAULT_SIZE_BYTES as u64;
    assert!(!cache.affordable(chunk, 0, DEFAULT_MEMORY_BUDGET));
}

/// **The checkpoints are held at a stated cost per entry** — what is left of
/// `KD26` for gzip, an index the size of its offsets
/// (`docs/design/decisions.md`, "D4"): a checkpoint is its plaintext offset,
/// its bit, its window's length and its unit's CRC-32, and a change to
/// `gz_seek` growing it fails here rather than silently moving the account.
#[test]
fn a_checkpoint_is_held_at_a_stated_cost() {
    assert_eq!(std::mem::size_of::<gz_seek::Checkpoint>(), 32);
}

/// A source over a buffer that lends every byte it holds, as a fetched
/// window does: the feed a lending decode takes.
struct Lent(Vec<u8>);

impl Positioned for Lent {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let at = (offset as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - at);
        buf[..n].copy_from_slice(&self.0[at..at + n]);
        Ok(n)
    }

    fn size(&self) -> std::io::Result<u64> {
        Ok(self.0.len() as u64)
    }

    fn slice_at(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let at = usize::try_from(offset).ok()?;
        self.0.get(at..at.checked_add(len)?)
    }
}

/// **The codec's units are the index's units**, as the arms read them: in
/// file order, tiling the plaintext, each compressed extent reaching its
/// checks, each decoded whole to its plaintext over a source that lends and
/// over one that does not.
#[test]
fn the_codecs_units_are_the_indexs_units() {
    let payload = gzip_test_payload();
    let (compressed, layout) = gzip_indexed(&payload, SPACING);
    let file = std::fs::File::open(compressed.path()).unwrap();
    let stored_size = file.metadata().unwrap().len();
    let codec = DeflateCodec::over(layout.clone(), stored_size).unwrap();

    assert_eq!(codec.name(), "gzip");
    assert_eq!(codec.index(), CompressionIndex::Gzip(layout));
    assert_eq!(codec.plaintext_size(), payload.len() as u64);
    assert_eq!(codec.compressed_size(), stored_size);
    assert!(codec.unit_count() > 2, "{} units", codec.unit_count());
    let units: Vec<_> = (0..codec.unit_count()).map(|i| codec.plaintext(i).unwrap()).collect();
    assert_eq!(units.first().map(|u| u.start), Some(0));
    assert!(units.windows(2).all(|w| w[0].end == w[1].start), "{units:?}");
    assert_eq!(units.last().map(|u| u.end), Some(codec.plaintext_size()));
    assert!(units[..units.len() - 1].iter().all(|u| u.end - u.start >= SPACING), "{units:?}");
    assert_eq!(codec.plaintext(codec.unit_count()), None);
    assert_eq!(codec.units_in(units[1].start..units[1].start + 1), 1..2);
    assert_eq!(codec.units_in(units[1].start - 1..units[2].start + 1), 0..3);
    assert!(codec.units_in(codec.plaintext_size()..codec.plaintext_size() + 1).is_empty());
    let widest = (0..codec.unit_count())
        .map(|i| codec.compressed(i).map(|c| c.end - c.start).unwrap())
        .max();
    assert_eq!(widest, Some(codec.widest_compressed()));
    let largest = units.iter().map(|u| u.end - u.start).max();
    assert_eq!(largest, Some(codec.largest_unit()));
    assert!(codec.pulling_footprint() > codec.lending_footprint(), "the input chunk");

    // Every unit decoded whole, pulled from the file and fed from bytes lent.
    let lent = Lent(std::fs::read(compressed.path()).unwrap());
    for (unit, range) in units.iter().enumerate() {
        let want = &payload[range.start as usize..range.end as usize];
        let mut out = vec![0u8; want.len()];
        codec.decode(unit, &file, &mut out).unwrap();
        assert_eq!(out, want, "unit {unit}, pulled");
        out.fill(0);
        codec.decode(unit, &lent, &mut out).unwrap();
        assert_eq!(out, want, "unit {unit}, lent");
    }
}

/// **An index that does not describe the file is refused as such**, the
/// error [`refuses_index`] names and recognition turns into a mismatch; an
/// incomplete index, or one handed in with nowhere to read its windows, is a
/// caller's error instead, and no claim about the file.
#[test]
fn an_index_for_another_file_is_refused_as_not_describing_it() {
    let (compressed, layout) = gzip_indexed(&gzip_test_payload(), SPACING);
    let stored_size = std::fs::metadata(compressed.path()).unwrap().len();

    let error = DeflateCodec::over(layout.clone(), stored_size + 1).err().expect("refused");
    assert!(refuses_index(&error), "{error}");
    assert!(error.to_string().starts_with("gzip error: "), "{error}");

    let bare = GzipIndex { index: layout.index.clone(), windows: None };
    let error = DeflateCodec::over(bare, stored_size).err().expect("no windows");
    assert!(!refuses_index(&error), "{error}");

    let file = std::fs::read(compressed.path()).unwrap();
    let mut pass = gz_seek::Forward::new(NonZeroU64::new(SPACING).unwrap()).unwrap();
    let mut out = vec![0u8; 4096];
    pass.read(&file, &mut out).unwrap();
    let partial = GzipIndex { index: pass.index(), windows: layout.windows.clone() };
    assert!(!partial.is_complete());
    let error = DeflateCodec::over(partial, stored_size).err().expect("incomplete");
    assert!(!refuses_index(&error), "{error}");
}

/// The payload's gzip file with one byte inside unit `unit`'s deflate data
/// flipped, and the index of the intact file.
fn damaged(unit: usize) -> (Vec<u8>, tempfile::NamedTempFile, GzipIndex) {
    let payload = gzip_test_payload();
    let (compressed, layout) = gzip_indexed(&payload, SPACING);
    let range = layout.task(unit).compressed_range();
    let mut bytes = std::fs::read(compressed.path()).unwrap();
    bytes[((range.start + range.end) / 2) as usize] ^= 0x55;
    std::fs::write(compressed.path(), &bytes).unwrap();
    (payload, compressed, layout)
}

/// **A damaged unit is refused, never read as other bytes**: whichever check
/// meets it first — the inflater's, the unit's own CRC-32 or the index's
/// placement — the decode is a gzip codec error.
#[test]
fn a_damaged_unit_is_refused() {
    let (_, compressed, layout) = damaged(1);
    let file = std::fs::File::open(compressed.path()).unwrap();
    let codec = DeflateCodec::over(layout, file.metadata().unwrap().len()).unwrap();
    let len = codec.plaintext(1).map(|u| u.end - u.start).unwrap() as usize;
    let error = codec.decode(1, &file, &mut vec![0u8; len]).expect_err("refused");
    assert!(error.codec_cause::<gz_seek::Error>().is_some(), "{error}");
    // Its neighbours are untouched.
    let len = codec.plaintext(0).map(|u| u.end - u.start).unwrap() as usize;
    codec.decode(0, &file, &mut vec![0u8; len]).unwrap();
}

/// Both ways to a gzip file's bytes over an index handed in: the local file
/// and the same file fetched through a plain source.
async fn arms(path: &std::path::Path, layout: &GzipIndex) -> [Box<dyn ByteRangeSource>; 2] {
    let local = CompressedFileSource::with_index(path, CompressionIndex::Gzip(layout.clone()));
    let transport: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(path).unwrap());
    let stored_size = transport.stored_size().await.unwrap();
    let fetched = FetchedCompressedSource::with_index(
        transport,
        stored_size,
        CompressionIndex::Gzip(layout.clone()),
    );
    [Box::new(local.unwrap()), Box::new(fetched.unwrap())]
}

/// **One file, every arm, one answer**: a gzip file read through its index is
/// its plaintext on the local and the fetched arm, decoding whole units where
/// the budget holds one and in pieces where it does not — in reads that land
/// inside a unit, straddle a checkpoint and span several, forward and back.
#[tokio::test]
async fn a_gzip_file_read_through_its_index_is_its_plaintext_on_every_arm() {
    let payload = gzip_test_payload();
    let (compressed, layout) = gzip_indexed(&payload, SPACING);
    let starts: Vec<u64> = layout.unit_ranges().map(|u| u.start).collect();
    for budget in [None, Some(1)] {
        for source in arms(compressed.path(), &layout).await {
            source.hint_parallelism(Parallelism::Serial { memory_bytes: budget });
            assert_eq!(source.extent().await.unwrap(), Extent::Exact(payload.len() as u64));
            assert_eq!(source.compression_index(), Some(CompressionIndex::Gzip(layout.clone())));
            assert_eq!(
                source.stored_size().await.unwrap(),
                std::fs::metadata(compressed.path()).unwrap().len()
            );
            let mut offset = 0u64;
            for len in [100usize, 40_000, 3000, 70_000, 1] {
                let at = offset as usize;
                let got = source.read_range(offset, len).await.unwrap();
                assert_eq!(&got[..], &payload[at..at + len], "at {offset}, budget {budget:?}");
                offset += len as u64;
            }
            for &start in starts[1..].iter().rev() {
                let at = (start - 10) as usize;
                let got = source.read_range(start - 10, 20).await.unwrap();
                assert_eq!(&got[..], &payload[at..at + 20], "at {start}, budget {budget:?}");
            }
            let end = payload.len() as u64;
            assert!(source.read_range(end - 5, 10).await.is_err(), "past the end");
            source.complete_reads().await.unwrap();
        }
    }
}

/// **A unit a read stopped inside owes its check until the run ends**, on
/// both piecewise arms: the bytes before a damaged byte go out before the
/// unit can be checked, and `complete_reads` is what refuses it.
#[tokio::test]
async fn a_unit_a_read_stopped_inside_is_compared_when_reads_complete() {
    let (payload, compressed, layout) = damaged(1);
    let start = layout.task(1).plaintext_range().start;
    for source in arms(compressed.path(), &layout).await {
        source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
        let at = start as usize;
        let got = source.read_range(start, 10).await.unwrap();
        assert_eq!(&got[..], &payload[at..at + 10]);
        let error = source.complete_reads().await.expect_err("the unit's check");
        assert!(error.codec_cause::<gz_seek::Error>().is_some(), "{error}");
        source.complete_reads().await.unwrap();
    }
}

/// **The block path cuts at checkpoints**: a source whose budget holds a
/// unit advises partitions at the units' starts, as an `.xz` one does at its
/// blocks'.
#[tokio::test]
async fn a_gzip_file_partitions_at_its_checkpoints() {
    let payload = gzip_test_payload();
    let (compressed, layout) = gzip_indexed(&payload, SPACING);
    let starts: Vec<u64> = layout.unit_ranges().map(|u| u.start).collect();
    let source =
        CompressedFileSource::with_index(compressed.path(), CompressionIndex::Gzip(layout))
            .unwrap();
    source.hint_parallelism(Parallelism::workers(4, 64 << 20));
    let advice = source.partitions(0..payload.len() as u64);
    let crate::PartitionBoundaries::At(cuts) = advice.boundaries() else {
        panic!("a block-decoding source cuts at its units: {advice:?}");
    };
    assert!(cuts.iter().all(|cut| starts.contains(cut)), "{cuts:?} against {starts:?}");
}

/// **Recognition knows gzip by its magic**: an index a previous pass built
/// opens the file without building one, an index for another file or a claim
/// that the file is plain or `.xz` is a mismatch, and with nothing known the
/// file is refused by name, no first pass being built.
#[tokio::test]
async fn recognition_reads_gzip_through_an_index_and_refuses_it_without_one() {
    let payload = gzip_test_payload();
    let (compressed, layout) = gzip_indexed(&payload, SPACING);
    let origin = Origin::local(compressed.path());

    let known = KnownCompression::Compressed(CompressionIndex::Gzip(layout));
    let Recognized::Source(source) = crate::open(&origin, known).await.unwrap() else {
        panic!("a gzip file opens over its own index");
    };
    assert_eq!(&source.read_range(0, 64).await.unwrap()[..], &payload[..64]);

    let (other, other_layout) = gzip_indexed(&payload[..payload.len() / 2], SPACING);
    drop(other);
    let foreign = KnownCompression::Compressed(CompressionIndex::Gzip(other_layout));
    assert!(matches!(crate::open(&origin, foreign).await.unwrap(), Recognized::Mismatch));
    assert!(matches!(
        crate::open(&origin, KnownCompression::Plain).await.unwrap(),
        Recognized::Mismatch
    ));

    let refused = crate::open(&origin, KnownCompression::Unknown).await.err().expect("refused");
    assert!(
        matches!(&refused, pgdt_core::Error::SourceNotReadable { why, .. } if why.contains("gzip")),
        "{refused}"
    );
    let refused = CompressedFileSource::open(compressed.path()).err().expect("refused");
    assert!(matches!(refused, pgdt_core::Error::SourceNotReadable { .. }), "{refused}");
}

/// **An index is persisted as its checkpoints and read back as the same
/// index**, its provider left behind for the reader to attach; parts
/// `gz_seek` would refuse are undecodable rather than an index, so a damaged
/// cache never hands one out.
#[cfg(feature = "serde")]
#[test]
fn an_index_reads_back_as_its_checkpoints_and_refuses_parts_gz_seek_would() {
    let config = bincode::config::standard();
    let (_, layout) = gzip_indexed(&gzip_test_payload(), SPACING);
    let persisted = CompressionIndex::Gzip(layout.clone());
    let bytes = bincode::serde::encode_to_vec(&persisted, config).unwrap();
    let (back, _): (CompressionIndex, _) =
        bincode::serde::decode_from_slice(&bytes, config).unwrap();
    assert_eq!(back, persisted);
    let CompressionIndex::Gzip(back) = back else { panic!("a gzip index") };
    assert!(!back.has_windows() && layout.has_windows());

    // The persisted parts' shape, with a first checkpoint past the file's start.
    #[derive(serde::Serialize)]
    struct Parts {
        spacing: NonZeroU64,
        checkpoints: Vec<(u64, u64, Option<u16>, Option<u32>)>,
        end: Option<(u64, u64)>,
    }
    let damaged = Parts {
        spacing: NonZeroU64::new(SPACING).unwrap(),
        checkpoints: vec![(1, 0, None, None)],
        end: Some((10, 20)),
    };
    let bytes = bincode::serde::encode_to_vec(&damaged, config).unwrap();
    assert!(bincode::serde::decode_from_slice::<GzipIndex, _>(&bytes, config).is_err());
}

/// **`pg_dump`'s own `-Z` output reads through its index** at every routine
/// major, on both arms, as the plaintext another decoder makes of it.
#[tokio::test]
async fn pg_dump_s_compressed_dump_reads_through_its_index() {
    use std::io::Read;
    for version in pgdt_fixtures::VERSIONS {
        let path = pgdt_fixtures::gzip_fixture(version, "edge_cases", "compress");
        let file = std::fs::read(&path).unwrap();
        let mut plaintext = Vec::new();
        flate2::read::GzDecoder::new(&file[..]).read_to_end(&mut plaintext).unwrap();
        let layout = held::indexed_in_memory(&file, 1024);
        for source in arms(&path, &layout).await {
            let got = source.read_range(0, plaintext.len()).await.unwrap();
            assert_eq!(&got[..], &plaintext[..], "{}", path.display());
            source.complete_reads().await.unwrap();
        }
    }
}
