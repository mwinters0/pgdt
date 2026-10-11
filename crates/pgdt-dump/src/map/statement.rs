//! Where a statement ends: an incremental scan of its lines under the SQL
//! lexer's rules, so a statement the builder holds whole and an `INSERT` run
//! it holds as raw lines are both ended where the scanner's regions say they
//! can be.

use crate::lex::{Lexer, Region};

/// An incremental scan of a SQL statement's lines: how deep its parens are,
/// where [`crate::lex`] leaves it — the rules [`crate::scan::CopyScanner`]
/// lexes the same lines by, so a statement ends where the scanner's regions
/// say it can — whether its last line ends in a `--` comment, and what its
/// last non-whitespace byte was.
///
/// Line at a time, as the lexer is: [`crate::map`] holds every statement as
/// the lines the scanner surfaced. Byte-level rather than `char`-level: the
/// lexer tells a non-ASCII byte from an ASCII one and no further, and no ASCII
/// byte occurs inside a multi-byte UTF-8 sequence, so it answers alike over
/// raw bytes and over their lossy conversion, and an `INSERT` run is
/// classified off raw bytes with no `String` per line
/// (`docs/design/decisions.md`, "D33").
#[derive(Debug, Clone)]
pub(crate) struct StatementScan {
    lexer: Lexer,
    depth: i32,
    /// The last line fed ends in a `--` comment, which only the newline a
    /// further line brings would close.
    in_comment: bool,
    /// The last byte fed that is not [`is_sql_space`], or `0` if none is —
    /// the incremental form of `buf.trim_end()`'s last character.
    last_significant: u8,
    /// A non-empty line has been fed since the last [`reset`](Self::reset),
    /// so that [`is_empty`](Self::is_empty) answers what `buf.is_empty()`
    /// answered.
    fed: bool,
}

/// The whitespace `str::trim_end` strips, restricted to ASCII. A non-ASCII
/// Unicode space after a statement's `;` is therefore *not* trimmed here and
/// the statement reads as incomplete — `pg_dump` writes none, and the
/// consequence of being wrong is the graceful-degradation path
/// [`crate::map`] already takes for an unterminated statement.
const fn is_sql_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

impl StatementScan {
    /// A scan at a statement's start, under the dump's
    /// `standard_conforming_strings` (I50).
    pub(crate) const fn new(standard_strings: bool) -> Self {
        Self {
            lexer: Lexer::with_standard_strings(standard_strings),
            depth: 0,
            in_comment: false,
            last_significant: 0,
            fed: false,
        }
    }

    /// Start a fresh statement, under the same setting.
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.lexer.standard_strings());
    }

    /// Nothing has been fed since the last [`reset`](Self::reset) — the
    /// incremental equivalent of `buf.is_empty()`, and [`crate::map`]'s
    /// signal that the next line either opens a statement or ends the run.
    pub(crate) fn is_empty(&self) -> bool {
        !self.fed
    }

    /// Feed one more physical line, its newline stripped, as
    /// [`push_stmt_line`] joins them.
    pub(crate) fn feed_line(&mut self, line: &[u8]) {
        if line.is_empty() && !self.fed {
            return;
        }
        self.fed = true;
        if let Some(&b) = line.iter().rev().find(|&&b| !is_sql_space(b)) {
            self.last_significant = b;
        }
        let depth = &mut self.depth;
        // Deficiency register: `deficiency: KD9` — an `INSERT` run costs an
        // order of magnitude more than a `COPY` scan warm, and the device is
        // what decides whether a reader meets it (`measurements.md`,
        // `scan-throughput-warm` and `scan-throughput-nvme`). Three cuts
        // against the remainder are known. No `INSERT` statement's end
        // depends on `depth`, so a run-only scan would skip this count;
        // `insert_run_line` (`map.rs`) feeds the `INSERT INTO <table>` prefix
        // it has just matched, whose lexing is provably a run of code, so
        // those bytes are crossed twice; and `CopyScanner` has already lexed
        // every line it surfaces by these rules, so a line's region, comment
        // and paren count could travel with its `Event::Line` rather than be
        // found again here. **(b) owned by P8**, whose row reader is the
        // caller that can say the count is dead weight; it is not a licence
        // to drop `depth`, which `statement_complete` reads.
        let found = self.lexer.line(line, |code| {
            for k in memchr::memchr2_iter(b'(', b')', code) {
                *depth += if code[k] == b'(' { 1 } else { -1 };
            }
        });
        self.in_comment = found.line_comment;
    }

    /// Whether what has been fed so far is a complete SQL statement: parens
    /// balanced, between tokens rather than inside a literal, identifier or
    /// comment, and ending in `;`.
    pub(crate) fn complete(&self) -> bool {
        *self.lexer.region() == Region::Code
            && !self.in_comment
            && self.depth == 0
            && self.last_significant == b';'
    }

    /// Whether the lines so far end inside a literal, a quoted identifier or
    /// a `/* */` comment — the one case where a line that *looks* like a
    /// fresh boundary (starts with `--`, in [`crate::map`]'s case) is really
    /// that region's content spanning physical lines. `pg_dump` never emits
    /// a `--` comment inside a non-dollar-quoted statement's own parens, so
    /// paren depth does not gate this the same way.
    pub(crate) fn in_quote(&self) -> bool {
        *self.lexer.region() != Region::Code
    }

    fn feed_text(&mut self, buf: &str) {
        for line in buf.split('\n') {
            self.feed_line(line.as_bytes());
        }
    }
}

/// Whether `buf` (everything accumulated for a statement so far, its lines
/// joined by [`push_stmt_line`]) is a complete SQL statement —
/// [`StatementScan::complete`] over a buffer a caller holds whole rather than
/// feeding line by line. One implementation for both, [`crate::map`] deciding
/// the same question from a `String` for a DDL statement and from raw bytes
/// for an `INSERT` run.
pub(crate) fn statement_complete(buf: &str, standard_strings: bool) -> bool {
    let mut scan = StatementScan::new(standard_strings);
    scan.feed_text(buf);
    scan.complete()
}

/// [`StatementScan::in_quote`] over a whole buffer — see that method.
pub(crate) fn in_open_quote(buf: &str, standard_strings: bool) -> bool {
    let mut scan = StatementScan::new(standard_strings);
    scan.feed_text(buf);
    scan.in_quote()
}

/// Append `line` to a statement buffer being accumulated line by line,
/// joining with `\n` — but never a leading one before the buffer's first
/// line. Used by [`crate::map`]'s general statement scan, whose spans carry
/// the statement's own text; an `INSERT` run's does not, and feeds
/// [`StatementScan::feed_line`] instead.
pub(crate) fn push_stmt_line(buf: &mut String, line: &str) {
    if !buf.is_empty() {
        buf.push('\n');
    }
    buf.push_str(line);
}

#[cfg(test)]
mod tests;
