<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

Its call edges are matched by name, not resolved: a method called through a
typed or `dyn` receiver, or passed by path, is often missing, a common name can
resolve into a crate the caller cannot depend on, and nothing outside the
workspace is indexed. So `rg` when every occurrence is wanted or the name is
defined outside the workspace, and `scripts/module_graph.py --edges <module>`
(rust-analyzer's references) when exact references decide something.
