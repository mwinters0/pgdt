//! What decoding one long value holds beside its line: one decoded copy, in
//! a query over escaped text and over `bytea` (`docs/design/decisions.md`,
//! "D141") and over a nested value, decoded into its builders and tested
//! for its tier by a walk staging nothing ("D153"),
//! none in a filter's term reading an escaped field off its bytes
//! ("D150") or ordering a nested one as the output grammars' walk reads it
//! ("D151"), a normalized one's positions alone ("D152"), and none in a
//! parse, its statistics and a strict parse's check reading an escaped field
//! off its bytes ("D142"), a strict parse walking a nested, `json` or
//! `jsonb` value a part at a time ("D143"), and a strict check fed its field
//! over runs holding none of it ("D144"), a type read by its input grammar
//! alone among them.
//!
//! **This binary installs a global allocator tracking the bytes live**, and
//! their peak, which is why it is a test file of its own. A line past its
//! chunk is held in a mapping of its own, outside the allocator (D139), so
//! what a window's peak rises by is the decode's copies and the little a pass
//! allocates beside them. Linux only, where that mapping is.

#![cfg(target_os = "linux")]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow::array::{Array, AsArray, RecordBatch};
use common::Cold;
use futures::StreamExt;
use pgdt_codec::compare::comparison_for;
use pgdt_codec::internal::FieldWalk;
use pgdt_dump::cache::CacheMode;
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::PostgresInvalidValues;
use pgdt_query::{Expr, Predicate, PredicateOp, QueryOptions};
use tokio_util::sync::CancellationToken;

struct Live;

/// The bytes allocated and not yet freed.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The most [`LIVE`] has been since the last [`window`] opened.
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Live {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    // A grow is counted at both sizes at once, as the copy it may make holds
    // both.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        grew(new_size);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Live = Live;

/// The chunk every pass here reads in, far below each long field.
const CHUNK: usize = 64 << 10;

/// Open a window: the peak is reset to what is live, which is returned.
fn window() -> usize {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    live
}

/// A dump of `public.t (a integer, b <declared>)` whose second row's `b` is
/// `field`, as written to disk, between two short rows holding `short`.
fn dump(dir: &Path, declared: &str, short: &str, field: &[u8]) -> std::path::PathBuf {
    dump_after(dir, "", declared, short, field)
}

/// [`dump`], `preamble` written ahead of the table.
fn dump_after(
    dir: &Path,
    preamble: &str,
    declared: &str,
    short: &str,
    field: &[u8],
) -> std::path::PathBuf {
    let path = dir.join("long.sql");
    let mut file = format!(
        "{preamble}CREATE TABLE public.t (\n    a integer,\n    b {declared}\n);\n\n\
         COPY public.t (a, b) FROM stdin;\n1\t{short}\n2\t"
    )
    .into_bytes();
    file.extend_from_slice(field);
    file.extend_from_slice(format!("\n3\t{short}\n\\.\n\n").as_bytes());
    std::fs::write(&path, file).unwrap();
    path
}

fn source(path: &Path) -> Arc<dyn ByteRangeSource> {
    Arc::new(LocalFileSource::open(path).unwrap())
}

/// The batches a cold query of `public.t` over `path` reads, and how far
/// the allocator's peak rose past what was live as it began. Its span cap is
/// below the long line, so the line ends its batch, as one past the default
/// cap does: what a `bytea` batch's one buffer copies as it grows past a
/// value is the span's, which the replay is charged (`decisions.md`, "D84").
async fn query(path: &Path) -> (Vec<RecordBatch>, usize) {
    query_with(path, QueryOptions::default()).await
}

/// [`query`] as `options` ask, under its span cap.
async fn query_with(path: &Path, options: QueryOptions) -> (Vec<RecordBatch>, usize) {
    let scan = ScanOptions {
        read: ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() },
        ..ScanOptions::default()
    };
    let cold = Cold { scan, ..Cold::default() };
    let options = QueryOptions { max_source_span: Some(1 << 20), ..options };
    let before = window();
    let mut stream = cold.stream(source(path), "public.t", options).await.unwrap().unwrap();
    let mut batches = Vec::new();
    while let Some(batch) = stream.next().await {
        batches.push(batch.unwrap());
    }
    drop(stream);
    (batches, PEAK.load(Ordering::Relaxed) - before)
}

