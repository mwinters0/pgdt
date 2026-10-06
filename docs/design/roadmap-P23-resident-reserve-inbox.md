# P23 inbox — facts filed for its grilling

Evidence that P23 (statistics coverage and the resident reserve) will need.
**This is a queue, not a document**: when P23 is grilled, walk every entry,
fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## mimalloc reads back every option by name

**Fact.** mimalloc 3.3.2 exports `mi_options_print_out(out, arg)`
(`src/options.c`), which writes each option's name and value as mimalloc holds
it, through a caller's output function. `libmimalloc-sys` 0.1.49 does not
declare it, nor v3's purge options; the symbol is linked regardless, as
`mi_option_set` is. 30.8 read one option back instead, by an index into
`mi_option_e` counted by hand, guarded by a test `mise run check` never runs
(`introspect`'s `MI_OPTION_PURGE_DELAY`, from `30f73fb6`, on 30.9's
removal list).

**Why P23 cares.** If P23 confirms the unit mimalloc keeps by switching an
option off, or sets one as the remedy, the instrument has to show the option
reached mimalloc rather than assume it. Reading all of them by name needs no
index to drift at an upgrade; copying back 30.8's hand count is the obvious
route and the worse one.

**Origin.** 2026-10-06
([`../status/history/2026-10-06.md`](../status/history/2026-10-06.md),
"30.8's read-back goes with it"). *Contingent on* mimalloc still exporting
`mi_options_print_out`, and on `libmimalloc-sys` still not declaring it.
