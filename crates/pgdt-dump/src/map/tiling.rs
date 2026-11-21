//! What every map is checked for and carries, recomputed wherever one is
//! mapped or loaded: the tiling check and its diagnostic, the TOC-coverage
//! figure, an unseekable compressed source's warning, the partition roots
//! rows are written through and TimescaleDB's findings — the
//! [`derived_diagnostics`] — and each span's text read back from the file by
//! offset (`docs/design/decisions.md`, "D30").

use pgdt_core::Result;
use pgdt_io::ByteRangeSource;
use pgdt_model::internal::DiagnosticExt;
use pgdt_model::{
    CompressedContainer, Diagnostic, DumpIndex, SPAN_STORED_TEXT_MAX_BYTES, Span, SpanBody,
    SpanText, TilingIssue,
};

/// Whether a span of this kind stores its text at all. `Data` spans never do
/// (`docs/design/decisions.md`, "D30"); `Unscanned` covers bytes by
/// definition unread, so there is nothing to slice.
fn stores_text(body: &SpanBody) -> bool {
    !matches!(body, SpanBody::Data(_) | SpanBody::Unscanned)
}

/// Whether a span's bytes exceed what [`SpanText`] stores of them.
fn over_cap(span: &Span) -> bool {
    span.end - span.start > SPAN_STORED_TEXT_MAX_BYTES as u64
}

/// Fill in [`Span::text`] for every span that stores it, reading the bytes
/// back from `source` by offset — sliced from the file, never accumulated
/// from [`crate::scan::Event::Line`], which emits nothing for a dollar-quoted
/// string and would miss every function body in the file.
///
/// Runs as a pass over finished spans rather than as each span closes,
/// because only the caller owns the source. Contiguous runs of text-storing
/// spans are coalesced into one `read_range`, so this is one read per gap
/// between data blocks rather than one per span. A span longer than
/// [`SPAN_STORED_TEXT_MAX_BYTES`] ends a run and is read alone, capped, so a
/// multi-gigabyte `Unparsed` region is never pulled into memory whole and
/// every other run's one read is whole.
///
/// Deficiency register: `deficiency: KD136` — over a fetched compressed
/// source each run's read is a fetch of the whole unit holding it, after the
/// scan has already downloaded the object once: the object again where it is
/// under one unit the block path no longer retains, the units holding its DDL
/// where it is not. **(c)
/// unowned**; promoted by a remote `parse` whose second download a user
/// minds.
pub async fn attach_text(source: &dyn ByteRangeSource, spans: &mut [Span]) -> Result<()> {
    let mut i = 0;
    while i < spans.len() {
        if !stores_text(&spans[i].body) {
            spans[i].text = None;
            i += 1;
            continue;
        }
        // Extend over the contiguous run of text-storing spans no longer than
        // the cap, so one read serves all of them; a longer span is a run of
        // its own.
        let mut j = i;
        while !over_cap(&spans[j])
            && j + 1 < spans.len()
            && stores_text(&spans[j + 1].body)
            && spans[j + 1].start == spans[j].end
            && !over_cap(&spans[j + 1])
        {
            j += 1;
        }
        let (run_start, run_end) = (spans[i].start, spans[j].end);
        // A run of several spans is no longer than the cap each, so it is
        // read whole; a span over the cap is alone and read as far as it.
        let want = if over_cap(&spans[i]) {
            SPAN_STORED_TEXT_MAX_BYTES
        } else {
            (run_end - run_start) as usize
        };
        let bytes = source.read_range(run_start, want).await?;
        for span in &mut spans[i..=j] {
            let from = (span.start - run_start) as usize;
            let to = ((span.end - run_start) as usize).min(bytes.len());
            let slice = if from < to { &bytes[from..to] } else { &[][..] };
            let truncated = slice.len() < (span.end - span.start) as usize
                || (span.end - span.start) as usize > SPAN_STORED_TEXT_MAX_BYTES;
            let slice = &slice[..slice.len().min(SPAN_STORED_TEXT_MAX_BYTES)];
            span.text =
                Some(SpanText { text: String::from_utf8_lossy(slice).into_owned(), truncated });
        }
        i = j + 1;
    }
    Ok(())
}

