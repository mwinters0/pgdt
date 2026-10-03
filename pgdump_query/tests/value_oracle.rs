//! Every typed value the `types` fixture holds, held to the server's own
//! reading of it (`docs/design/decisions.md`, "D73": the round trip holds that
//! render inverts decode, this that decode is right).
//!
//! `fixtures/<major>/oracle/values.tsv` is `scripts/value_oracle.py`'s: one
//! row per node of every column — the value, each array element, composite
//! field and range bound and flag — read through the type's send function or
//! the server's arithmetic, never its output spelling. Each `types` flag set
//! carrying DDL is read here column by column, typed, and every node whose
//! Arrow type is not text is compared to its row: a value the oracle reads
//! that Arrow's format spec cannot hold (an infinity, `NaN` under a typmod,
//! `24:00:00`, a timestamp past `i64` microseconds) must read as NULL, the
//! default mode's reading. Every flag set compares the columns `default`
//! does, so a column one of them reads as text is a failure too.
//!
//! A defect a flag set exposes is a strict exclusion naming its entry, as
//! `pgdump_query/tests/known_failures.rs` keeps them: the column is asserted
//! still to fail, so its fix fails this test until the fixing slice deletes
//! the row.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::{
    DataType, Date32Type, Decimal128Type, Decimal256Type, Float32Type, Float64Type, Int16Type,
    Int32Type, Int64Type, IntervalMonthDayNanoType, Time64MicrosecondType, TimeUnit,
    TimestampMicrosecondType, UInt32Type,
};
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::map::SpanBody;
use pgdump_query::resolve::SchemaMode;
use pgdump_query::{
    DataBlock, LocalFileSource, NestedPlan, QueryOptions, ScanOptions, build_map, table_stream,
};

mod common;
use common::{VERSIONS, fixtures_root, types_fixture};

/// Every `types` flag set a typed read can be asked of, `default` first: all
/// but the one carrying no DDL.
const FLAG_SETS: &[&str] = &[
    "default",
    "binary-upgrade",
    "quote-all-identifiers",
    "extra-float-digits-0",
    "bytea-output-escape",
    "timezone-monrovia",
];

/// The flag set holding rows and no `CREATE TABLE`, so nothing to type them by.
const NO_DDL: &str = "data-only";

/// A column a flag set reads wrongly by a filed defect, asserted to fail;
/// `flag_set` `None` is every one.
struct Exclusion {
    kd: &'static str,
    flag_set: Option<&'static str>,
    table: &'static str,
    column: &'static str,
}

const EXCLUSIONS: &[Exclusion] = &[Exclusion {
    kd: "KD2",
    flag_set: None,
    table: "public.t_composite_matrix",
    column: "v_tagged",
}];

impl Exclusion {
    fn applies(&self, flag_set: &str) -> bool {
        self.flag_set.is_none_or(|f| f == flag_set)
    }
}

/// How a float is held to its reading: bit for bit, or — where the flag set
/// writes `DBL_DIG` and `FLT_DIG` digits rather than the shortest exact
/// spelling — within one unit of the text's last digit.
#[derive(Clone, Copy)]
enum Floats {
    Exact,
    Digits,
}

fn floats(flag_set: &str) -> Floats {
    if flag_set == "extra-float-digits-0" { Floats::Digits } else { Floats::Exact }
}

/// The oracle's rows for one column: `(row, path)` to the reading.
type Readings = BTreeMap<(usize, String), Option<String>>;

/// `values.tsv` at `version`, by `(table, column)`.
fn oracle(version: u32) -> BTreeMap<(String, String), Readings> {
    let path = fixtures_root().join(version.to_string()).join("oracle/values.tsv");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} — regenerate the value oracle", path.display()));
    let mut out: BTreeMap<(String, String), Readings> = BTreeMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [table, column, row, path, _type, reading] = fields[..] else {
            panic!("{}: a row of six fields, not {line:?}", path.display());
        };
        let reading = (reading != "\\N").then(|| reading.to_string());
        out.entry((table.to_string(), column.to_string()))
            .or_default()
            .insert((row.parse().unwrap(), path.to_string()), reading);
    }
    out
}

