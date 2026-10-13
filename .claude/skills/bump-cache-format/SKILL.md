---
name: bump-cache-format
description: Bump `CACHE_FORMAT_VERSION`. Use whenever a change alters what a cache persists — a field added, removed or reshaped, or a new way of choosing what an unchanged field holds, a parse fix that changes a table's columns, a type, a version or which database a statement belongs to included — or when deciding whether a change needs a bump.
---

Pre-1.0: just do it and stop worrying about it. The constant is
`crates/pgdt-model/src/lib.rs`'s, beside the model it versions. Moving a
persisted type between modules or crates moves no cache byte — bincode encodes
no path and serde names fields — so a move alone owes no bump.
