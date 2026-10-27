## graft

The repo is indexed by graft (`graft/`, gitignored). Its CLI is now named
`trail`; `.config/bin/graft`, on `PATH` through `.config/mise.toml`, runs it
under the old name its own wiring and directions use. Reach for it before
grep/find or reading files when you need to understand or locate code:

- **MCP tools** (when available): `graft_find_code` for "how does X work" or
  "where is Y", `graft_find_all` for every occurrence, `graft_trace_calls` for
  callers, callees and blast radius, `graft_file_api` for a file's signatures.
  If they are listed but deferred, load them all in one tool search.
- **Shell** (always works): `graft ask "<q>" --source`, `graft grep`,
  `graft callers`, `graft skeleton` print the same.

Its call edges are matched by name, not resolved. A name with one definition
links reliably. A name with several — every trait method, since the trait and
each impl count — is linked only to callers in the same file, where it binds
to that file's definition whatever the receiver: a call through
`Arc<dyn ByteRangeSource>` lands on the impl beside it, and `self.inner.m()`
inside `fn m` reads as recursion. Callers in other files are dropped, so a
trait method's callers are mostly missing, `dyn` or not. Nothing outside the
workspace is indexed. So `graft grep` or `rg` when every occurrence is wanted
or the name is defined outside the workspace, and
`scripts/module_graph.py --edges <module>` (rust-analyzer's references) when
exact references decide something.