/// What a typed read of one column produced: each node's reading in the
/// form the oracle's converts to, or the paths whose Arrow type is text.
#[derive(Default)]
struct Read {
    values: BTreeMap<(usize, String), (DataType, Option<String>)>,
    text: BTreeSet<(usize, String)>,
    /// The column's Arrow type and plan, which place a node the read holds
    /// none of.
    root: Option<(DataType, NestedPlan)>,
}

/// The tables `path`'s `COPY` blocks fill, and each one's columns.
async fn tables(path: &Path) -> BTreeMap<String, Vec<String>> {
    let source = LocalFileSource::open(path).unwrap();
    let spans = build_map(&source, &ScanOptions::default()).await.unwrap();
    let mut out = BTreeMap::new();
    for span in spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = span.body {
            out.insert(block.header.qualified_name(), block.header.columns.clone());
        }
    }
    out
}

/// One column of `table`, read typed alone: `Ok(None)` where it resolves to
/// text, `Err` where the read refuses.
async fn read_column(path: &Path, table: &str, column: &str) -> Result<Option<Read>, String> {
    let source = LocalFileSource::open(path).unwrap();
    let options = QueryOptions {
        schema_mode: SchemaMode::Typed,
        projection: Some(vec![column.to_string()]),
        ..Default::default()
    };
    let mut stream =
        table_stream(&source, table, ScanOptions::default(), options, None, CacheMode::DISABLED);
    let mut read = Read::default();
    let mut row = 0;
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        let resolved = stream.resolved_schema();
        let index = resolved.schema.index_of(column).unwrap();
        let plan = resolved.plans[index].clone();
        let array = batch.column(batch.schema().index_of(column).unwrap());
        if is_text(array.data_type()) {
            return Ok(None);
        }
        read.root = Some((array.data_type().clone(), plan.clone()));
        for i in 0..batch.num_rows() {
            row += 1;
            nodes(array, i, &plan, row, String::new(), &mut read);
        }
    }
    Ok(Some(read))
}

fn is_text(data_type: &DataType) -> bool {
    matches!(data_type, DataType::Utf8 | DataType::Utf8View | DataType::LargeUtf8 | DataType::Null)
        || matches!(data_type, DataType::Dictionary(_, value) if is_text(value))
}

/// Every node of `array[index]` under `plan`, in the oracle's addressing: a
/// container's own reading, then its children's — a multi-dimensional array's
/// elements flat, in `unnest`'s order.
fn nodes(
    array: &ArrayRef,
    index: usize,
    plan: &NestedPlan,
    row: usize,
    path: String,
    out: &mut Read,
) {
    let data_type = array.data_type().clone();
    if is_text(&data_type) {
        out.text.insert((row, path));
        return;
    }
    if array.is_null(index) {
        out.values.insert((row, path), (data_type, None));
        return;
    }
    match plan {
        NestedPlan::Array(_) | NestedPlan::Int2Vector | NestedPlan::Multirange(_) => {
            let lengths = dimensions(array, index, plan);
            out.values.insert((row, path.clone()), (data_type, Some(lengths)));
            let mut elements = Vec::new();
            flatten(array, index, plan, &mut elements);
            for (k, (values, j, element)) in elements.iter().enumerate() {
                nodes(values, *j, element, row, format!("{path}[{}]", k + 1), out);
            }
        }
        NestedPlan::Record(plans) => {
            out.values.insert((row, path.clone()), (data_type, Some("()".to_string())));
            let record = array.as_struct();
            for ((field, child), plan) in record.fields().iter().zip(record.columns()).zip(plans) {
                nodes(child, index, plan, row, format!("{path}.{}", field.name()), out);
            }
        }
        NestedPlan::Range(bound) => {
            out.values.insert((row, path.clone()), (data_type, Some("()".to_string())));
            let range = array.as_struct();
            for (field, child) in range.fields().iter().zip(range.columns()) {
                let bounded = field.name() == "lower" || field.name() == "upper";
                let plan = if bounded { bound.as_ref() } else { &NestedPlan::Scalar };
                nodes(child, index, plan, row, format!("{path}.{}", field.name()), out);
            }
        }
        NestedPlan::Scalar | NestedPlan::Decimal { .. } => {
            out.values.insert((row, path), (data_type.clone(), Some(scalar(array, index))));
        }
    }
}

