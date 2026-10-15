//! The listing `info` prints, and `parse` closes on.

use std::collections::{BTreeMap, BTreeSet};

use arrow::datatypes::DataType;
use pgdt_codec::ColumnResolution;
use pgdt_dump::cache::CompressionShape;
use pgdt_mapping::StrictUnchecked;
use pgdt_model::{Diagnostic, DumpIndex, DumpMetadata, Finding, Severity};

use super::labels::{
    arrow_type_label, block_resolutions, block_unchecked, enum_labels, label_list, span_summary,
    type_kind_summary,
};
use super::statistics;

/// How a database is named in the listing. A `\connect`-less dump has no name
/// to print, and `(unnamed)` is what the listing calls it — one spelling, so
/// the metadata header, the block listing and `--map` cannot disagree.
fn database_label(database: Option<&str>) -> &str {
    match database {
        Some(name) => name,
        None => "(unnamed)",
    }
}

/// Prints a `database: <name>` line each time the database changes, and only
/// when a listing spans more than one.
///
/// **The rows are already in file order and every `\connect` segment is
/// contiguous in the file**, so a header whenever the value changes is the
/// whole grouping rule. It is also what makes an `AmbiguousTable` error's
/// candidate names ones this listing already showed.
struct DatabaseHeadings<'a> {
    multi: bool,
    current: Option<Option<&'a str>>,
}

impl<'a> DatabaseHeadings<'a> {
    /// `databases` is every row's database, in listing order; only its
    /// cardinality is read here.
    fn new(databases: impl Iterator<Item = Option<&'a str>>) -> Self {
        let multi = databases.collect::<BTreeSet<_>>().len() > 1;
        Self { multi, current: None }
    }

    fn before(&mut self, database: Option<&'a str>) {
        if self.multi && self.current != Some(database) {
            self.current = Some(database);
            println!("database: {}", database_label(database));
        }
    }
}

/// Dump-level metadata header: server/`pg_dump` versions, extension and
/// user-defined-type counts (`docs/design/decisions.md`, "The CLI"). The
/// `database: <name>` line is shown only when it is informative — a single
/// unnamed database prints no header line.
///
/// Under `detail`, the `user-defined types` count becomes the heading of a
/// listing of the types themselves, one line each, in the order the dump
/// declares them. The count is otherwise their only trace.
pub(crate) fn print_metadata(metadata: &DumpMetadata, detail: bool) {
    let multi = metadata.databases.len() > 1;
    for db in &metadata.databases {
        let show_name = multi || db.name.is_some();
        let indent = if show_name { "  " } else { "" };
        if show_name {
            println!("database: {}", database_label(db.name.as_deref()));
        }
        if let Some(v) = &db.server_version {
            println!("{indent}server version: {v}");
        }
        if let Some(v) = &db.pg_dump_version {
            println!("{indent}pg_dump version: {v}");
        }
        println!("{indent}extensions: {}", db.extensions.len());
        println!("{indent}user-defined types: {}", db.types.len());
        if detail {
            // Padded to the widest name this database declares, so the kinds
            // line up; the right edge stays ragged.
            let width = db.types.iter().map(|t| t.name.chars().count()).max().unwrap_or(0);
            for def in &db.types {
                println!(
                    "{indent}    {:width$}  {}",
                    def.name,
                    type_kind_summary(&def.kind),
                    width = width
                );
            }
        }
    }
}

/// The container line `info --detail` prints above the listing, and
/// nothing at all for a plain file, which has no container to describe.
///
/// **Three numbers a user is otherwise sent to `xz --list` for**, which on a
/// many-stream file walks every footer. `largest block` is the largest term of
/// what `--memory`'s read-buffer budget has to clear for a query to read this
/// file a block at a time — **four times over**, one reader's block beside the further
/// blocks the pool keeps however few readers run, plus a read buffer and the
/// decompressor's own working memory (`docs/design/decisions.md`, "D15").
fn compression_line(shape: &CompressionShape) -> String {
    format!(
        "compression: {} — {} block(s) in {} stream(s), largest block {} bytes uncompressed",
        shape.container, shape.blocks, shape.streams, shape.max_block_uncompressed
    )
}

