//! The `statistics` fixture holds the shapes its schema says it does, on every
//! major.
//!
//! Per-row-group statistics are tested against these columns: a bound, a
//! dictionary, a null count and a block's sortedness are each right or wrong
//! against what the file holds. So what the file holds is asserted here, on the
//! bytes `pg_dump` wrote, rather than taken from the schema's comments — a
//! column believed ascending that is not would make every sortedness test that
//! reads it vacuous (`docs/design/decisions.md`, "D73").
//!
//! **Read as raw `COPY` text, not through the library.** The subject is the
//! fixture, and a scanner under test is not the instrument for checking its own
//! input. No value in the schema needs escaping, so a field is its value.
//!
//! The two collation claims — `c_text` out of order and `default_text` in order
//! under the database's default collation — are the server's, and the schema
//! asks the server while generating; what is asserted here is their bytewise
//! half.

use std::cmp::Ordering;

use pgdump_query::DEFAULT_CHUNK_SIZE;

mod common;
use common::{VERSIONS, statistics_fixture};

/// The stored-value cap bounds and dictionary entries share, which the long
/// values exist to exceed.
const STORED_VALUE_CAP: usize = 256;

/// One table's `COPY` block: its column names, and each row's fields with
/// `\N` read as `None`.
struct Block {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

impl Block {
    fn read(version: u32, table: &str) -> Block {
        let path = statistics_fixture(version, "default");
        let text = std::fs::read_to_string(&path).unwrap();
        let header = format!("COPY {table} (");
        let mut lines = text.lines().skip_while(|line| !line.starts_with(&header));
        let header =
            lines.next().unwrap_or_else(|| panic!("{}: no block for {table}", path.display()));
        let columns = header[header.find('(').unwrap() + 1..header.find(')').unwrap()]
            .split(", ")
            .map(str::to_string)
            .collect::<Vec<_>>();
        let rows = lines
            .take_while(|line| *line != "\\.")
            .map(|line| {
                let fields = line
                    .split('\t')
                    .map(|field| (field != "\\N").then(|| field.to_string()))
                    .collect::<Vec<_>>();
                assert_eq!(fields.len(), columns.len(), "{table} on {version}");
                fields
            })
            .collect();
        Block { columns, rows }
    }

    fn column(&self, name: &str) -> Vec<Option<&str>> {
        let at = self.columns.iter().position(|c| c == name).unwrap_or_else(|| panic!("no {name}"));
        self.rows.iter().map(|row| row[at].as_deref()).collect()
    }
}

/// Whether consecutive non-null values never decrease, and never increase,
/// under `cmp` — the two running flags a block's sortedness is settled by.
fn order_flags<T>(values: &[T], cmp: impl Fn(&T, &T) -> Ordering) -> (bool, bool) {
    let never_decreases = values.windows(2).all(|w| cmp(&w[0], &w[1]) != Ordering::Greater);
    let never_increases = values.windows(2).all(|w| cmp(&w[0], &w[1]) != Ordering::Less);
    (never_decreases, never_increases)
}

fn integers(values: &[Option<&str>]) -> Vec<i64> {
    values.iter().flatten().map(|v| v.parse().unwrap()).collect()
}

/// PostgreSQL's float order: `NaN` above everything, `-0` equal to `0`.
fn float_order(a: &f64, b: &f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(b).unwrap(),
    }
}

fn floats(values: &[Option<&str>]) -> Vec<f64> {
    values.iter().flatten().map(|v| v.parse().unwrap()).collect()
}