/// **An escaped text is held once beside its line**: a value of sixteen MiB
/// carrying an escape every few bytes is its column's block as its decode
/// wrote it, never copied into a block of the builder's.
#[tokio::test]
async fn an_escaped_text_is_one_copy() {
    let dir = tempfile::tempdir().unwrap();
    let long = 16 << 20;
    let value = "abcde\t".repeat(long / 6);
    let field = value.replace('\t', "\\t");
    let path = dump(dir.path(), "text", "short", field.as_bytes());

    let (batches, peak) = query(&path).await;
    let column = batches[0].column(1).as_string_view();
    assert_eq!(column.value(1), value);
    assert!(peak < field.len() + long / 4, "{peak} bytes allocated at the peak");
}

/// **A `bytea` is decoded into its column, nothing beside it**: a hex value
/// as `pg_dump` writes it, and one whose every digit `COPY` spells as an
/// escape of its own, each hold the value and little more, never its
/// unescaped text.
#[tokio::test]
async fn a_bytea_is_one_copy() {
    let dir = tempfile::tempdir().unwrap();
    for (bytes, spell) in [(8usize << 20, false), (2 << 20, true)] {
        let value: Vec<u8> = (0..bytes).map(|i| (i % 251) as u8).collect();
        let mut field = b"\\\\x".to_vec();
        for byte in &value {
            let pair = format!("{byte:02x}");
            if spell {
                field.extend(pair.bytes().flat_map(|digit| format!("\\x{digit:02x}").into_bytes()));
            } else {
                field.extend_from_slice(pair.as_bytes());
            }
        }
        let path = dump(dir.path(), "bytea", "\\\\x00", &field);

        let (batches, peak) = query(&path).await;
        let column = batches[0].column(1).as_binary::<i32>();
        assert_eq!(column.value(1), value, "spelled: {spell}");
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);
        assert!(peak < bytes + bytes / 4, "spelled: {spell}: {peak} bytes allocated at the peak");
    }
}

/// **A nested value is decoded into its builders, no element of it staged**
/// (`docs/design/decisions.md`, "D153"): a `bigint[]` of short elements
/// written in place and a `text[]` whose every element `COPY` writes with an
/// escape, each of seven or eight MiB, are read into their batch holding the
/// builders' reservation and no more, never the elements' text beside it or
/// the field unescaped: each is past its chunk term, so its builders are
/// reserved to it ("D154"), an element's slot for every two of its bytes.
#[tokio::test]
async fn a_nested_value_decodes_into_its_builders() {
    let dir = tempfile::tempdir().unwrap();
    let array = |element: &str, count: usize| {
        let mut text = "{".to_string();
        text.push_str(&format!("{element},").repeat(count - 1));
        text.push_str(element);
        text.push('}');
        text
    };
    // An `int8`'s eight bytes, and a `text`'s view's sixteen.
    let bigints = 4usize << 20;
    let texts = 1usize << 20;
    for (declared, text, count, slot) in [
        ("bigint[]", array("7", bigints), bigints, 8),
        ("text[]", array("\"a\tb\"", texts), texts, 16),
    ] {
        let field = pgdt_core::escape_field(Some(&text));
        let path = dump(dir.path(), declared, "{}", &field);
        let (batches, peak) = query(&path).await;
        let rows = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
        assert_eq!(rows, 3, "{declared}");
        let longest = batches
            .iter()
            .flat_map(|batch| {
                let list = batch.column(1).as_list::<i32>();
                (0..list.len()).map(|row| list.value_length(row)).collect::<Vec<_>>()
            })
            .max();
        assert_eq!(longest, Some(count as i32), "{declared}");
        let bound = field.len().div_ceil(2) * slot + (1 << 20);
        assert!(peak < bound, "{declared}: {peak} bytes allocated at the peak, past {bound}");
    }
}