/// An array's dimension lengths as `array_length` reads them, `x`-joined, or
/// `0` for an empty one; a multirange's or `int2vector`'s element count.
fn dimensions(array: &ArrayRef, index: usize, plan: &NestedPlan) -> String {
    let mut lengths = Vec::new();
    let (mut values, mut at, mut plan) = (array.clone(), index, plan);
    loop {
        let element = values.as_list::<i32>().value(at);
        if element.is_empty() {
            break;
        }
        lengths.push(element.len().to_string());
        match plan {
            NestedPlan::Array(inner) if matches!(inner.as_ref(), NestedPlan::Array(_)) => {
                (values, at, plan) = (element, 0, inner.as_ref());
            }
            _ => break,
        }
    }
    if lengths.is_empty() { "0".to_string() } else { lengths.join("x") }
}

/// The leaf elements of the list at `array[index]`, a nested `Array` plan
/// being a further dimension of the same array, each with its own plan.
fn flatten(
    array: &ArrayRef,
    index: usize,
    plan: &NestedPlan,
    out: &mut Vec<(ArrayRef, usize, NestedPlan)>,
) {
    let element = array.as_list::<i32>().value(index);
    for j in 0..element.len() {
        match plan {
            NestedPlan::Array(inner) if matches!(inner.as_ref(), NestedPlan::Array(_)) => {
                flatten(&element, j, inner, out);
            }
            NestedPlan::Array(inner) => out.push((element.clone(), j, inner.as_ref().clone())),
            NestedPlan::Int2Vector => out.push((element.clone(), j, NestedPlan::Scalar)),
            NestedPlan::Multirange(bound) => {
                out.push((element.clone(), j, NestedPlan::Range(bound.clone())));
            }
            _ => unreachable!("a list's plan"),
        }
    }
}