#[test]
fn the_integer_columns_hold_every_order_shape() {
    for version in VERSIONS {
        let block = Block::read(version, "public.ordered");
        assert!(block.rows.len() > 64, "ordered on {version} holds too few rows to fill a group");
        let flags = |name: &str| order_flags(&integers(&block.column(name)), i64::cmp);

        let id = integers(&block.column("id"));
        assert!(id.windows(2).all(|w| w[0] < w[1]), "id strictly ascending on {version}");
        assert_eq!(flags("reversed"), (false, true), "reversed on {version}");
        let reversed = integers(&block.column("reversed"));
        assert!(reversed.windows(2).all(|w| w[0] > w[1]), "reversed strictly on {version}");

        let stepped = integers(&block.column("stepped"));
        assert_eq!(flags("stepped"), (true, false), "stepped on {version}");
        assert!(stepped.windows(2).any(|w| w[0] == w[1]), "stepped has equal neighbours");

        assert_eq!(flags("unsorted"), (false, false), "unsorted on {version}");

        assert_eq!(flags("constant"), (true, true), "constant on {version}");
        assert!(block.column("constant").iter().all(Option::is_some), "constant has no NULL");

        assert!(block.column("all_null").iter().all(Option::is_none), "all_null on {version}");

        let gappy = block.column("gappy");
        assert_eq!(flags("gappy"), (true, false), "gappy's non-null values on {version}");
        let first = gappy.iter().position(Option::is_some).unwrap();
        let last = gappy.iter().rposition(Option::is_some).unwrap();
        assert!(
            gappy[first..last].iter().any(Option::is_none),
            "gappy has a NULL between non-null values on {version}"
        );

        assert_eq!(integers(&block.column("single")).len(), 1, "single on {version}");
    }
}

#[test]
fn the_text_columns_hold_both_cardinalities_and_both_collation_orders() {
    for version in VERSIONS {
        let block = Block::read(version, "public.ordered");
        let distinct = |name: &str| {
            let mut values = block.column(name);
            assert!(values.iter().all(Option::is_some), "{name} has no NULL on {version}");
            values.sort();
            values.dedup();
            values.len()
        };
        let low = distinct("low_card");
        assert!((2..=64).contains(&low), "low_card has {low} distinct values on {version}");
        let high = distinct("high_card");
        assert!(high > 64, "high_card has {high} distinct values on {version}");

        let bytewise = |name: &str| {
            let values = block.column(name).into_iter().flatten().collect::<Vec<_>>();
            order_flags(&values, |a, b| a.as_bytes().cmp(b.as_bytes()))
        };
        assert_eq!(bytewise("c_text"), (true, false), "c_text on {version}");
        assert_eq!(bytewise("default_text"), (false, false), "default_text on {version}");
    }
}

#[test]
fn the_special_values_are_present_and_ordered_as_postgresql_orders_them() {
    for version in VERSIONS {
        let block = Block::read(version, "public.specials");

        let f8 = block.column("f8");
        for special in ["NaN", "Infinity", "-Infinity", "-0", "0"] {
            assert!(f8.contains(&Some(special)), "f8 holds {special} on {version}");
        }
        assert_eq!(order_flags(&floats(&f8), float_order), (true, false), "f8 on {version}");
        let f4 = block.column("f4");
        assert!(f4.contains(&Some("NaN")) && f4.contains(&Some("-0")), "f4 on {version}");
        assert_eq!(order_flags(&floats(&f4), float_order), (true, false), "f4 on {version}");
        let unsorted = floats(&block.column("f8_unsorted"));
        assert_eq!(order_flags(&unsorted, float_order), (false, false), "f8_unsorted on {version}");

        // Equal as numbers, different as text: the pair a dictionary looked up
        // bytewise would get wrong.
        let n = block.column("n");
        for spelling in ["1.5", "1.50", "1.500", "NaN"] {
            assert!(n.contains(&Some(spelling)), "n holds {spelling} on {version}");
        }
        assert!(n.contains(&None), "n holds a NULL on {version}");
    }
}

#[test]
fn a_long_value_runs_past_the_read_chunk_and_the_stored_value_cap() {
    for version in VERSIONS {
        let block = Block::read(version, "public.long_value");
        let lengths = block.column("v").iter().map(|v| v.unwrap().len()).collect::<Vec<_>>();
        assert!(
            lengths.iter().any(|&len| len > DEFAULT_CHUNK_SIZE),
            "a value past the read chunk on {version}: {lengths:?}"
        );
        assert!(
            lengths.iter().any(|&len| len > STORED_VALUE_CAP && len < DEFAULT_CHUNK_SIZE),
            "a value past the cap and short of the chunk on {version}: {lengths:?}"
        );
        let v = block.column("v").into_iter().flatten().collect::<Vec<_>>();
        assert_eq!(
            order_flags(&v, |a, b| a.as_bytes().cmp(b.as_bytes())),
            (true, false),
            "v ascending bytewise on {version}"
        );
    }
}