/// **A nested value past its chunk term is decoded into builders reserved
/// to it** (`docs/design/decisions.md`, "D154"): a `bigint[]` of a power of
/// two of one-digit elements, between rows of two elements, is a batch of its
/// own and holds what its reservation states — eight bytes and a bit for every
/// two of its text — never the buffer its builder would outgrow by doubling
/// past the elements beside it, nor the outgrown one beside it.
#[tokio::test]
async fn a_long_nested_value_is_decoded_into_reserved_builders() {
    let dir = tempfile::tempdir().unwrap();
    let count = 4usize << 20;
    let text = format!("{{{}}}", vec!["7"; count].join(","));
    let path = dump(dir.path(), "bigint[]", "{1,2}", text.as_bytes());
    let (batches, peak) = query(&path).await;
    let sizes: Vec<usize> = batches.iter().map(RecordBatch::num_rows).collect();
    assert_eq!(sizes, [1, 1, 1]);
    assert_eq!(batches[1].column(1).as_list::<i32>().value_length(0), count as i32);
    let reserved = text.len().div_ceil(2) * 65 / 8;
    let bound = reserved + (1 << 20);
    assert!(peak < bound, "{peak} bytes allocated at the peak, past {bound}");
}

/// **A tier test stages nothing** (`docs/design/decisions.md`, "D153"): a
/// `date[]` of eight MiB in a block a short row's `infinity` counts is
/// tested and decoded by a cold query, the test and the census's count each a
/// walk of its own holding no element of it, so the query holds its
/// builders' reservation and no more — a `date`'s slot for every two bytes of
/// text — and a parse, counting and gathering it, nothing that grows with it.
#[tokio::test]
async fn a_tier_test_stages_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let long = 8usize << 20;
    let element = "2024-01-01";
    let count = long / (element.len() + 1);
    let text = format!("{{{}}}", vec![element; count].join(","));
    let path = dump(dir.path(), "date[]", "{infinity}", text.as_bytes());

    let (batches, peak) = query(&path).await;
    let lengths: Vec<Option<i32>> = batches
        .iter()
        .flat_map(|batch| {
            let list = batch.column(1).as_list::<i32>();
            (0..list.len())
                .map(|row| list.is_valid(row).then(|| list.value_length(row)))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(lengths, [None, Some(count as i32), None], "the infinities are NULL whole");
    let bound = text.len().div_ceil(2) * 33 / 8 + (1 << 20);
    assert!(peak < bound, "{peak} bytes allocated at the peak, past {bound}");

    for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
        let peak = parse(&path, invalid).await.unwrap();
        assert!(peak < long / 4, "{invalid:?}: {peak} bytes allocated at the peak");
    }
}

/// **A filter term holds no copy of an escaped field it reads**
/// (`docs/design/decisions.md`, "D150"): a `text` and a `character` of
/// eight MiB carrying an escape every few bytes, and a `bytea` of eight
/// MiB, each written as `pg_dump` writes it, are ordered, compared, listed
/// and tested for NULL by a query reading none of them into its batch,
/// with none of them allocated; the rows each keeps are the server's.
#[tokio::test]
async fn a_filter_term_holds_no_copy_of_an_escaped_field() {
    use PredicateOp::{Eq, Gt, In, IsNotNull, Lt, Ne};
    let dir = tempfile::tempdir().unwrap();
    let long = 8usize << 20;
    let text = "abcde\\t".repeat(long / 6);
    let mut bytea = b"\\\\x".to_vec();
    for i in 0..long {
        bytea.extend_from_slice(format!("{:02x}", i % 251).as_bytes());
    }
    let listed = |values: &[&str]| In(values.iter().map(|v| Some(v.to_string())).collect());
    let texts = [
        (Gt("abc".into()), 3),
        (Lt("abcde\tz".into()), 1),
        (Eq("short".into()), 2),
        (Ne("short".into()), 1),
        (listed(&["short", "x"]), 2),
        (IsNotNull, 3),
    ];
    let byteas = [
        (Lt("\\x0002".into()), 3),
        (Gt("\\x00".into()), 1),
        (Eq("\\x00".into()), 2),
        (listed(&["\\x00", "\\x01"]), 2),
    ];
    for (declared, short, field, filters) in [
        ("text", "short", text.as_bytes(), &texts[..]),
        ("character(10485760)", "short", text.as_bytes(), &texts[..]),
        ("bytea", "\\\\x00", bytea.as_slice(), &byteas[..]),
    ] {
        let path = dump(dir.path(), declared, short, field);
        for (op, rows) in filters {
            let options = QueryOptions {
                projection: Some(vec!["a".to_string()]),
                filter: Expr::Term(Predicate { column: "b".into(), op: op.clone() }),
                ..QueryOptions::default()
            };
            let (batches, peak) = query_with(&path, options).await;
            let kept = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
            assert_eq!(kept, *rows, "{declared} {op:?}");
            assert!(peak < long / 4, "{declared} {op:?}: {peak} bytes allocated at the peak");
        }
    }
}

