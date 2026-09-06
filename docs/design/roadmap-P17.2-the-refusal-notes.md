# P17.2 — the refusal, and the deletion goes

What 17.3 inherits.

## Where the refusal is, and what it carries

`Error::CacheSourceMismatch { path, cached_stored_size, live_stored_size }`,
raised at all three scan entry points — `index::preamble_only`,
`stream::map_file`, `stream::table_stream` — from the `CacheLoad::SourceChanged`
arm 17.1 put in front of them. The other three unusable arms still answer
`DumpIndex::default()`, and `Disabled` with them: the caller consulted no path,
so there was never a cache to protect.

**The path comes from the mode, not from the load.** `CacheMode::source_mismatch`
is the one-line constructor each site calls; it matches `Enabled(path)` and
answers `Error::CacheModeMismatch` for the other two, which is the same
caller-contract answer `load` already gives `Offline`. `CacheLoad` still carries
no path, for 17.1's reason — `Disabled` has none — and every one of these callers
holds the `&CacheMode` anyway.

**17.3 does not have to re-`stat` anything.** Both sizes and the path are in the
error, so a CLI message naming what it found, what it expected and the two ways
out is a `Display` away.

## What the CLI does now, and the one thing it does not

`discard_unusable_cache` is gone, and with it `open_unaided`, which existed only
to reopen the file after the deletion. `parse`'s `Recognized::Mismatch` arm now
bails with `unusable_cache_message(&CacheStatus::Unreadable, &path, Some(&file))`
— byte for byte the sentence `info` and `query` already print for that
condition, so all three commands answer a contradicted compression claim
identically and having read nothing.

**That is the second condition, and it is not `SourceChanged`.** A cache whose
compression claim the file contradicts has *equal* stored sizes — that is what
makes it reachable at all — so it never reaches `CacheLoad::SourceChanged` and
the library's refusal has nothing to say about it. Recognition catches it in the
CLI, before any source exists. Two conditions, two sites, one rule.

**One sentence now covers two conditions `parse` treats differently**, which is
17.3's to weigh when it writes the messages. `Unreadable`'s "… is not a pgdq
cache" is printed both for foreign bytes at the cache path — which `parse` scans
over, there being nothing there worth keeping — and for a contradicted
compression claim, which `parse` refuses. `info` and `query` refuse both, so the
sentence reads correctly for them; it is `parse` that now has two outcomes
behind one message. Splitting them means a fifth sentence in
`unusable_cache_message`, whose match is on `CacheStatus` and would need a
condition that is not one.

**The `.xz` size-mismatch path still pays a footer walk before the refusal, and
17.3 is where that is decided.** `parse` calls `open_with_cache` first;
`known_compression` sees the stored sizes disagree, answers `Unknown`, and
`open_local` walks the file's stream footers — 85 s on the koji download — after
which `map_file` refuses in a millisecond. The information needed to refuse is
already on disk before the walk starts, so the fix is an ordering one in the CLI
rather than anything in the library. Nothing in this slice depends on it and no
test covers it; the cost is bounded by how many streams the file has, and is one
read for every shape but the concatenated one.

## Evidence

- `a_scan_refuses_a_cache_that_records_another_source_and_leaves_it_alone`
  (`tests/cache.rs`) puts a grown dump to all three entry points and asserts the
  variant, the path and both sizes at each, **then reads the cache file back byte
  for byte**. That last assertion is the one that would fail silently: a refusal
  that still wrote is indistinguishable from one that did not until the bytes are
  compared.
- `parse_refuses_a_cache_that_records_another_source_and_leaves_it_alone`
  (`pgdump_query-cli/tests/partial_reporting.rs`) is the same claim through the
  binary, plus the way out — remove the file and the same command scans.
- `every_command_refuses_a_cache_that_does_not_describe_the_file` and
  `parse_scans_once_the_refused_cache_is_removed`
  (`pgdump_query-cli/tests/xz_source.rs`) replace the test that asserted the
  deletion.

## Claims corrected in this change

`architecture.md`, "The cache" carries the reversal — the rule, its four rejected
alternatives, and D3/D4 as one paragraph on what mismatch is *not*; "The
compressed source" keeps the deletion only as a rejected alternative, and
"Errors" lists the new variant. `docs/manual/dump-inspection.md` said `info` and
`query` refuse "and `pgdq parse` is what builds a fresh one", which this slice
falsified in the same change, per `../process.md`, "Where does this fact go?".
The 2026-09-06 history entry's `M61` paragraph named the deletion as the answer
to a hazard that is real and still worth knowing — a condemned cache passes
`load`'s identity check on the second pass — so the hazard and the rejected
cross-check stayed and the resolution was rewritten.