/// Print the whole listing, below whatever coverage line [`super::report`] already
/// stated. `complete` is passed straight through to [`block_resolutions`],
/// which is where it means something, and **nothing here is qualified by how
/// much of the file was scanned** — the coverage line says it once
/// (see [`super::completion_line`]). `unchecked` prints each block's `unchecked by
/// a strict parse` lines, which every listing otherwise counts alone: `info`
/// passes its `--detail`, `parse` whether it was strict, so what `info` shows
/// follows its own flag and never which run built the cache.
pub(crate) fn print_index(
    index: &DumpIndex,
    compression: Option<CompressionShape>,
    detail: bool,
    map: bool,
    complete: bool,
    unchecked: bool,
) {
    if let Some(metadata) = &index.metadata {
        print_metadata(metadata, detail);
        println!();
    }

    // Above the diagnostics rather than below them, because the shape is what
    // the compression warning beneath it is *about*.
    if let Some(shape) = compression.filter(|_| detail) {
        println!("{}", compression_line(&shape));
        println!();
    }

    if print_diagnostics(&index.diagnostics) {
        println!();
    }

    let printed_roles = print_cross_references(index);
    let printed_kinds = print_object_kinds(index);
    if printed_roles || printed_kinds {
        println!();
    }

    if map {
        print_map(index);
        println!();
        println!("{} span(s)", index.spans.len());
        return;
    }

    // One resolution pass, shared with `--json` — see `block_resolutions`.
    let blocks = block_resolutions(index, complete);
    if blocks.is_empty() {
        println!("no COPY blocks found");
        return;
    }

    let mut total_columns = 0usize;
    let mut total_unmapped = 0usize;
    let mut outside_strict = 0usize;

    let mut headings = DatabaseHeadings::new(blocks.iter().map(|(database, ..)| *database));

    for &(database, block, ref resolved) in &blocks {
        headings.before(database);
        println!("{} ({} rows)", block.header.qualified_name(), block.row_count);
        println!("    level: {}", level_label(block));
        if block.checked_in_full {
            println!("    checked: every value, by a strict parse");
        }
        let held = block_unchecked(index, database, block);
        if unchecked {
            for line in unchecked_lines(&held) {
                println!("    unchecked by a strict parse: {line}");
            }
        }
        outside_strict += usize::from(!held.is_empty());
        for column in block.ignored_refusals.iter().flat_map(|ignored| &ignored.columns) {
            println!("    refused by PostgreSQL: {}", ignored_refusals_line(block, column));
        }
        if block.header.columns.is_empty() {
            println!("    columns: none (every column dropped or generated, or none declared)");
        } else {
            let columns: Vec<String> = resolved
                .notes
                .iter()
                .map(|d| match &d.declared {
                    Some(ty) => format!("{} {ty}", d.column),
                    None => format!("{} (unknown)", d.column),
                })
                .collect();
            println!("    columns: {}", columns.join(", "));
            if detail {
                // One line per column that has something to say: a column
                // that did not map says why, one that mapped says what it
                // mapped *to* unless that is `Utf8View` — the no-information
                // answer, and the only Arrow type a non-`Mapped` resolution
                // produces, so the two arms never both fire.
                //
                // An enum column carries its declared labels on a
                // continuation line beneath, uncapped: this is the only place
                // a user can read them without grepping the dump for its
                // `CREATE TYPE`, and a continuation rather than a suffix
                // keeps a long list from breaking the alignment.
                for (i, note) in resolved.notes.iter().enumerate() {
                    let data_type = resolved.schema.field(i).data_type();
                    if note.resolution != ColumnResolution::Mapped {
                        println!("    {}: {}", note.column, note.resolution.describe());
                    } else if *data_type != DataType::Utf8View {
                        println!(
                            "    {}: {}",
                            note.column,
                            arrow_type_label(data_type, &resolved.plans[i])
                        );
                    }
                    if let Some(labels) = enum_labels(&resolved.comparisons[i]) {
                        println!("        labels: {}", label_list(labels));
                    }
                    let count = block.unrepresentable().and_then(|c| c.get(i));
                    if let Some(count) = count.filter(|c| !c.is_zero())
                        && *data_type != DataType::Utf8View
                    {
                        println!("        unrepresentable: {}", unrepresentable_line(count));
                    }
                }
            }
            total_columns += resolved.notes.len();
            total_unmapped += resolved.unmapped_count();
        }
        if detail {
            println!("    header offset: {}", block.header_offset);
            println!("    data offset:   {}", block.data_offset);
            println!("    terminator:    {}", block.terminator_offset);
            println!("    end offset:    {}", block.end_offset);
        }
    }

    if detail {
        println!();
        print_statistics(index);
    }

    println!();
    // No byte count here: `info`'s coverage line owns that.
    println!("{} COPY block(s), {} row(s)", blocks.len(), index.total_rows());
    if total_unmapped > 0 {
        println!(
            "{total_unmapped} of {total_columns} columns unmapped — run with --detail for details"
        );
    }
    if outside_strict > 0 {
        let where_listed = if unchecked {
            ", each listed as `unchecked by a strict parse`"
        } else {
            " — `pgdt info --detail` lists it"
        };
        println!(
            "{outside_strict} of {} COPY block(s) hold what a strict parse does not \
             check{where_listed}",
            blocks.len()
        );
    }
}

