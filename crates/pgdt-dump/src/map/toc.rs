//! A `--` comment block read line by line: whether it is a TOC entry, the
//! header its `Name:` line parses to (I3, I16), the partition-root marker
//! (I2), the version header's two lines (I9), and what the block becomes when
//! it closes.

use pgdt_model::{SpanBody, TocHeader};

/// Parse pg_dump's TOC header line into a [`TocHeader`] — `None` for any
/// comment line that doesn't match, which leaves
/// [`Span::toc`](pgdt_model::Span::toc) `None` — a comment block shaped like an
/// entry still owning its span, so inheriting nothing
/// ([`Span::toc_owned`](pgdt_model::Span::toc_owned)). `line` is expected to
/// have its leading whitespace and its terminator removed and nothing else:
/// the last field, the owner or the tablespace, runs to the line's end, so an
/// empty owner's `; Owner: ` must arrive whole to split, and a name ending in
/// whitespace is the name `sanitize_line` wrote.
///
/// All three of `_printTocEntry()`'s prefixes go through one code path:
/// `TOC_PREFIX_DATA` ("Data for ") and `TOC_PREFIX_STATS` ("Statistics for ",
/// a `pg_dump` 18+ `--statistics` component, I18) are each optional, and what
/// follows one must still be `Name: `. Whether a prefix is also a *boundary*
/// signal is [`looks_like_toc_name_line`]'s question
/// (`docs/design/decisions.md`, "D31").
///
/// Splits on the field markers in the order `_printTocEntry()` writes them,
/// not on a general grammar: `sanitize_line` never escapes a literal
/// `; Type: ` inside an object's own name, so a pathological name can
/// mis-split. I3 treats the TOC comment as a hint, and this parser only ever
/// feeds enrichment, never a boundary.
///
pub(super) fn parse_toc_header_line(line: &str) -> Option<TocHeader> {
    let rest = line.strip_prefix("-- ")?;
    let rest = rest
        .strip_prefix("Data for ")
        .or_else(|| rest.strip_prefix("Statistics for "))
        .unwrap_or(rest);
    let rest = rest.strip_prefix("Name: ")?;
    let (name, rest) = rest.split_once("; Type: ")?;
    let (kind, rest) = rest.split_once("; Schema: ")?;
    let (schema, rest) = rest.split_once("; Owner: ")?;
    let (owner, tablespace) = match rest.split_once("; Tablespace: ") {
        Some((owner, tablespace)) => (owner, Some(tablespace.to_string())),
        None => (rest, None),
    };
    let none_if_placeholder = |s: &str| (!s.is_empty() && s != "-").then(|| s.to_string());
    Some(TocHeader {
        name: name.to_string(),
        kind: kind.to_string(),
        schema: none_if_placeholder(schema),
        owner: none_if_placeholder(owner),
        tablespace,
    })
}

/// Whether `-- Name: ...` (pg_dump's `_printTocEntry()` header, I3) is
/// present in `line` — the lexical, field-blind boundary signal the module
/// docs describe. Deliberately not full TOC parsing: it never reads
/// `Type:`/`Schema:`/`Owner:`, only confirms this comment block is a real
/// TOC entry rather than framing prose.
///
/// `TOC_PREFIX_STATS` ("Statistics for ") counts, what follows a statistics
/// entry being an ordinary statement the span must run into; `TOC_PREFIX_DATA`
/// ("Data for ") does not (`docs/design/decisions.md`, "D31"). The refusal
/// decides nothing on a default dump, and earns its keep on a
/// `--disable-triggers` one (I31), where a statement *does* intervene between
/// the entry and its data: absorbing there would run the entry into that
/// statement's span, where [`super::Builder::push_statement_span`]'s `Framing`
/// veto discards the header outright. Pinned by
/// `a_data_entry_keeps_its_own_span_when_disable_triggers_intervenes`.
pub(super) fn looks_like_toc_name_line(line: &str) -> bool {
    let named = line.starts_with("-- Name: ") || line.starts_with("-- Statistics for Name: ");
    named && line.contains("; Type: ")
}

/// The root table named by a `-- load via partition root <name>` marker
/// line (I2), if `line` is one. `pg_dump` writes it whenever the entry's
/// `COPY` header names the partition's **root** rather than the partition —
/// the only shape in which one header name owns several blocks.
pub(super) fn partition_root_marker(line: &str) -> Option<String> {
    let rest = line.strip_prefix("-- load via partition root ")?;
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Whether `line` is one of the version-header block's two lines (I9), and
/// if so, which field it fills (`true` for server version, `false` for
/// `pg_dump` version).
pub(super) fn version_header_field(line: &str) -> Option<(bool, String)> {
    if let Some(rest) = line.strip_prefix("-- Dumped from database version ") {
        return Some((true, rest.trim().to_string()));
    }
    line.strip_prefix("-- Dumped by pg_dump version ").map(|rest| (false, rest.trim().to_string()))
}

/// What a closing comment block becomes: a TOC entry stays
/// [`SpanBody::Unparsed`] (its span continues into the statement it precedes —
/// see [`super::Builder::step`]'s `Mode::Comment` arm), the version-header
/// block becomes [`SpanBody::VersionHeader`], and everything else is generic
/// [`SpanBody::Framing`].
pub(super) fn close_comment(
    saw_name: bool,
    server_version: Option<String>,
    pg_dump_version: Option<String>,
) -> SpanBody {
    if saw_name {
        SpanBody::Unparsed
    } else if server_version.is_some() || pg_dump_version.is_some() {
        SpanBody::VersionHeader { server_version, pg_dump_version }
    } else {
        SpanBody::Framing
    }
}

#[cfg(test)]
mod tests;
