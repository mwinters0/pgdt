//! The server's answers beside a dump: what a restore of it holds per table,
//! read with its descendants and `ONLY` (`docs/design/decisions.md`,
//! "D130"), which a table read with its descendants is held to — and, over a
//! TimescaleDB restore, each hypertable's root as its restore holds it, which
//! the server's own reads leave out (TS8).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How the server read a table: with its descendants, as `SELECT … FROM t`,
/// or `ONLY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reading {
    Descendants,
    Only,
}

/// The server's answer for one table read one way: its rows as `COPY` text,
/// sorted, or the error it refused the read with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Rows(Vec<String>),
    Error(String),
}

/// What a restore of one dump answered: per table (`schema.table`) and
/// reading, and the errors the restore itself met.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    pub tables: BTreeMap<(String, Reading), Answer>,
    /// Per hypertable, the rows its root holds itself, read `ONLY` with
    /// TimescaleDB's planner inactive: the generator's `held` lines.
    pub held: BTreeMap<String, Answer>,
    pub restore_errors: Vec<String>,
}

/// Which answer a line adds to: a table read one way, or a root as held.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Read(String, Reading),
    Held(String),
}

impl Answers {
    fn slot(&mut self, key: &Key) -> Option<&mut Answer> {
        match key {
            Key::Read(table, reading) => self.tables.get_mut(&(table.clone(), *reading)),
            Key::Held(table) => self.held.get_mut(table),
        }
    }

    fn insert(&mut self, key: Key, answer: Answer) {
        match key {
            Key::Read(table, reading) => self.tables.insert((table, reading), answer),
            Key::Held(table) => self.held.insert(table, answer),
        };
    }
}

/// The answers file beside `dump`: `<flag-set>.answers.tsv` for
/// `<flag-set>.sql`.
pub fn answers_path(dump: &Path) -> PathBuf {
    dump.with_extension("answers.tsv")
}

/// The answers beside `dump`, parsed; a `rows <n>` line checked against the
/// `row` lines that follow it.
pub fn answers(dump: &Path) -> Answers {
    let path = answers_path(dump);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no answers at {}: {e}", path.display()));
    let mut answers = Answers::default();
    let mut counts: BTreeMap<Key, usize> = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.splitn(4, '\t');
        let (table, reading, kind) = (fields.next(), fields.next(), fields.next());
        let rest = fields.next().unwrap_or_default();
        let (Some(table), Some(reading), Some(kind)) = (table, reading, kind) else {
            panic!("{}: malformed line {line:?}", path.display());
        };
        if kind == "restore-error" {
            answers.restore_errors.push(rest.to_string());
            continue;
        }
        let table = table.to_string();
        let key = match reading {
            "descendants" => Key::Read(table, Reading::Descendants),
            "only" => Key::Read(table, Reading::Only),
            "held" => Key::Held(table),
            other => panic!("{}: unknown reading {other:?}", path.display()),
        };
        match kind {
            "rows" => {
                counts.insert(key.clone(), rest.parse().unwrap());
                answers.insert(key, Answer::Rows(Vec::new()));
            }
            "row" => match answers.slot(&key) {
                Some(Answer::Rows(rows)) => rows.push(rest.to_string()),
                _ => panic!("{}: a row before its count: {line:?}", path.display()),
            },
            "error" => answers.insert(key, Answer::Error(rest.to_string())),
            other => panic!("{}: unknown line kind {other:?}", path.display()),
        }
    }
    for (key, count) in counts {
        let Some(Answer::Rows(rows)) = answers.slot(&key) else { unreachable!() };
        let rows = rows.len();
        assert_eq!(rows, count, "{}: {key:?}'s rows against its count", path.display());
    }
    answers
}