/// One line per position a strict parse checks nothing of, then one per
/// `CHECK`, then one per partition bound and one for routing through a
/// partitioned table — or, for a table the dump declares nowhere, every
/// field of which goes unchecked, one saying so and the routing.
fn unchecked_lines(unchecked: &StrictUnchecked) -> Vec<String> {
    let routed = unchecked.routed_through.iter().map(|table| {
        format!("routed through {table}: a row no partition's bound admits is refused")
    });
    if !unchecked.declared {
        let undeclared = "every value — the table is declared nowhere in the dump".to_string();
        return std::iter::once(undeclared).chain(routed).collect();
    }
    let columns = unchecked.columns.iter().map(|c| {
        let at = format!("{}{}", c.column, c.path);
        match c.declared.as_str() {
            "" => format!("{at}: {}", c.why.describe()),
            declared => format!("{at} {declared}: {}", c.why.describe()),
        }
    });
    let checks = unchecked.checks.iter().map(|check| {
        let name = check.name.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
        match &check.inherited_from {
            Some(from) => format!("CHECK{name}, inherited from {from}: an expression"),
            None => format!("CHECK{name}: an expression"),
        }
    });
    let bounds = unchecked.bounds.iter().map(|b| {
        let bound = b.bound.as_deref().unwrap_or("(its bound not read)");
        format!(
            "{} PARTITION OF {} {bound}: a partition bound, compared under its key's operator \
             classes",
            b.partition, b.parent
        )
    });
    columns.chain(checks).chain(bounds).chain(routed).collect()
}

/// A column's unrepresentable count, as `info --detail` states it beneath the
/// column's type: each tier it holds a value in, the engine's with the day
/// its calendar ends (`docs/design/decisions.md`, "D96").
fn unrepresentable_line(count: &pgdt_model::Unrepresentable) -> String {
    let mut parts = Vec::new();
    if count.format() > 0 {
        parts.push(format!("{} value(s) Arrow cannot hold", count.format()));
    }
    if count.engine() > 0 {
        parts.push(format!(
            "{} past {}, which a DataFusion query cannot display",
            count.engine(),
            pgdt_codec::decode::render_date32(pgdt_model::calendar_end())
        ));
    }
    parts.join("; ")
}

/// What a block records of the fields PostgreSQL refuses in one column that a
/// parse under `--postgres-invalid-values ignore` went past: how many, and the
/// first as the refusal names it — what a parse under `default` tracking the
/// column fails with over this cache.
fn ignored_refusals_line(
    block: &pgdt_model::CopyBlock,
    ignored: &pgdt_model::ColumnRefusals,
) -> String {
    let first = &ignored.first;
    let column = block.header.columns.get(first.column).map_or("?", String::as_str);
    let value = match &first.value {
        Some(value) => format!("`{value}` as `{}`", first.declared_type),
        None => "a NULL in a column declared `NOT NULL`".to_string(),
    };
    format!(
        "column {column}, {} field(s) a parse under --postgres-invalid-values ignore went past, \
         the first at COPY line {}: {value}, at offset {}",
        ignored.count,
        first.line,
        block.data_offset + first.offset,
    )
}

