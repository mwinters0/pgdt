//! What `pgdq --help` and `pgdq <command> -h` actually render.
//!
//! A flag's help *is* its doc comment here (`docs/design/decisions.md`,
//! "D67"), so the help pages
//! are paragraphs of prose rather than one-line captions — and the rendering of
//! a paragraph is invisible from the source, which is why `clap`'s `wrap_help`
//! feature and these snapshots matter: without wrapping, a page prints single
//! lines that a terminal hard-wraps mid-word, with none of the hanging indent
//! an option list is read by.
//!
//! **The snapshots are here to make the rendered shape reviewable, not to
//! assert that the prose is true.** A wording change is expected to move them
//! and `cargo insta review` is the point at which someone sees the result. What
//! is asserted outright is [`no_help_page_exceeds_the_wrap_width`], the one
//! property a snapshot cannot carry: a snapshot taken against an unwrapped page
//! looks exactly as plausible as one taken against a wrapped page, so dropping
//! the feature again would be accepted as "the text changed".
//!
//! `clap` picks the width from the terminal, falling back to `COLUMNS` and then
//! to its own 100-column default (`clap_builder`'s `help_template::dimensions`).
//! Every run here is a pipe with no terminal on any descriptor, so `COLUMNS` is
//! what decides — and it is stated rather than inherited, so the snapshots do
//! not move with the shell that launched `cargo test`.

use std::process::Command;

mod common;
use common::{pgdq, stderr_of, stdout_of};

/// The width every page below is rendered at. `clap`'s own default
/// `max_term_width` is also 100, so this is the width a user with no terminal
/// — a redirected `--help`, a CI log — sees.
const WRAP_WIDTH: usize = 100;

/// Every help page `pgdq` prints, as `(name, args)`. Both depths of each
/// command: `clap_derive` splits a doc comment by paragraph, so `-h` prints the
/// first one and `--help` prints the whole comment, and the two renderings are
/// different artifacts.
///
/// `help` itself is omitted — its page is `clap`'s, not ours, and nothing in
/// this repo writes a word of it.
///
/// The root page's two depths currently render identically, because the only
/// prose on it is the subcommand list and `clap` always takes the short form
/// there. Both are still snapshotted rather than one: a root-level flag with a
/// multi-paragraph doc comment would split them, and the pair is what notices.
const PAGES: &[(&str, &[&str])] = &[
    ("root_short", &["-h"]),
    ("root_long", &["--help"]),
    ("parse_short", &["parse", "-h"]),
    ("parse_long", &["parse", "--help"]),
    ("info_short", &["info", "-h"]),
    ("info_long", &["info", "--help"]),
    ("query_short", &["query", "-h"]),
    ("query_long", &["query", "--help"]),
];

/// Render one help page at [`WRAP_WIDTH`], with the width stated rather than
/// inherited.
fn help_page(args: &[&str]) -> String {
    let mut cmd: Command = pgdq();
    let out = cmd
        .args(args)
        .env("COLUMNS", WRAP_WIDTH.to_string())
        .output()
        .expect("the pgdq binary runs");
    // `clap` exits 0 for a help request and writes the page to stdout.
    assert!(out.status.success(), "pgdq {args:?} failed: {}", stderr_of(&out));
    stdout_of(&out)
}

/// The property the snapshots cannot carry: with `wrap_help` off, `clap` wraps
/// at no width at all, and a snapshot of the unwrapped page is as plausible as
/// a snapshot of the wrapped one.
#[test]
fn no_help_page_exceeds_the_wrap_width() {
    for (name, args) in PAGES {
        for line in help_page(args).lines() {
            // Columns, not bytes: the help text carries em dashes and curly
            // quotes, and a byte count would make this pass or fail on the
            // punctuation.
            let columns = line.chars().count();
            assert!(
                columns <= WRAP_WIDTH,
                "{name} ({args:?}) prints a {columns}-column line at COLUMNS={WRAP_WIDTH}; \
                 `clap`'s `wrap_help` feature is what wraps these, and without it no width \
                 wraps them. The line: {line:?}"
            );
        }
    }
}

/// No flag renders with nothing beside it. A flag's help is its doc comment
/// (`docs/design/decisions.md`, "D67"), so a flag added without one prints as a bare spec in the option
/// list — invisible from the source, which is exactly the failure this
/// assertion is for. Snapshots alone would not catch it: a blank is as
/// plausible a snapshot as a paragraph.
///
/// Both of `clap`'s option layouts are accepted, because which one a page gets
/// depends on how wide its widest flag spec is: the description either follows
/// the spec on the same line, or sits on the next line indented deeper.
#[test]
fn no_flag_renders_without_help() {
    for (name, args) in PAGES {
        let page = help_page(args);
        let lines: Vec<&str> = page.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            if indent == 0 || !trimmed.starts_with('-') {
                continue; // Not the line an option's spec starts on.
            }
            // A spec is `-h, --help` or `--source <SOURCE>`: flags, commas and
            // one value placeholder. Anything past that is the description.
            let described_here = trimmed
                .split_whitespace()
                .any(|word| !word.starts_with('-') && !word.starts_with('<'));
            let described_below = lines.get(i + 1).is_some_and(|next| {
                let next_indent = next.len() - next.trim_start().len();
                !next.trim().is_empty() && next_indent > indent
            });
            assert!(
                described_here || described_below,
                "{name} ({args:?}) prints a flag with no help beside it: {line:?}. \
                 A flag's help is its doc comment, so the flag is missing one."
            );
        }
    }
}

/// Every page is non-empty and names the command it is for, so a page that
/// silently stopped being generated cannot pass as a wrapped one.
#[test]
fn every_page_renders() {
    for (name, args) in PAGES {
        let page = help_page(args);
        assert!(page.lines().count() > 3, "{name} ({args:?}) rendered almost nothing: {page:?}");
        assert!(page.contains("Usage: pgdq"), "{name} ({args:?}) has no usage line: {page:?}");
    }
}

#[test]
fn root_short_help() {
    insta::assert_snapshot!(help_page(&["-h"]));
}

#[test]
fn root_long_help() {
    insta::assert_snapshot!(help_page(&["--help"]));
}

#[test]
fn parse_short_help() {
    insta::assert_snapshot!(help_page(&["parse", "-h"]));
}

#[test]
fn parse_long_help() {
    insta::assert_snapshot!(help_page(&["parse", "--help"]));
}

#[test]
fn info_short_help() {
    insta::assert_snapshot!(help_page(&["info", "-h"]));
}

#[test]
fn info_long_help() {
    insta::assert_snapshot!(help_page(&["info", "--help"]));
}

#[test]
fn query_short_help() {
    insta::assert_snapshot!(help_page(&["query", "-h"]));
}

#[test]
fn query_long_help() {
    insta::assert_snapshot!(help_page(&["query", "--help"]));
}
