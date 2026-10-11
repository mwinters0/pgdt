//! `info --json`.

use std::io::Write;

use anyhow::{Context, Result};
use pgdt_codec::pgtype::NestedPlan;
use pgdt_dump::cache::{CacheEnvelope, CompressionShape};
use pgdt_mapping::StrictUnchecked;
use pgdt_model::{Diagnostic, DumpIndex};

use super::labels::{arrow_type_label, block_resolutions, block_unchecked, resolution_token};

/// `--json`'s shape: the whole cache file — the [`DumpIndex`] and the
/// [`CacheEnvelope`] around it — flattened to one object, plus what the file
/// does not itself carry: the container's shape derived from its seek table,
/// the diagnostics `#[serde(skip)]` drops (`docs/design/decisions.md`, "D22"),
/// and the per-block type resolution, a codec conclusion the model's index has no
/// field for. No schema stability is promised — see the `--json` flag's help
/// text.
///
/// **Coverage is components, not a rendered percentage**: `scanned_through`
/// and `total_size` sit side by side, so a script computes its own ratio.
///
/// **Every block's per-group statistics are exported as the cache holds them**,
/// with no per-table rollup, and the document is written compact and straight
/// to stdout, never held whole (`docs/design/decisions.md`, "D67").
#[derive(serde::Serialize)]
struct IndexJson<'a> {
    #[serde(flatten)]
    envelope: &'a CacheEnvelope,
    #[serde(flatten)]
    index: &'a DumpIndex,
    total_size: u64,
    /// The container's shape, `null` for a plain file — the same three
    /// numbers `text::compression_line` prints, exported because `--json` carries
    /// everything `--detail` would add.
    compression: Option<CompressionShape>,
    diagnostics: &'a [Diagnostic],
    resolution: Vec<BlockResolutionJson<'a>>,
}

/// One `COPY` block's resolution, keyed by the block rather than rolled up per
/// table: a table can span blocks (I2), each naming its columns in its own
/// order, so a per-table rollup needs a merge rule that does not exist (`docs/design/decisions.md`, "D67").
#[derive(serde::Serialize)]
struct BlockResolutionJson<'a> {
    database: Option<&'a str>,
    table: String,
    header_offset: u64,
    columns: Vec<ColumnResolutionJson<'a>>,
    /// What a strict parse checks nothing of, as the listing's `unchecked by
    /// a strict parse` lines state it.
    unchecked: StrictUnchecked,
}

/// One column's resolution: what the DDL declared, what it became, and why.
/// `arrow_type` is the exact string `info --detail` prints for a column that
/// resolved `Mapped` to a type other than `Utf8View`, so the two renderings
/// cannot disagree about such a column's type; `--detail` prints no type for
/// any other column.
#[derive(serde::Serialize)]
struct ColumnResolutionJson<'a> {
    name: &'a str,
    declared: Option<&'a str>,
    outcome: &'static str,
    arrow_type: String,
    plan: &'a NestedPlan,
}

pub(super) fn print_index_json(
    index: &DumpIndex,
    total_size: u64,
    compression: Option<CompressionShape>,
    envelope: &CacheEnvelope,
    complete: bool,
) -> Result<()> {
    let resolutions = block_resolutions(index, complete);
    let resolution = resolutions
        .iter()
        .map(|(block, resolved)| BlockResolutionJson {
            database: block.database.as_deref(),
            table: block.header.qualified_name(),
            header_offset: block.header_offset,
            columns: resolved
                .notes
                .iter()
                .enumerate()
                .map(|(i, note)| ColumnResolutionJson {
                    name: &note.column,
                    declared: note.declared.as_deref(),
                    outcome: resolution_token(&note.resolution),
                    arrow_type: arrow_type_label(
                        resolved.schema.field(i).data_type(),
                        &resolved.plans[i],
                    ),
                    plan: &resolved.plans[i],
                })
                .collect(),
            unchecked: block_unchecked(index, block),
        })
        .collect();
    let wrapped = IndexJson {
        envelope,
        index,
        total_size,
        compression,
        diagnostics: &index.diagnostics,
        resolution,
    };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    serde_json::to_writer(&mut out, &wrapped).context("writing the JSON export")?;
    writeln!(out).and_then(|()| out.flush()).context("writing the JSON export")
}