/// The level a block was mapped at, as `--statistics-level` spells it: `data`
/// where it holds a census, `metadata` where it holds nothing drawn from its
/// rows (`docs/design/decisions.md`, "D35").
fn level_label(block: &pgdt_model::CopyBlock) -> &'static str {
    match block.census {
        Some(_) => "data",
        None => "metadata",
    }
}

/// `--detail`'s statistics section: a line per table and, under one that
/// gathered any block, one per column, or one line saying no block carries any
/// ([`statistics::table_statistics`]).
fn print_statistics(index: &DumpIndex) {
    let tables = statistics::table_statistics(index);
    if tables.iter().all(|t| t.gathered_blocks == 0) {
        println!("statistics: none gathered");
        return;
    }
    println!("statistics:");
    let mut headings = DatabaseHeadings::new(tables.iter().map(|t| t.database));
    for table in &tables {
        headings.before(table.database);
        println!("    {}", table.line());
        if table.gathered_blocks > 0 {
            for column in &table.columns {
                println!("        {}", column.line(table.blocks));
            }
        }
    }
}

/// `DumpIndex::diagnostics` (or, for `--preamble-only`, the diagnostics
/// `preamble_only` reports separately), printed unconditionally with no
/// severity threshold. Cache-only mode's "unverified, historical" banner rides
/// this same path (`DiagnosticKind::CacheOffline`). Returns whether anything
/// was printed.
pub(crate) fn print_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    if diagnostics.is_empty() {
        return false;
    }
    println!("diagnostics:");
    for d in diagnostics {
        println!("    [{}] {}", severity_label(d.severity()), d.message());
    }
    true
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

/// Referenced-role and referenced-tablespace summary — an empty set prints
/// nothing. Returns
/// whether anything was printed, so the caller knows whether to add a
/// separating blank line.
fn print_cross_references(index: &DumpIndex) -> bool {
    let mut printed = false;
    if !index.roles.is_empty() {
        println!("roles: {}", index.roles.iter().cloned().collect::<Vec<_>>().join(", "));
        printed = true;
    }
    if !index.tablespaces.is_empty() {
        println!(
            "tablespaces: {}",
            index.tablespaces.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        printed = true;
    }
    printed
}

/// Per-`Type:` object-kind counts — one per archive entry, the same closed
/// vocabulary the TOC-coverage diagnostic counts against
/// (`docs/design/decisions.md`, "D31"). Counts `toc_owned` spans, not every
/// attributed one: this is an object *census*, and a follow-on statement
/// (`ALTER ... OWNER TO`, etc.) inherits its governing entry's `toc`, so
/// counting `span.toc.is_some()` would count that object twice. A span with
/// no TOC comment at all contributes to no bucket. Returns whether anything
/// was printed.
fn print_object_kinds(index: &DumpIndex) -> bool {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for span in &index.spans {
        if span.toc_owned
            && let Some(toc) = &span.toc
        {
            *counts.entry(toc.kind.as_str()).or_default() += 1;
        }
    }
    if counts.is_empty() {
        return false;
    }
    println!("object kinds:");
    for (kind, count) in counts {
        println!("    {kind}: {count}");
    }
    true
}

/// `--map`: every span the full file map found, in file order — the raw
/// structure `DumpIndex::spans` keeps, not the per-table view `blocks()`
/// filters it down to (`docs/design/decisions.md`, "D34"). Grouped by
/// [`DatabaseHeadings`], as the ordinary block listing is.
fn print_map(index: &DumpIndex) {
    let mut headings = DatabaseHeadings::new(index.spans.iter().map(|s| s.database.as_deref()));
    for span in &index.spans {
        headings.before(span.database.as_deref());
        println!("[{}, {}) {}", span.start, span.end, span_summary(span));
    }
}
