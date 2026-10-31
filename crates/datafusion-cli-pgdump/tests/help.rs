//! The exported [`Args`]' help as an embedder renders it: wrapped, as `clap`'s
//! `wrap_help` wraps it, and with none of `pgdt sql`'s own settings — no
//! `next_line_help` above all, `clap` choosing each page's layout itself.
//!
//! `pgdt`'s `help_text.rs` snapshots the pages `pgdt sql` prints; this holds
//! the library's own, which no `pgdt` attribute can rescue.

use clap::CommandFactory;
use datafusion_cli_pgdump::Args;

/// `clap`'s fallback width where neither a terminal nor `COLUMNS` states one,
/// as `pgdt`'s `help_text.rs` renders at.
const WIDTH: usize = 100;

/// Both depths of the page, `-h`'s and `--help`'s, which `clap` lays out apart.
fn pages() -> [(&'static str, String); 2] {
    let mut command = Args::command().term_width(WIDTH);
    let short = command.render_help().to_string();
    let long = command.render_long_help().to_string();
    [("-h", short), ("--help", long)]
}

/// Every line within the page, `--source`'s spec included, and none indented
/// past half of it: a value name wider than the page sets every flag's help
/// column past it, printing that help a word to a line
/// (`docs/design/decisions.md`, "D67").
#[test]
fn every_flag_s_help_prints_across_the_page_without_next_line_help() {
    for (depth, page) in pages() {
        assert!(page.contains("--source <"), "{depth} lists no --source: {page}");
        for line in page.lines() {
            // Columns, not bytes: the help carries em dashes.
            let columns = line.chars().count();
            let indent = columns - line.trim_start().chars().count();
            assert!(
                columns <= WIDTH,
                "{depth} prints a {columns}-column line at {WIDTH}: {line:?}"
            );
            assert!(
                indent < WIDTH / 2,
                "{depth} indents a line {indent} columns at {WIDTH}, a help column \
                 pushed past half the page: {line:?}"
            );
        }
    }
}