/// Verify that `spans` tiles `[0, expected_end)` exactly: sorted, contiguous,
/// no gaps or overlaps, starting at 0 and ending at `expected_end`. Returns
/// every issue found, not just the first; a tiling failure is a bug in
/// whatever built the spans, never a reason to refuse the file
/// (`docs/design/decisions.md`, "D30").
pub fn check_tiling(spans: &[Span], expected_end: u64) -> Vec<TilingIssue> {
    let mut issues = Vec::new();
    if spans.is_empty() {
        if expected_end != 0 {
            issues.push(TilingIssue::DoesNotReachEnd { last_end: 0, expected_end });
        }
        return issues;
    }

    for (i, s) in spans.iter().enumerate() {
        if s.start >= s.end {
            issues.push(TilingIssue::Empty { index: i });
        }
    }
    for i in 0..spans.len() - 1 {
        let (a, b) = (&spans[i], &spans[i + 1]);
        if b.start < a.start {
            issues.push(TilingIssue::OutOfOrder { index: i + 1 });
        } else if a.end != b.start {
            issues.push(TilingIssue::Discontinuity {
                after_index: i,
                span_end: a.end,
                next_start: b.start,
            });
        }
    }
    if spans[0].start != 0 {
        issues.push(TilingIssue::DoesNotStartAtZero { first_start: spans[0].start });
    }
    let last_end = spans.last().unwrap().end;
    if last_end != expected_end {
        issues.push(TilingIssue::DoesNotReachEnd { last_end, expected_end });
    }
    issues
}

/// Run the tiling check over a finished map and turn any failure into a
/// diagnostic; the map is still returned
/// (`docs/design/decisions.md`, "D30").
fn tiling_diagnostics(spans: &[Span], size: u64) -> Vec<Diagnostic> {
    let issues = check_tiling(spans, size);
    if issues.is_empty() { Vec::new() } else { vec![Diagnostic::tiling_broken(issues)] }
}

/// The TOC-coverage figure for a finished map: how many `spans` are
/// attributed to a TOC entry (`pgdt_model::Span::toc.is_some()` — an
/// inherited entry counts the same as an owned one) against how many spans
/// exist at all (`docs/design/decisions.md`, "D31"). Always produced:
/// `attributed == 0` is a legitimate value, a file with zero TOC comments
/// being the map's header-less degraded mode rather than an error.
fn toc_coverage_diagnostic(spans: &[Span]) -> Diagnostic {
    let attributed = spans.iter().filter(|s| s.toc.is_some()).count();
    Diagnostic::toc_coverage(attributed, spans.len())
}

/// **What a whole-file map carries that nothing persists**
/// (`pgdt_model::DumpIndex::diagnostics` is `#[serde(skip)]`): the tiling
/// check's failure, the TOC-coverage figure, an unseekable compressed
/// source's warning, each partition root's blocks written through it
/// ([`pgdt_model::ThroughRoot::finding`]) and each TimescaleDB database's
/// findings ([`pgdt_model::timescaledb::findings`]), each a pure function of the map
/// `mapped` — its spans, its DDL and its catalog's rows — the file's
/// addressable `size` and the source's compression `index`. **The one place they are computed**, for
/// every site a whole map is finished or loaded — `pgdt_fixtures::build_index`,
/// `pgdt_mapping::map_file`, `pgdt_mapping::map_forward` at EOF and
/// `crate::cache::store::status_from_file` — so no site carries a subset of
/// what another does.
pub fn derived_diagnostics(
    mapped: &DumpIndex,
    size: u64,
    index: Option<&pgdt_io::CompressionIndex>,
) -> Vec<Diagnostic> {
    let mut diagnostics = tiling_diagnostics(&mapped.spans, size);
    diagnostics.push(toc_coverage_diagnostic(&mapped.spans));
    diagnostics.extend(non_seekable_compression_diagnostic(index));
    diagnostics.extend(mapped.through_roots().iter().map(pgdt_model::ThroughRoot::finding));
    diagnostics.extend(pgdt_model::timescaledb::findings(mapped));
    diagnostics
}

/// D19's warning for a compressed source with no usable seek structure, by
/// its codec's rule (`pgdt_io::CompressionIndex::is_seekable`) — `None` when
/// there is no compression layer at all (`index` is `None`), the index has
/// seek structure, or it is a gzip first pass's that has not reached the end,
/// which says nothing yet of how wide its last unit is. A whole map's is
/// [`derived_diagnostics`]'s; a preamble's alone is
/// [`preamble_only`](crate::preamble::preamble_only)'s.
pub fn non_seekable_compression_diagnostic(
    index: Option<&pgdt_io::CompressionIndex>,
) -> Option<Diagnostic> {
    let index = index?;
    if index.is_seekable() || !index.is_complete() {
        return None;
    }
    let container = match index {
        pgdt_io::CompressionIndex::Xz(_) => CompressedContainer::Xz,
        pgdt_io::CompressionIndex::Gzip(gzip) => {
            CompressedContainer::Gzip { spacing: gzip.spacing() }
        }
    };
    Some(Diagnostic::non_seekable_compressed_source(
        container,
        index.unit_count(),
        index.largest_unit(),
    ))
}
