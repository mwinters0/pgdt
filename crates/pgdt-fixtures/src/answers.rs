//! The server's answers beside a dump: what a restore of it holds per table,
//! read with its descendants and `ONLY` (`docs/design/decisions.md`,
//! "D130"), which a table read with its descendants is held to.

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
    pub restore_errors: Vec<String>,
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
    let mut counts: BTreeMap<(String, Reading), usize> = BTreeMap::new();
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
        let reading = match reading {
            "descendants" => Reading::Descendants,
            "only" => Reading::Only,
            other => panic!("{}: unknown reading {other:?}", path.display()),
        };
        let key = (table.to_string(), reading);
        match kind {
            "rows" => {
                counts.insert(key.clone(), rest.parse().unwrap());
                answers.tables.insert(key, Answer::Rows(Vec::new()));
            }
            "row" => match answers.tables.get_mut(&key) {
                Some(Answer::Rows(rows)) => rows.push(rest.to_string()),
                _ => panic!("{}: a row before its count: {line:?}", path.display()),
            },
            "error" => {
                answers.tables.insert(key, Answer::Error(rest.to_string()));
            }
            other => panic!("{}: unknown line kind {other:?}", path.display()),
        }
    }
    for (key, count) in counts {
        let Some(Answer::Rows(rows)) = answers.tables.get(&key) else { unreachable!() };
        assert_eq!(rows.len(), count, "{}: {key:?}'s rows against its count", path.display());
    }
    answers
}
