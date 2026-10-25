# P15.8.4 notes — a fetched first pass recommending one worker, and the take tests sequenced

A fetched gzip first pass recommends one worker again
(`FirstPass::default_workers`, D2), so its tasks ahead run only under a
stated `--jobs`; the download, `task_through` and the charge 15.8.2 built are
what a stated count runs. The take tests no longer race the scheduler (D73).
Both are the review's
([`../status/history/2026-10-24.md`](../status/history/2026-10-24.md),
"15.8.2's fetched default and the take tests, reviewed").

- **The sequenced take**: `a_lifted_frontier_takes_the_chunks_its_tasks_decoded`,
  on both feeds at two, four and eight workers and at two read sizes, reads a
  step short of any chunk's plaintext at a time, and between steps settles
  the pass as a read does before it steps — results reaped, tasks topped up
  (`Frontier::settle`, test-only) — awaiting each landing on the pass's
  `landed` notify, bounded, until `gz_seek::Parallel::ready` says a decode
  waits ahead of the frontier; it then reads on and asserts a take before the
  end. `ready` is `gz-seek`'s, beside `taken`, and its own test
  `a_decode_run_ahead_is_ready_before_the_frontier_takes_it` holds it.
- **The fetched download test** keeps its tiling assertion and drops its take,
  the sequenced test owning that on both feeds.
- **The evidence**: the sequenced test failed none of 80 runs, eight test
  binaries at once in the release image, where 15.8.2's fetched take failed 14
  of 80 ([notes](roadmap-P15.8.2-fetched-notes.md), "Negative results");
  `a_pass_inflates_ahead_on_either_feed_and_builds_the_serial_index` asserts
  each feed's recommendation; `remote.rs`'s
  `a_remote_gzip_member_is_downloaded_once_by_a_first_pass_inflating_ahead`
  asserts one, and
  `parse_and_query_over_a_remote_gzip_member_inflate_ahead_and_answer_what_the_local_file_does`
  that a `parse` stating no `--jobs` says it reads serially.

## What later slices inherit

- **A take needs blocks shorter than a chunk**: see below. 15.16 and 15.7,
  choosing files for a take, meet the same; the other lifted tests still read
  the unflushed member, so they hold the index and the refusals with tasks
  ahead, not the take path.
- **A take is read before the end**: once the frontier reaches the end the
  pass hands over and its `Frontier` goes, so `taken` answers nothing after.

## Negative results

- **The test payload as `gzip_compress` writes it ends a deflate block every
  three to six of the tests' 8 KiB chunks** under zlib-rs's default level,
  the frontier's boundaries at about 51 and 79 KB of a 100 KB member, so
  `CHUNKING`'s comment no longer claims several a chunk. Read a step at a
  time at two workers, that member never held a decode ahead of its
  frontier, so the take test reads a member flushed every 8 KiB of plaintext
  (`flushed_indexed`), as `remote.rs`'s gzip dump is flushed for the shipped
  chunk.
