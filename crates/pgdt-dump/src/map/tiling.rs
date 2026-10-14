//! What every map is checked for and carries, recomputed wherever one is
//! mapped or loaded: the tiling check and its diagnostic, the TOC-coverage
//! figure, an unseekable `.xz` source's warning, and each span's text read
//! back from the file by offset (`docs/design/decisions.md`, "D30").

use pgdt_core::Result;
use pgdt_io::ByteRangeSource;
use pgdt_model::internal::DiagnosticExt;
use pgdt_model::{Diagnostic, SPAN_STORED_TEXT_MAX_BYTES, Span, SpanBody, SpanText, TilingIssue};

/// Whether a span of this kind stores its text at all. `Data` spans never do
/// (`docs/design/decisions.md`, "D30"); `Unscanned` covers bytes by
/// definition unread, so there is nothing to slice.
fn stores_text(body: &SpanBody) -> bool {
    !matches!(body, SpanBody::Data(_) | SpanBody::Unscanned)
}

/// Fill in [`Span::text`] for every span that stores it, reading the bytes
/// back from `source` by offset — sliced from the file, never accumulated
/// from [`crate::scan::Event::Line`], which emits nothing for a dollar-quoted
/// string and would miss every function body in the file.
///
/// Runs as a pass over finished spans rather than as each span closes,
/// because only the caller owns the source. Contiguous runs of text-storing
/// spans are coalesced into one `read_range`, so this is one read per gap
/// between data blocks rather than one per span.
///
/// Deficiency register: `deficiency: KD31` — that read is capped at
/// `SPAN_STORED_TEXT_MAX_BYTES` times the run's span count from the run's start, not per span,
/// so a span following one longer than the cap can fall past what was read and
/// be stored empty and `truncated` however short it is. **(c) unowned**;
/// promoted by a `--map` listing seen to lose a statement's text, the fix
/// being a read per span past the cap.
pub async fn attach_text(source: &dyn ByteRangeSource, spans: &mut [Span]) -> Result<()> {
    let mut i = 0;
    while i < spans.len() {
        if !stores_text(&spans[i].body) {
            spans[i].text = None;
            i += 1;
            continue;
        }
        // Extend over the whole contiguous run of text-storing spans, so one
        // read serves all of them.
        let mut j = i;
        while j + 1 < spans.len()
            && stores_text(&spans[j + 1].body)
            && spans[j + 1].start == spans[j].end
        {
            j += 1;
        }
        let (run_start, run_end) = (spans[i].start, spans[j].end);
        // Capped at what the run's spans can store between them, so a
        // multi-gigabyte `Unparsed` region is never pulled into memory whole
        // (`KD31`).
        let want =
            (run_end - run_start).min(((j - i + 1) * SPAN_STORED_TEXT_MAX_BYTES) as u64) as usize;
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
/// every issue found, not just the first; a tiling failure is a bug in this
/// module, never a reason to refuse the file
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
pub fn tiling_diagnostics(spans: &[Span], expected_end: u64) -> Vec<Diagnostic> {
    let issues = check_tiling(spans, expected_end);
    if issues.is_empty() { Vec::new() } else { vec![Diagnostic::tiling_broken(issues)] }
}

/// The TOC-coverage figure for a finished map: how many `spans` are
/// attributed to a TOC entry (`pgdt_model::Span::toc.is_some()` — an
/// inherited entry counts the same as an owned one) against how many spans
/// exist at all (`docs/design/decisions.md`, "D31"). Always produced:
/// `attributed == 0` is a legitimate value, a file with zero TOC comments
/// being the map's header-less degraded mode rather than an error.
pub fn toc_coverage_diagnostic(spans: &[Span]) -> Diagnostic {
    let attributed = spans.iter().filter(|s| s.toc.is_some()).count();
    Diagnostic::toc_coverage(attributed, spans.len())
}

/// D19's warning for a `.xz` source with no usable seek structure — `None` when
/// there is no compression layer at all (`table` is `None`) or the table
/// already has more than one block. One function for both callers: a live
/// [`ByteRangeSource`] (`pgdt_mapping::build_index`,
/// [`preamble_only`](crate::preamble::preamble_only)) and a persisted cache
/// (`crate::cache::store::status_from_file`).
pub fn non_seekable_compression_diagnostic(
    table: Option<&pgdt_io::SeekTable>,
) -> Option<Diagnostic> {
    let table = table?;
    if table.is_seekable() {
        return None;
    }
    Some(Diagnostic::non_seekable_compressed_source(table.block_count()))
}
