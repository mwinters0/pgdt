//! **Each block's longest line is recorded by every pass that maps it**
//! (`docs/design/decisions.md`, "D140"): its start and its length, line ending
//! included, the earliest of the longest where several tie — at the metadata
//! and the data level, whose passes stream a row a whole chunk passes inside
//! where nothing reads its fields whole; serially and in the interior
//! workers; and by `build_map`'s eager pass. Each is held to the lines read
//! off the file's bytes directly.

use std::sync::Arc;

use pgdt_dump::cache::CacheMode;
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_mapping::{build_map, map_file};
use pgdt_model::{CopyBlock, DataBlock, LongestLine, PostgresInvalidValues, Span, SpanBody};
use tokio_util::sync::CancellationToken;

/// The chunk every pass here reads in, which the long rows outgrow.
const CHUNK: usize = 4 << 10;

/// `jobs` workers stated under no allowance, as `streamed_line.rs` writes it.
fn arranged(jobs: usize) -> pgdt_io::Arrangement {
    let request = pgdt_io::ArrangementRequest {
        jobs: pgdt_io::Jobs::Stated(jobs),
        ..pgdt_io::ArrangementRequest::recommended(jobs, None)
    };
    pgdt_io::Arrangement::resolve(&pgdt_io::Allowance::NoneFound, request)
}

/// A dump of five blocks: a long row among short ones; two rows as long as
/// each other, longer than the rest; CRLF-terminated rows, the longest
/// last; no row at all; and enough short rows to be cut into pieces, one of
/// them many chunks long near the block's end.
fn dump() -> Vec<u8> {
    let mut file = b"SET client_encoding = 'UTF8';\n\n".to_vec();
    file.extend_from_slice(b"COPY public.long (a, b) FROM stdin;\n");
    for i in 0..200 {
        file.extend_from_slice(format!("{i}\tshort\n").as_bytes());
    }
    file.extend_from_slice(
        format!("200\t{}\n201\tshort\n\\.\n\n", "x".repeat(5 * CHUNK)).as_bytes(),
    );

    file.extend_from_slice(b"COPY public.tied (a) FROM stdin;\n");
    let tied = "y".repeat(3 * CHUNK);
    file.extend_from_slice(format!("1\n{tied}\n2\n{tied}\n3\n\\.\n\n").as_bytes());

    file.extend_from_slice(b"COPY public.crlf (a) FROM stdin;\r\n");
    file.extend_from_slice(format!("1\r\n22\r\n{}\r\n\\.\r\n\n", "z".repeat(2 * CHUNK)).as_bytes());

    file.extend_from_slice(b"COPY public.empty (a) FROM stdin;\n\\.\n\n");

    file.extend_from_slice(b"COPY public.many (a, b) FROM stdin;\n");
    for i in 0..20_000 {
        file.extend_from_slice(format!("{i}\tvalue {i}\n").as_bytes());
    }
    file.extend_from_slice(
        format!("20000\t{}\n20001\tlast\n\\.\n", "w".repeat(7 * CHUNK)).as_bytes(),
    );
    file
}

/// The longest line of `block`, read off `file` line by line: the first of
/// the longest, each measured through its LF.
fn expected(file: &[u8], block: &CopyBlock) -> Option<LongestLine> {
    let data = &file[block.data_offset as usize..block.terminator_offset as usize];
    let mut longest: Option<LongestLine> = None;
    let mut offset = block.data_offset;
    for line in data.split_inclusive(|byte| *byte == b'\n') {
        let len = line.len() as u64;
        if longest.is_none_or(|l| len > l.len) {
            longest = Some(LongestLine { offset, len });
        }
        offset += len;
    }
    longest
}

fn blocks(spans: &[Span]) -> Vec<&CopyBlock> {
    spans
        .iter()
        .filter_map(|span| match &span.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(&**block),
            _ => None,
        })
        .collect()
}

/// Every block's recorded longest line is the one its bytes hold, and the
/// blocks are the five written, the rows past every chunk among them.
fn check(file: &[u8], spans: &[Span], pass: &str) {
    let blocks = blocks(spans);
    assert_eq!(blocks.len(), 5, "{pass}");
    for block in &blocks {
        let table = block.header.qualified_name();
        assert_eq!(block.longest_line, expected(file, block), "{pass}: {table}");
    }
    let lengths: Vec<Option<u64>> =
        blocks.iter().map(|block| block.longest_line.map(|line| line.len)).collect();
    let crlf = 2 * CHUNK as u64 + 2;
    let want = [
        Some(5 * CHUNK as u64 + 5),
        Some(3 * CHUNK as u64 + 1),
        Some(crlf),
        None,
        Some(7 * CHUNK as u64 + 7),
    ];
    assert_eq!(lengths, want, "{pass}");
    // Of two lines as long, the first.
    let tied = blocks[1];
    assert_eq!(tied.longest_line.map(|line| line.offset), Some(tied.data_offset + 2), "{pass}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_pass_records_each_blocks_longest_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lines.sql");
    let file = dump();
    std::fs::write(&path, &file).unwrap();
    let read = ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() };

    for (level, statistics) in
        [("metadata", StatisticsRequest::METADATA), ("data", StatisticsRequest::DATA)]
    {
        for jobs in [1usize, 4] {
            let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&path).unwrap());
            let opened = CacheMode::DISABLED.open_source(source).await.unwrap();
            let run = map_file(
                opened,
                read,
                &arranged(jobs),
                PostgresInvalidValues::Default,
                &statistics,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
            check(&file, &run.index.spans, &format!("{level} level, {jobs} jobs"));
        }
    }

    let source = LocalFileSource::open(&path).unwrap();
    let options = ScanOptions { read, ..ScanOptions::default() };
    check(&file, &build_map(&source, &options).await.unwrap(), "build_map");
}
