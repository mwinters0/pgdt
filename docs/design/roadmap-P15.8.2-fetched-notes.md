# P15.8.2 notes — a parallel first pass, the fetched feed over one download

A fetched first pass runs tasks ahead of its frontier as a local one does, and
every byte its frontier and tasks read comes out of one download of the object
(`first_pass/download.rs`, D117). A fetched BGZF file's units from its members'
headers are 15.8.3's
([`../status/history/2026-10-24.md`](../status/history/2026-10-24.md), "15.8.2
split: the fetched feed's tasks, and a BGZF file's units from its headers"); its
chunks are found meanwhile as any file's are, a speculation beginning at a
member header the search finds by magic.

- **The download** fetches the object a piece at a time, a piece a chunk long,
  each once, a second reader of a piece in flight waiting for its fetch. It
  holds from the piece the frontier stands in over a room of two pieces a
  worker (`Chunking::download_pieces`), dropping each piece the frontier has
  passed; a task asking past the room, or behind the frontier, is refused and
  hands back nothing; the frontier is never refused. A read behind the
  frontier goes straight to the transport, as before.
- **A speculation is handed out only on a chunk the room reaches**:
  `gz_seek::Parallel::task_through(last)`, `task` being `task_through(u64::MAX)`,
  which the pulled feed keeps. Without it a run of chunks whose speculations
  find nothing would hand out speculations ever further ahead of a frontier
  decoding serially, and the download would hold every piece they fetched.
- **A fetched task runs on the runtime**: each piece fetched there and decoded
  on a `spawn_blocking`, its landing told through a `Notify`. A fetched
  frontier standing waiting for a task hands the wait back to its read
  (`Served::Waits`), which awaits the landing rather than holding a blocking
  thread, so a pulled frontier still runs a task nobody has started itself and
  a fetched one never does.
- **The charge**: a fetched pass adds two chunks a worker for the download
  (`Chunking::download_bytes`) to the pass's own and to each task's.
- **A fetched pass recommends what a local one does**, the machine's cores
  capped at its chunk count (D2), rather than one: its workers fetch nothing
  a serial pass would not. Reviewed and reversed: 15.8.4 restores one
  ([`../status/history/2026-10-24.md`](../status/history/2026-10-24.md),
  "15.8.2's fetched default and the take tests, reviewed").
- **The evidence**: `a_fetched_pass_downloads_the_object_once_with_tasks_ahead`
  (the transport's ranges tiling the object once at one to eight workers),
  `a_pass_inflates_ahead_on_either_feed_and_builds_the_serial_index`,
  `a_lifted_pass_resumes_into_the_uninterrupted_index` and
  `a_damaged_file_is_refused_with_tasks_ahead` on both feeds,
  `a_frontier_waits_for_a_task_that_nothing_has_started_on_either_feed` (one
  blocking thread), the download's own tests, `gz-seek`'s
  `a_speculation_is_handed_out_no_further_than_its_bound`; `remote.rs`'s
  `a_remote_gzip_member_is_downloaded_once_by_a_first_pass_inflating_ahead`
  (the ranged GETs after the probe tiling a seven-chunk object once at four
  workers) and
  `parse_and_query_over_a_remote_gzip_member_inflate_ahead_and_answer_what_the_local_file_does`.

## What later slices inherit

- **15.8.3** takes a fetched BGZF file's units from its members' headers; the
  download is what its header reads come out of.
- **15.7's resident readings** read a fetched pass holding its download too,
  against the charge above; nothing is timed, and no figure reads a remote
  first pass.

## Negative results

- **Whether the frontier takes a chunk on a fetched feed is a closer race**
  than on a pulled one: a fetched task reaches the runtime between its fetch
  and each decode, and under CPU contention the frontier sometimes passes
  every chunk before its decode lands (14 of 80 runs of a four-worker pass at
  4099-byte reads, eight test binaries at once, against none of 80 on the
  pulled feed). So the fetched test asserts a take at four workers or eight,
  and the pulled test keeps its own assertion; both still race, which 15.8.4
  removes by sequencing them.
- **A `parse` over a fetched gzip file reads more than its pass**, as the
  serial pass did at `08c62385`: the scan's restart at the file's start reads
  behind the frontier, a piece fetched again, and `attach_text`'s read of each
  run of DDL back after the scan (D30) goes through the indexed source, which
  fetches the unit holding it whole — the whole object for a member under the
  spacing (`KD136`). Observed by printing the oracle's requests after the `parse` in
  `parse_and_query_over_a_remote_gzip_member_…`, on this tree and on it with
  this slice's `pgdt-io` and `gz-seek` stashed.