/// **A nested term holds no tree of the field it orders**
/// (`docs/design/decisions.md`, "D151"): a `bigint[]` of many short
/// elements, and a `text[]` and a composite array of escaped elements, each
/// of eight MiB and written as `pg_dump` writes it, are ordered and compared
/// by a query reading none of them into its batch, with no key of them
/// built and no copy of them made; the rows each keeps are the server's.
#[tokio::test]
async fn a_nested_term_holds_no_tree() {
    use PredicateOp::{Eq, Gt, Lt, Ne};
    let dir = tempfile::tempdir().unwrap();
    let long = 8usize << 20;
    let array = |element: &str| {
        let mut text = "{".to_string();
        text.push_str(&format!("{element},").repeat(long / (element.len() + 1)));
        text.push_str(element);
        text.push('}');
        pgdt_core::escape_field(Some(&text))
    };
    let pair = "CREATE TYPE public.pair AS (\n    n integer,\n    s text\n);\n\n";
    let bigints = [(Gt("{1}".into()), 1), (Lt("{7,8}".into()), 3), (Eq("{1}".into()), 2)];
    let texts = [
        (Gt("{a}".into()), 1),
        (Lt(r#"{"ab\"cd",z}"#.into()), 3),
        (Eq("{a}".into()), 2),
        (Ne("{a}".into()), 1),
    ];
    let pairs = [(Gt(r#"{"(1,a)"}"#.into()), 1), (Eq(r#"{"(1,a)"}"#.into()), 2)];
    for (preamble, declared, short, field, filters) in [
        ("", "bigint[]", "{1}", array("7"), &bigints[..]),
        ("", "text[]", "{a}", array("\"ab\\\"c\td\""), &texts[..]),
        (pair, "public.pair[]", r#"{"(1,a)"}"#, array(r#""(1,\"ab\"\"cd\")""#), &pairs[..]),
    ] {
        let short = String::from_utf8(pgdt_core::escape_field(Some(short))).unwrap();
        let path = dump_after(dir.path(), preamble, declared, &short, &field);
        for (op, rows) in filters {
            let options = QueryOptions {
                projection: Some(vec!["a".to_string()]),
                filter: Expr::Term(Predicate { column: "b".into(), op: op.clone() }),
                ..QueryOptions::default()
            };
            let (batches, peak) = query_with(&path, options).await;
            let kept = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
            assert_eq!(kept, *rows, "{declared} {op:?}");
            assert!(peak < long / 4, "{declared} {op:?}: {peak} bytes allocated at the peak");
        }
    }
}

/// **A normalized term holds its positions and no tree**
/// (`docs/design/decisions.md`, "D152"): a `jsonb` object holding one of
/// many pairs whose keys carry an escape, written out of storage order as
/// `COPY` escapes it, and an `int4multirange` of many members written out
/// of order, each of eight MiB, are ordered and compared by a query reading
/// neither into its batch, holding no more than its key factor says beside
/// the line; the rows each keeps are the server's.
#[tokio::test]
async fn a_normalized_term_holds_its_positions() {
    use PredicateOp::{Eq, Gt, Lt};
    use pgdt_codec::compare::CompareKind;
    use pgdt_codec::internal::key_factor;
    let dir = tempfile::tempdir().unwrap();
    let long = 8usize << 20;
    let pairs = long / 16;
    let mut object = "{\"b\": 1, \"a\": {".to_string();
    for i in (1..=pairs).rev() {
        object.push_str(&format!("\"k\\t{i:07}\": 1, "));
    }
    object.push_str("\"k\\t0000000\": 1}}");
    let members = long / 18;
    let mut multirange = "{".to_string();
    for i in (1..=members).rev() {
        multirange.push_str(&format!("[{},{}),", 2 * i, 2 * i + 1));
    }
    multirange.push_str("[0,1)}");
    let short_object = r#"{"a": {}, "b": 1}"#;
    let objects = [
        (Gt(short_object.into()), 1),
        (Eq(short_object.into()), 2),
        (Lt(r#"{"a": {"k": 1}, "b": 1}"#.into()), 2),
    ];
    let ranges =
        [(Gt("{[0,1)}".into()), 1), (Eq("{[0,1)}".into()), 2), (Lt("{[0,1),[2,3)}".into()), 2)];
    let jsonb = key_factor(&CompareKind::Jsonb).unwrap();
    let multiranges = pgdt_codec::internal::nested_key_factor(&match comparison_for(
        "int4multirange",
        None,
        &[],
        &[],
    ) {
        pgdt_codec::compare::ComparisonPlan::Nested(plan) => plan,
        other => panic!("{other:?}"),
    })
    .unwrap();
    for (declared, short, field, factor, filters) in [
        ("jsonb", short_object, object, jsonb, &objects[..]),
        ("int4multirange", "{[0,1)}", multirange, multiranges, &ranges[..]),
    ] {
        let field = pgdt_core::escape_field(Some(&field));
        let short = String::from_utf8(pgdt_core::escape_field(Some(short))).unwrap();
        let path = dump(dir.path(), declared, &short, &field);
        let held = factor.of(field.len() as u64) as usize;
        for (op, rows) in filters {
            let options = QueryOptions {
                projection: Some(vec!["a".to_string()]),
                filter: Expr::Term(Predicate { column: "b".into(), op: op.clone() }),
                ..QueryOptions::default()
            };
            let (batches, peak) = query_with(&path, options).await;
            let kept = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
            assert_eq!(kept, *rows, "{declared} {op:?}");
            assert!(
                peak < held + long / 16,
                "{declared} {op:?}: {peak} bytes allocated at the peak"
            );
        }
    }
}

/// How far the allocator's peak rose past what was live as a whole-file
/// parse of `path` began, gathering statistics at the data level and
/// reading values as `invalid` says, or the parse's error.
async fn parse(path: &Path, invalid: PostgresInvalidValues) -> pgdt_core::Result<usize> {
    let opened = CacheMode::DISABLED.open_source(source(path)).await.unwrap();
    let read = ReadOptions { chunk_size_bytes: CHUNK, ..ReadOptions::default() };
    let before = window();
    let run = map_file(
        opened,
        read,
        &common::arranged(1),
        invalid,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await?;
    let peak = PEAK.load(Ordering::Relaxed) - before;
    assert!(run.index.is_complete(std::fs::metadata(path).unwrap().len()));
    Ok(peak)
}

/// **A strict parse checks a `bytea` holding none of it**: a field of
/// sixteen MiB is checked, and its statistics gathered, with no unescaped
/// text or decoded value of it allocated, the line past its chunk outside
/// the allocator.
#[tokio::test]
async fn a_strict_bytea_check_holds_no_copy() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = 8usize << 20;
    let mut field = b"\\\\x".to_vec();
    for i in 0..bytes {
        field.extend_from_slice(format!("{:02x}", i % 251).as_bytes());
    }
    let path = dump(dir.path(), "bytea", "\\\\x00", &field);

    let peak = parse(&path, PostgresInvalidValues::Strict).await.unwrap();
    assert!(peak < bytes / 4, "{peak} bytes allocated at the peak");
}

/// **A parse holds no copy of an escaped text**: a value of eight MiB
/// carrying an escape every few bytes is gathered, and checked against its
/// typmod by a strict parse, with none of it allocated; one past its typmod
/// is refused all the same.
#[tokio::test]
async fn a_parse_holds_no_copy_of_an_escaped_text() {
    let dir = tempfile::tempdir().unwrap();
    // `character varying`'s longest typmod, in characters.
    let typmod = 10_485_760usize;
    let escaped = |chars: usize| "abcde\\t".repeat(chars / 6);
    for declared in ["text".to_string(), format!("character varying({typmod})")] {
        let long = 8usize << 20;
        let path = dump(dir.path(), &declared, "short", escaped(long).as_bytes());
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            let peak = parse(&path, invalid).await.unwrap();
            assert!(peak < long / 4, "{declared}, {invalid:?}: {peak} bytes allocated at the peak");
        }
    }
    let past = escaped(typmod + (1 << 20));
    let path = dump(dir.path(), &format!("character varying({typmod})"), "short", past.as_bytes());
    let refused = parse(&path, PostgresInvalidValues::Strict).await.unwrap_err();
    assert!(matches!(refused, pgdt_core::Error::FieldRefused { .. }), "{refused:?}");
}

/// **A strict parse walks a nested, `json` or `jsonb` value holding no tree
/// of it**: an array of many short elements, an array and a composite of
/// escaped texts, and a `json` and a `jsonb` document of escaped strings,
/// each of eight MiB, are checked, and gathered, with none of them
/// allocated; one an element of which its type refuses is refused all the
/// same.
#[tokio::test]
async fn a_strict_parse_holds_no_tree_of_a_nested_value() {
    let dir = tempfile::tempdir().unwrap();
    let long = 8usize << 20;
    let repeat = |part: &str, closing: &str| {
        let mut field = part.repeat(long / part.len());
        field.push_str(closing);
        field
    };
    let pair = "CREATE TYPE public.pair AS (\n    n integer,\n    s text\n);\n\n";
    for (preamble, declared, short, field) in [
        ("", "smallint[]", "{1}", format!("{{{}}}", repeat("7,", "7"))),
        ("", "text[]", "{a}", format!("{{{}}}", repeat("\"ab\\\\\"cd\",", "x"))),
        (pair, "public.pair", "(1,a)", format!("(1,\"{}\")", repeat("ab\\\\\"cd", ""))),
        ("", "json", "1", format!("[{}]", repeat("\"a\\\\nb\",", "1"))),
        ("", "jsonb", "1", format!("[{}]", repeat("\"a\\\\nb\",", "1"))),
    ] {
        let path = dump_after(dir.path(), preamble, declared, short, field.as_bytes());
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            let peak = parse(&path, invalid).await.unwrap();
            assert!(peak < long / 4, "{declared}, {invalid:?}: {peak} bytes allocated at the peak");
        }
    }
    for (declared, short, field) in [
        ("smallint[]", "{1}", format!("{{{}}}", repeat("7,", "70000"))),
        ("jsonb", "1", format!("[{}]", repeat("\"a\\\\nb\",", "\"\\\\u0000\""))),
    ] {
        let path = dump(dir.path(), declared, short, field.as_bytes());
        let refused = parse(&path, PostgresInvalidValues::Strict).await.unwrap_err();
        assert!(
            matches!(refused, pgdt_core::Error::FieldRefused { .. }),
            "{declared}: {refused:?}"
        );
    }
}

/// The run a field is fed over in [`fed_over_runs`], far below each field.
const RUN: usize = 64 << 10;

/// Whether a strict check refuses the field a column declared `declared`
/// holds as `head`, `piece` `count` times and `tail`, unescaped, fed it as
/// `COPY` writes it in runs of [`RUN`] bytes, each made and dropped in its
/// turn as a line's chunks go by; and how far the allocator's peak rose past
/// what was live as the check began.
fn fed_over_runs(
    declared: &str,
    head: &str,
    piece: &str,
    count: usize,
    tail: &str,
) -> (bool, usize) {
    let plan = comparison_for(declared, None, &[], &[]);
    let escaped = |text: &str| pgdt_core::escape_field(Some(text));
    let (head, piece, tail) = (escaped(head), escaped(piece), escaped(tail));
    let mut bytes =
        head.iter().chain(std::iter::repeat_n(&piece, count).flatten()).chain(&tail).copied();
    let before = window();
    let mut walk = FieldWalk::new(&plan);
    loop {
        let run: Vec<u8> = bytes.by_ref().take(RUN).collect();
        if run.is_empty() {
            break;
        }
        walk.run(&run);
    }
    let refused = walk.finish();
    (refused, PEAK.load(Ordering::Relaxed) - before)
}

/// **A strict check fed its field over runs holds none of it**: an array of
/// many short elements, one of escaped texts, one whose unquoted element
/// holds a long whitespace run — alternating its bytes, interior, trailing,
/// and after `NULL` either way, a `json` element's refused where interior —
/// an array of ranges, a `jsonb` and a `json` of escaped strings, a `bytea`
/// and a long escaped text under its typmod, each of eight MiB, are checked
/// a run at a time with no more allocated than a run and the readers' own
/// state; an element its type refuses is refused all the same.
#[test]
fn a_strict_check_fed_over_runs_holds_no_field() {
    let long = 8usize << 20;
    let typmod = "character varying(10485760)";
    let typmod_array = format!("{typmod}[]");
    for (declared, head, piece, tail, refused) in [
        ("smallint[]", "{", "7,", "7}", false),
        ("smallint[]", "{", "7,", "70000}", true),
        ("text[]", "{", "\"ab\\\\\\\"c\td\",", "x}", false),
        (typmod_array.as_str(), "{a", " ", "b}", false),
        (typmod_array.as_str(), "{a", " \t", "b}", false),
        (typmod_array.as_str(), "{a", " \t", "}", false),
        (typmod_array.as_str(), "{NULL", " \t", "}", false),
        (typmod_array.as_str(), "{NULL", " \t", "b}", false),
        ("json[]", "{1", "\u{b}\t", "}", false),
        ("json[]", "{1", "\u{b}\t", "2}", true),
        ("int4range[]", "{", "\"[1,2)\",", "\"(,3]\"}", false),
        ("jsonb", "[", "\"a\\nb\",", "1]", false),
        ("jsonb", "[", "\"a\\nb\",", "\"\\u0000\"]", true),
        ("json", "[", "\"a\\nb\",", "1]", false),
        ("bytea", "\\x", "a1", "", false),
        ("bytea", "\\x", "a1", "z", true),
        (typmod, "", "ab\tc", "", false),
    ] {
        let count = long / piece.len();
        let (was_refused, peak) = fed_over_runs(declared, head, piece, count, tail);
        assert_eq!(was_refused, refused, "{declared} {piece:?}");
        assert!(peak < 4 * RUN, "{declared} {piece:?}: {peak} bytes allocated at the peak");
    }
}

/// **A strict check holds no text a grammar reads**: a `polygon[]` element
/// of many points and a `bit varying[]` element of many bits, each of eight
/// MiB, are checked a run at a time with no more allocated than a run and
/// the readers' own state; one its type refuses is refused all the same.
#[test]
fn a_strict_check_holds_no_text_a_grammar_reads() {
    let long = 8usize << 20;
    for (declared, head, piece, tail, refused) in [
        ("polygon[]", "{\"", "(1,2),", "(3,4)\"}", false),
        ("polygon[]", "{\"", "(1,2),", "(3,4\"}", true),
        ("polygon[]", "{\"", "(1,2),", "(3,4),\"}", true),
        ("bit varying[]", "{", "10", "1}", false),
        ("bit varying[]", "{", "10", "2}", true),
        ("bit varying(8)[]", "{", "10", "1}", true),
    ] {
        let count = long / piece.len();
        let (was_refused, peak) = fed_over_runs(declared, head, piece, count, tail);
        assert_eq!(was_refused, refused, "{declared} {tail:?}");
        assert!(peak < 4 * RUN, "{declared} {tail:?}: {peak} bytes allocated at the peak");
    }
}

/// **A strict parse reads a column its grammar reads off its bytes**: a
/// `path` of many points and a `bit varying` of many bits, each of eight
/// MiB and written with an escape every few bytes, are checked, and
/// gathered, with none of them allocated; one its type refuses is refused
/// all the same.
#[tokio::test]
async fn a_strict_parse_holds_no_text_of_a_column_a_grammar_reads() {
    let dir = tempfile::tempdir().unwrap();
    let long = 8usize << 20;
    let repeat = |piece: &str, closing: &str| {
        let mut field = piece.repeat(long / piece.len());
        field.push_str(closing);
        field
    };
    for (declared, short, field) in [
        ("path", "[(1,2)]", format!("[{}]", repeat("(1,\\t2),", "(3,4)"))),
        ("bit varying", "1", repeat("1\\x30", "1")),
    ] {
        let path = dump(dir.path(), declared, short, field.as_bytes());
        for invalid in [PostgresInvalidValues::Default, PostgresInvalidValues::Strict] {
            let peak = parse(&path, invalid).await.unwrap();
            assert!(peak < long / 4, "{declared}, {invalid:?}: {peak} bytes allocated at the peak");
        }
    }
    for (declared, short, field) in [
        ("path", "[(1,2)]", format!("[{}]", repeat("(1,\\t2),", "(3,4),"))),
        ("bit varying(8)", "1", repeat("1\\x30", "1")),
    ] {
        let path = dump(dir.path(), declared, short, field.as_bytes());
        let refused = parse(&path, PostgresInvalidValues::Strict).await.unwrap_err();
        assert!(
            matches!(refused, pgdt_core::Error::FieldRefused { .. }),
            "{declared}: {refused:?}"
        );
    }
}
