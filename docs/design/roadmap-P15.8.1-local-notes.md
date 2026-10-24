# P15.8.1 notes — a parallel first pass, the local arm

`pgdt-io`'s first pass drives `gz_seek::Parallel` in place of
`gz_seek::Forward` on both feeds; a pulled feed runs the tasks it hands out
ahead of the frontier, each a `spawn_blocking` of its own reading the file
positionally (`first_pass/ahead.rs`). A fetched feed runs none, so its pass
is the serial one: the fetched arm, and a fetched BGZF file's units from its
headers, are 15.8.2's
([`../status/history/2026-10-24.md`](../status/history/2026-10-24.md),
"15.8.1 split: the local arm, and the fetched arm over one download").

- **How many tasks run**: every worker the announced `Parallelism` affords at
  a task's charge but the one the frontier is, and no more than the file has
  chunks past the first; none before any parallelism is announced and none
  under `Serial` (D1), so the library default is the serial pass.
- **What is held** is bounded by the gate in `Ahead::top_up`: at most that
  many tasks running, and the pass's `pending()` under twice that many, so
  each task's charge (`Chunking::task_bytes`) covers what it holds running
  and one decode waiting for the frontier.
- **A wait never rests on a thread the runtime has yet to give**: the
  frontier waits on a blocking thread, so a task it waits for that no
  blocking thread has started it takes and runs itself (`Ahead::wait_for`);
  on a runtime of one blocking thread that task would otherwise sit queued behind the
  wait forever.
- **A local file recommends its cores capped at its chunk count** and charges
  a task a worker, a file of one chunk keeping the pass's own charge; the
  scan stays one partition, so a leader stood on an open `COPY` block still
  declines its interior and reports `bound_by=source` (D2, `scan_region`).
- **The started line moved to the first read**, where the parallelism is
  announced: it says `reading serially` or `inflating in parallel … its scan
  one reader` with `workers=`, the resume's checkpoint as before (D64).
- **The evidence**: `a_pulled_pass_inflates_ahead_and_builds_the_serial_index`
  (both feeds, one to eight workers), `a_lifted_frontier_takes_the_chunks_its_tasks_decoded`,
  `a_lifted_pass_resumes_into_the_uninterrupted_index`,
  `a_damaged_file_is_refused_with_tasks_ahead`,
  `a_frontier_runs_a_task_it_waits_for_that_nothing_has_started`;
  `determinism.rs`'s gzip test
  now runs the default and `--jobs 8` legs with tasks ahead and resumes at
  both counts; `a_gzip_first_pass_says_whether_it_inflates_in_parallel`.

## What later slices inherit

- **15.8.2** gives the fetched feed tasks: each task reads its chunk, and the
  engine's decode of it reads it again, so a fetched pass running them
  as built here would download the object more than once.
- **The chunk length and the plaintext limit** (`Chunking::SHIPPED`) are set
  without a figure; 15.7's readings on more than one writer's files say
  whether a chunk compressing past the limit, which the frontier then decodes
  itself, binds.
- **15.7's resident readings** read a pass now holding its tasks too; the
  charge is the bound to compare them against.
- **Nothing is timed**, as 15.8 left it: what the lift buys is 15.7's.

## Negative results

- **A taken chunk asks for no compressed input**, so the frontier steps before
  reading any (`Decode::steps_without_input`): the fill loop's refill ahead of
  a step would re-read a whole input chunk at every taken chunk's stop, bytes
  its task has already read.
