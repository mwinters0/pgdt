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
use std::collections::BTreeSet;

use pgdump_query::{
    DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES, ROW_GROUP_DEFAULT_SIZE_BYTES,
    SCAN_CHUNK_DEFAULT_SIZE_BYTES,
};

mod common;
use common::{VERSIONS, statistics_fixture};

/// One table's `COPY` block: its column names, and each row's fields with
/// `\N` read as `None`.
struct Block {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

impl Block {
    /// `table`'s first block in the `default` flag set's file.
    fn read(version: u32, table: &str) -> Block {
        Block::all(version, "default", table).remove(0)
    }

    /// Every block `flag_set`'s file writes for `table`, in file order: more
    /// than one only where a partitioned table is loaded via its root (I2).
    fn all(version: u32, flag_set: &str, table: &str) -> Vec<Block> {
        let path = statistics_fixture(version, flag_set);
        let text = std::fs::read_to_string(&path).unwrap();
        let header = format!("COPY {table} (");
        let mut lines = text.lines();
        let mut blocks = Vec::new();
        while let Some(header) = lines.by_ref().find(|line| line.starts_with(&header)) {
            let columns = header[header.find('(').unwrap() + 1..header.find(')').unwrap()]
                .split(", ")
                .map(str::to_string)
                .collect::<Vec<_>>();
            let rows = lines
                .by_ref()
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
            blocks.push(Block { columns, rows });
        }
        assert!(!blocks.is_empty(), "{}: no block for {table}", path.display());
        blocks
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
fn a_long_value_runs_past_the_read_chunk_the_group_size_and_the_stored_value_cap() {
    for version in VERSIONS {
        let block = Block::read(version, "public.long_value");
        let lengths = block.column("v").iter().map(|v| v.unwrap().len()).collect::<Vec<_>>();
        assert!(
            lengths.iter().any(|&len| len > SCAN_CHUNK_DEFAULT_SIZE_BYTES),
            "a value past the read chunk on {version}: {lengths:?}"
        );
        assert!(
            lengths.iter().any(|&len| len as u64 > ROW_GROUP_DEFAULT_SIZE_BYTES),
            "a value past the default group size on {version}: {lengths:?}"
        );
        assert!(
            lengths.iter().any(|&len| len > DICTIONARY_ENTRY_MAX_BYTES && len < SCAN_CHUNK_DEFAULT_SIZE_BYTES),
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

/// `name`'s values across every block of `blocks`, in file order.
fn across<'a>(blocks: &'a [Block], name: &str) -> Vec<Option<&'a str>> {
    blocks.iter().flat_map(|block| block.column(name)).collect()
}

fn distinct<'a>(values: &[Option<&'a str>]) -> BTreeSet<&'a str> {
    values.iter().flatten().copied().collect()
}

/// **One table, three blocks**: under `--load-via-partition-root` each
/// partition of `spans` is a `COPY public.spans` block of its own, written in
/// partition order, so an order a column holds inside a block it holds across
/// the boundaries too — or, for `local`, visibly does not. Without the flag the
/// same rows are three tables of one block each.
#[test]
fn a_partitioned_table_s_columns_hold_their_order_across_its_blocks() {
    for version in VERSIONS {
        let blocks = Block::all(version, "load-via-partition-root", "public.spans");
        assert_eq!(blocks.len(), 3, "spans' blocks on {version}");
        for (i, block) in blocks.iter().enumerate() {
            let part = distinct(&block.column("part"));
            assert_eq!(part, BTreeSet::from([(i + 1).to_string().as_str()]), "part on {version}");
        }
        let id = integers(&across(&blocks, "id"));
        assert!(id.windows(2).all(|w| w[0] < w[1]), "id ascends across blocks on {version}");
        let reversed = integers(&across(&blocks, "reversed"));
        assert!(reversed.windows(2).all(|w| w[0] > w[1]), "reversed across blocks on {version}");
        let label = across(&blocks, "label").into_iter().flatten().collect::<Vec<_>>();
        assert!(label.windows(2).all(|w| w[0] < w[1]), "label ascends bytewise on {version}");

        for block in &blocks {
            let local = integers(&block.column("local"));
            assert!(local.windows(2).all(|w| w[0] < w[1]), "local inside a block on {version}");
        }
        let local = integers(&across(&blocks, "local"));
        assert_eq!(order_flags(&local, i64::cmp), (false, false), "local across on {version}");

        let gappy = across(&blocks, "gappy");
        assert_eq!(gappy.iter().filter(|v| v.is_none()).count(), 1, "gappy on {version}");
        assert_eq!(order_flags(&integers(&gappy), i64::cmp), (true, false), "gappy on {version}");

        let per_table = (1..=3)
            .map(|p| Block::all(version, "default", &format!("public.spans_{p}")))
            .collect::<Vec<_>>();
        assert!(per_table.iter().all(|b| b.len() == 1), "a table per partition on {version}");
        let rows = per_table.iter().map(|b| b[0].rows.len()).sum::<usize>();
        assert_eq!(rows, id.len(), "the same rows either way on {version}");
    }
}

/// The distinct sets a count taken from dictionaries is checked against: one
/// every block's dictionary holds, one no block's does, one some blocks'
/// do, and one whose union is larger than any block's.
#[test]
fn the_low_cardinality_columns_hold_every_dictionary_shape_across_blocks() {
    for version in VERSIONS {
        let blocks = Block::all(version, "load-via-partition-root", "public.spans");
        let per_block = |name: &str| {
            blocks.iter().map(|block| distinct(&block.column(name)).len()).collect::<Vec<_>>()
        };
        assert_eq!(per_block("colour"), [4, 4, 4], "colour on {version}");
        assert_eq!(per_block("tint"), [3, 3, 3], "tint on {version}");
        assert_eq!(distinct(&across(&blocks, "tint")).len(), 9, "tint's union on {version}");
        assert!(
            per_block("wide").iter().all(|&n| n > DICTIONARY_MAX_ENTRIES),
            "wide overflows every block on {version}"
        );
        let mixed = per_block("mixed");
        assert!(
            mixed[..2].iter().all(|&n| n <= DICTIONARY_MAX_ENTRIES)
                && mixed[2] > DICTIONARY_MAX_ENTRIES,
            "mixed on {version}: {mixed:?}"
        );
        assert!(across(&blocks, "small").contains(&None), "small holds a NULL on {version}");
        assert_eq!(distinct(&across(&blocks, "flag")).len(), 2, "flag on {version}");

        // Two of the three instants were written from different offsets and
        // are one instant: the dump renders every value in one zone, so they
        // are one text.
        let stamp = distinct(&across(&blocks, "stamp"));
        assert_eq!(stamp.len(), 2, "stamp on {version}: {stamp:?}");

        let padded = distinct(&across(&blocks, "padded"));
        assert_eq!(padded, BTreeSet::from(["a   ", "b   "]), "padded on {version}");
        let bare = distinct(&across(&blocks, "bare"));
        assert_eq!(bare, BTreeSet::from(["a", "a ", "b"]), "bare on {version}");
    }
}

/// Each integer column's sum passes its own type, and `big`'s and `huge`'s
/// pass the widest type a sum of them is kept in.
#[test]
fn the_summed_columns_pass_their_types() {
    for version in VERSIONS {
        let blocks = Block::all(version, "load-via-partition-root", "public.spans");
        let sum =
            |name: &str| integers(&across(&blocks, name)).iter().map(|&v| i128::from(v)).sum();
        let i2: i128 = sum("i2");
        assert!(i2 > i128::from(i16::MAX), "i2 on {version}");
        let i4: i128 = sum("i4");
        assert!(i4 > i128::from(i32::MAX) && i4 < i128::from(i64::MAX), "i4 on {version}");
        let big: i128 = sum("big");
        assert!(big > i128::from(i64::MAX), "big on {version}");
        let ident: i128 = sum("ident");
        assert!(ident > i128::from(u32::MAX), "ident on {version}");

        let huge = across(&blocks, "huge")
            .into_iter()
            .flatten()
            .map(|v| v.parse::<i128>().unwrap())
            .try_fold(0i128, i128::checked_add);
        assert_eq!(huge, None, "huge's sum passes an i128 on {version}");
        let amount = across(&blocks, "amount");
        assert!(amount.iter().flatten().all(|v| v.contains('.')), "amount on {version}");
    }
}

/// Each `zeros` column's extreme is a zero, both zeros are present, and the
/// one seen first is the one its name says.
#[test]
fn both_zeros_sit_at_a_float_column_s_extreme_in_both_orders() {
    for version in VERSIONS {
        let block = Block::read(version, "public.zeros");
        for (name, extreme, first) in [
            ("min_pos_first", Ordering::Less, "0"),
            ("min_neg_first", Ordering::Less, "-0"),
            ("max_neg_first", Ordering::Greater, "-0"),
            ("max_pos_first", Ordering::Greater, "0"),
            ("r_min_pos_first", Ordering::Less, "0"),
            ("r_max_neg_first", Ordering::Greater, "-0"),
        ] {
            let values = block.column(name);
            let zeros =
                values.iter().flatten().filter(|v| ["0", "-0"].contains(v)).collect::<Vec<_>>();
            assert_eq!(zeros.len(), 2, "{name} holds both zeros on {version}");
            assert_eq!(*zeros[0], first, "{name}'s first zero on {version}");
            let floats = floats(&values);
            let other = floats.iter().find(|v| **v != 0.0).unwrap();
            assert_eq!(float_order(other, &0.0), extreme.reverse(), "{name} on {version}");
        }
    }
}

#[test]
fn a_text_minimum_is_longer_than_a_stored_value() {
    for version in VERSIONS {
        let block = Block::read(version, "public.long_min");
        for name in ["v", "vc"] {
            let values = block.column(name).into_iter().flatten().collect::<Vec<_>>();
            let min = values.iter().min_by(|a, b| a.as_bytes().cmp(b.as_bytes())).unwrap();
            assert!(min.len() > DICTIONARY_ENTRY_MAX_BYTES, "{name}'s minimum on {version}");
        }
    }
}

/// `moods.m` ascends in its labels' text, which is the order Arrow sorts an
/// enum by, and descends in the order its type declares them, with no NULL.
#[test]
fn an_enum_ascends_by_label_text_against_its_declared_order() {
    const DECLARED: [&str; 3] = ["sad", "ok", "happy"];
    for version in VERSIONS {
        let block = Block::read(version, "public.moods");
        let values = block.column("m");
        assert!(values.iter().all(Option::is_some), "m holds no NULL on {version}");
        let labels = values.into_iter().flatten().collect::<Vec<_>>();
        assert!(labels.iter().collect::<BTreeSet<_>>().len() > 1, "m varies on {version}");
        let bytewise = order_flags(&labels, |a, b| a.as_bytes().cmp(b.as_bytes()));
        assert_eq!(bytewise, (true, false), "m ascends by label text on {version}");
        let position = |label: &&str| DECLARED.iter().position(|d| d == label).unwrap();
        let declared = order_flags(&labels, |a, b| position(a).cmp(&position(b)));
        assert_eq!(declared, (false, true), "m descends in declared order on {version}");
    }
}