/// A scalar leaf's value in the oracle's converted form.
fn scalar(array: &ArrayRef, index: usize) -> String {
    use DataType as D;
    match array.data_type() {
        D::Int16 => format!("{:04x}", array.as_primitive::<Int16Type>().value(index)),
        D::Int32 => format!("{:08x}", array.as_primitive::<Int32Type>().value(index)),
        D::Int64 => format!("{:016x}", array.as_primitive::<Int64Type>().value(index)),
        D::UInt32 => format!("{:08x}", array.as_primitive::<UInt32Type>().value(index)),
        D::Boolean => format!("{:02x}", array.as_boolean().value(index) as u8),
        D::Float32 => format!("{:08x}", array.as_primitive::<Float32Type>().value(index).to_bits()),
        D::Float64 => {
            format!("{:016x}", array.as_primitive::<Float64Type>().value(index).to_bits())
        }
        D::Decimal128(_, s) => {
            format!("{} {s}", array.as_primitive::<Decimal128Type>().value(index))
        }
        D::Decimal256(_, s) => {
            format!("{} {s}", array.as_primitive::<Decimal256Type>().value(index))
        }
        D::Date32 => array.as_primitive::<Date32Type>().value(index).to_string(),
        D::Timestamp(TimeUnit::Microsecond, _) => {
            array.as_primitive::<TimestampMicrosecondType>().value(index).to_string()
        }
        D::Time64(TimeUnit::Microsecond) => {
            array.as_primitive::<Time64MicrosecondType>().value(index).to_string()
        }
        D::Interval(_) => {
            let v = array.as_primitive::<IntervalMonthDayNanoType>().value(index);
            format!("{} {} {}", v.months, v.days, v.nanoseconds)
        }
        D::FixedSizeBinary(_) => hex(array.as_fixed_size_binary().value(index)),
        D::Binary => hex(array.as_binary::<i32>().value(index)),
        other => panic!("{other}: a leaf type this test does not read — teach `scalar` it"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const SPECIALS: [&str; 5] = ["infinity", "-infinity", "NaN", "Infinity", "-Infinity"];

/// The oracle's `reading` as a node of `data_type` holds it, in [`scalar`]'s
/// form; `None` where Arrow's format spec cannot hold it.
fn expected(reading: &str, data_type: &DataType) -> Option<String> {
    use DataType as D;
    let special = SPECIALS.contains(&reading);
    match data_type {
        D::Date32 if !special => {
            let julian: i64 = reading.parse().unwrap();
            i32::try_from(julian - 2_440_588).ok().map(|d| d.to_string())
        }
        D::Timestamp(TimeUnit::Microsecond, _) if !special => {
            let micros: i128 = reading.parse().unwrap();
            i64::try_from(micros).ok().map(|m| m.to_string())
        }
        D::Time64(TimeUnit::Microsecond) => {
            let micros: i64 = reading.parse().unwrap();
            (0..86_400_000_000).contains(&micros).then(|| micros.to_string())
        }
        D::Interval(_) if !special => {
            let parts: Vec<i64> = reading.split(' ').map(|p| p.parse().unwrap()).collect();
            let nanos = parts[2].checked_mul(1000)?;
            Some(format!("{} {} {nanos}", parts[0], parts[1]))
        }
        D::Decimal128(p, s) | D::Decimal256(p, s) if !special => decimal(reading, *p, *s),
        D::Date32 | D::Timestamp(..) | D::Interval(_) | D::Decimal128(..) | D::Decimal256(..) => {
            None
        }
        _ => Some(reading.to_string()),
    }
}

/// `unscaled scale` at the column's scale `s`, or `None` past precision `p`.
fn decimal(reading: &str, p: u8, s: i8) -> Option<String> {
    let (unscaled, scale) = reading.split_once(' ').unwrap();
    let scale: i32 = scale.parse().unwrap();
    let (negative, mut digits) = match unscaled.strip_prefix('-') {
        Some(rest) => (true, rest.to_string()),
        None => (false, unscaled.to_string()),
    };
    let shift = i32::from(s) - scale;
    if shift >= 0 {
        digits.push_str(&"0".repeat(shift as usize));
    } else {
        let keep = digits.len().saturating_sub((-shift) as usize);
        assert!(digits[keep..].bytes().all(|b| b == b'0'), "{reading} rounds at scale {s}");
        digits.truncate(keep);
    }
    let digits = digits.trim_start_matches('0');
    if digits.len() > usize::from(p) {
        return None;
    }
    let digits = if digits.is_empty() { "0" } else { digits };
    let sign = if negative && digits != "0" { "-" } else { "" };
    Some(format!("{sign}{digits} {s}"))
}

/// Whether a read value agrees with what the oracle's reading converts to.
fn agrees(expect: Option<&str>, found: Option<&str>, data_type: &DataType, floats: Floats) -> bool {
    match (expect, found, floats, data_type) {
        (Some(e), Some(f), Floats::Digits, DataType::Float64) => within(
            f64::from_bits(u64::from_str_radix(e, 16).unwrap()),
            f64::from_bits(u64::from_str_radix(f, 16).unwrap()),
            15,
        ),
        (Some(e), Some(f), Floats::Digits, DataType::Float32) => within(
            f64::from(f32::from_bits(u32::from_str_radix(e, 16).unwrap())),
            f64::from(f32::from_bits(u32::from_str_radix(f, 16).unwrap())),
            6,
        ),
        _ => expect == found,
    }
}

/// `found` within one unit of the last of `digits` significant digits of
/// `exact`: the text's precision, and the nearest binary value to it.
fn within(exact: f64, found: f64, digits: i32) -> bool {
    if exact.is_nan() || found.is_nan() {
        return exact.is_nan() && found.is_nan();
    }
    if exact.is_infinite() || found.is_infinite() || exact == 0.0 {
        return exact.to_bits() == found.to_bits();
    }
    // Two factors, since `powi` of one exponent past `-308` underflows to zero
    // where the unit itself is a subnormal.
    let unit = 10f64.powi(exact.abs().log10().floor() as i32) * 10f64.powi(1 - digits);
    (exact - found).abs() <= unit
}

/// The readings a read can hold: a value holding a leaf its Arrow type
/// cannot hold is one as a whole (`docs/design/decisions.md`, "D96"), so it
/// reads NULL with no node beneath it.
fn held(readings: &Readings, read: &Read) -> Readings {
    let whole: BTreeSet<usize> = readings
        .iter()
        .filter(|((_, path), reading)| {
            let Some(reading) = reading else { return false };
            let Some((root, plan)) = &read.root else { return false };
            let Some(data_type) = type_at(root, plan, path) else { return false };
            !path.is_empty() && expected(reading, &data_type).is_none()
        })
        .map(|((row, _), _)| *row)
        .collect();
    readings
        .iter()
        .filter(|((row, path), _)| !whole.contains(row) || path.is_empty())
        .map(|((row, path), reading)| {
            let reading = if whole.contains(row) { None } else { reading.clone() };
            ((*row, path.clone()), reading)
        })
        .collect()
}

/// The Arrow type of the node at `path` beneath a column of `data_type`
/// under `plan`, in [`nodes`]' addressing; `None` beneath a text node.
fn type_at(data_type: &DataType, plan: &NestedPlan, path: &str) -> Option<DataType> {
    if path.is_empty() {
        return Some(data_type.clone());
    }
    if let Some(rest) = path.strip_prefix('[') {
        let rest = &rest[rest.find(']')? + 1..];
        let (element, plan) = element_of(data_type, plan)?;
        return type_at(&element, &plan, rest);
    }
    let rest = path.strip_prefix('.')?;
    let end = rest.find(['.', '[']).unwrap_or(rest.len());
    let (name, rest) = rest.split_at(end);
    let DataType::Struct(fields) = data_type else { return None };
    let index = fields.iter().position(|f| f.name() == name)?;
    let plan = match plan {
        NestedPlan::Record(plans) => plans[index].clone(),
        NestedPlan::Range(bound) if name == "lower" || name == "upper" => bound.as_ref().clone(),
        _ => NestedPlan::Scalar,
    };
    type_at(fields[index].data_type(), &plan, rest)
}

/// A list's element type and plan, a further dimension of the same array
/// read through to its leaves, as [`flatten`] reads them.
fn element_of(data_type: &DataType, plan: &NestedPlan) -> Option<(DataType, NestedPlan)> {
    let DataType::List(field) = data_type else { return None };
    let element = field.data_type().clone();
    match plan {
        NestedPlan::Array(inner) if matches!(inner.as_ref(), NestedPlan::Array(_)) => {
            element_of(&element, inner)
        }
        NestedPlan::Array(inner) => Some((element, inner.as_ref().clone())),
        NestedPlan::Int2Vector => Some((element, NestedPlan::Scalar)),
        NestedPlan::Multirange(bound) => Some((element, NestedPlan::Range(bound.clone()))),
        _ => None,
    }
}

/// Whether `key` is a text node or beneath one: a field the read holds as
/// text has no children for the oracle's to meet.
fn under_text(key: &(usize, String), text: &BTreeSet<(usize, String)>) -> bool {
    let (row, path) = key;
    text.iter().any(|(r, p)| {
        r == row
            && path.strip_prefix(p.as_str()).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with('[') || rest.starts_with('.')
            })
    })
}

/// One column's comparison: `Ok(true)` compared and agreeing, `Ok(false)` read
/// as text, `Err` saying what disagrees.
fn compare(
    read: Result<Option<Read>, String>,
    readings: Option<&Readings>,
    floats: Floats,
) -> Result<bool, String> {
    let Some(read) = read? else { return Ok(false) };
    let empty = Readings::new();
    let readings = held(readings.unwrap_or(&empty), &read);
    let mut wrong = Vec::new();
    for (key, reading) in &readings {
        if under_text(key, &read.text) {
            continue;
        }
        let Some((data_type, found)) = read.values.get(key) else {
            wrong.push(format!(
                "row {} {:?}: the oracle reads {reading:?}, the read has no such node",
                key.0, key.1
            ));
            continue;
        };
        let expect = reading.as_deref().and_then(|r| expected(r, data_type));
        if !agrees(expect.as_deref(), found.as_deref(), data_type, floats) {
            wrong.push(format!(
                "row {} {:?} ({data_type}): the oracle reads {reading:?}, so {expect:?}; the read has {found:?}",
                key.0, key.1
            ));
        }
    }
    for key in read.values.keys() {
        if !readings.contains_key(key) {
            wrong.push(format!("row {} {:?}: a typed node the oracle does not read", key.0, key.1));
        }
    }
    if wrong.is_empty() { Ok(true) } else { Err(wrong.join("\n    ")) }
}

#[tokio::test]
async fn every_typed_value_is_the_one_the_server_reads() {
    let tree: BTreeSet<String> =
        std::fs::read_dir(types_fixture(VERSIONS[0], "default").parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path().file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
    let named: BTreeSet<String> =
        FLAG_SETS.iter().chain([&NO_DDL]).map(|s| s.to_string()).collect();
    assert_eq!(tree, named, "a `types` flag set this test does not name, or one it names gone");

    let mut failures = Vec::new();
    for version in VERSIONS {
        let oracle = oracle(version);
        let mut typed_in_default: BTreeMap<(String, String), bool> = BTreeMap::new();
        for &flag_set in FLAG_SETS {
            let path = types_fixture(version, flag_set);
            let tables = tables(&path).await;
            for (table, _) in oracle.keys() {
                assert!(
                    tables.contains_key(table),
                    "{version}: the oracle reads {table}, which {flag_set} does not hold"
                );
            }
            for exclusion in EXCLUSIONS.iter().filter(|x| x.applies(flag_set)) {
                let columns = tables.get(exclusion.table);
                assert!(
                    columns.is_some_and(|c| c.iter().any(|c| c == exclusion.column)),
                    "{}: {version} {flag_set} holds no {}.{}",
                    exclusion.kd,
                    exclusion.table,
                    exclusion.column
                );
            }
            for (table, columns) in &tables {
                for column in columns {
                    let key = (table.clone(), column.clone());
                    let read = read_column(&path, table, column).await;
                    let mut verdict = compare(read, oracle.get(&key), floats(flag_set));
                    if flag_set == "default" {
                        if let Ok(typed) = verdict {
                            typed_in_default.insert(key.clone(), typed);
                        }
                    } else if let Ok(typed) = verdict {
                        let default = typed_in_default.get(&key).copied().unwrap_or(false);
                        if typed != default {
                            verdict = Err(format!("typed {typed} here, {default} in default"));
                        }
                    }
                    let excluded = EXCLUSIONS
                        .iter()
                        .find(|x| x.applies(flag_set) && x.table == table && x.column == column);
                    match (verdict, excluded) {
                        (Ok(_), None) | (Err(_), Some(_)) => {}
                        (Err(why), None) => {
                            failures
                                .push(format!("{version} {flag_set} {table}.{column}:\n    {why}"));
                        }
                        (Ok(_), Some(x)) => failures.push(format!(
                            "{version} {flag_set} {table}.{column} now agrees with the oracle — \
                             {}'s fix deletes its exclusion",
                            x.kd
                        )),
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} disagreements:\n{}", failures.len(), failures.join("\n"));
}
