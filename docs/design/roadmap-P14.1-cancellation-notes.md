# P14.1 — An awaitable cancellation

What landed: `scan::Cancellation`, a public type holding the polled bit and a
`tokio::sync::Notify` beside it, replacing the bare `Arc<AtomicBool>` behind
`ScanOptions::cancel`. [`decisions.md`](decisions.md), "D26" is amended in the
same change. No remote code, and no observable change to any command.

## What the rest of the phase inherits

- **The seam a source cancels through is `Cancellation::until_cancelled`.** It
  races a future against the signal and returns `None` where the cancellation
  won, dropping the abandoned future at that point. That is the whole of what
  14.5's ranged GET needs: nothing in the trait, no second argument on
  `read_range`, and no per-source cancellation concept. What 14.5 still owes is
  *handing* the source a clone — `ScanOptions` is not visible to a
  `ByteRangeSource`, so the remote constructor takes one or the source is given
  it at `hint_parallelism` time; that choice is open and belongs with the
  constructor of D14.
- **Nothing in the tree awaits the signal yet.** The local source's wait is a
  blocking `pread` and cannot be interrupted mid-read, so every in-tree call
  site still polls and is unchanged. Until 14.5, `cancelled()` and
  `until_cancelled` are exercised by `scan.rs`'s unit tests alone — which is
  the state D16 predicted, not a gap.
- **`Cancellation` is L1, in `scan.rs`.** No module and no layer was added:
  `io.rs` and `scan.rs` already import each other, so the remote source reaches
  it without a new edge in `tests/layering.rs`.
- **The polled contract is untouched.** `ScanOptions::cancelled()` keeps its
  name, its meaning and its `Relaxed` load, so D26's "per chunk or leader
  window, honoured by the mapping passes alone" still describes when a *scan*
  stops. The signal widens who can wait, not who honours it.

## Negative results

- **`tokio_util::sync::CancellationToken` was not taken**, though its API is
  what `is_cancelled`/`cancelled` is named after. It is a dependency
  (`tokio-util`) for a type that is twenty lines over the `sync` feature this
  crate already carries, and D16 costed it that way before the slice opened.
- **A plain `Notify` is not enough on its own.** `notify_waiters` stores no
  permit, so a cancellation landing between the bit's load and the waiter's
  registration would be lost. `cancelled()` therefore registers first
  (`Notified::enable`) and loads second, and the store in `cancel` is `SeqCst`
  to order against that load — the one place in the type where `Relaxed` would
  be wrong.
- **`tokio::select!` was refused** for `until_cancelled`: it needs `tokio`'s
  `macros` feature, which the library does not carry and which would be a
  dependency change for a two-branch race. `futures::future::select` over two
  `std::pin::pin!`ed futures does it with what is already there.
- **The drop is asserted, not assumed.** `until_cancelled_drops_the_work_it_gave_up_on`
  holds a guard inside a `pending` future and checks the guard ran; without it
  the "drops the request in flight" claim would be a comment the code does not
  test ([`../../.claude/skills/evidence/SKILL.md`](../../.claude/skills/evidence/SKILL.md)).
