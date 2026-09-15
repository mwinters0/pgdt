//! The layering check: `docs/design/decisions.md`, "D68" and "D74", read off
//! `src/*.rs`. Production code only — a test module's imports are not the
//! module's dependencies — with comments stripped, so a doc link to
//! `crate::foo` is not an edge.
//!
//! Every `crate::` dependency has to be in a `use` for this to be complete,
//! which is itself one of the assertions: an inline `crate::a::b` at a use
//! site is an edge the import list does not show.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Each module's layer; 0 is cross-cutting. A `src/*.rs` file absent from
/// this table fails: a module is assigned a layer before it is written (D68).
const LAYERS: &[(&str, u8)] = &[
    ("lib", 0),
    ("error", 0),
    ("instrument", 0),
    ("io", 1),
    ("scan", 1),
    ("copy", 1),
    ("map", 1),
    ("index", 1),
    ("preamble", 1),
    ("cache", 1),
    ("diagnostic", 1),
    ("statistics", 1),
    ("pgtype", 2),
    ("resolve", 2),
    ("decode", 2),
    ("nested", 2),
    ("batch", 3),
    ("stream", 4),
    ("predicate", 4),
    ("leader", 4),
    ("gather", 4),
    ("prune", 4),
];

/// The upward edges D68 records. Each must still exist — a deviation the
/// register lists and the code no longer has is a stale entry — and no other
/// upward edge may.
const DEVIATIONS: &[(&str, &str)] = &[("batch", "stream"), ("batch", "predicate")];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn layer_of(module: &str) -> u8 {
    LAYERS
        .iter()
        .find(|(name, _)| *name == module)
        .map(|(_, layer)| *layer)
        .unwrap_or_else(|| panic!("src/{module}.rs has no layer in tests/layering.rs (D68)"))
}

/// The module's production code: everything before its first `#[cfg(test)]
/// mod`, with `//` comments removed.
fn production(module: &str) -> String {
    let text = fs::read_to_string(src_dir().join(format!("{module}.rs"))).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "#[cfg(test)]"
            && lines.get(i + 1).is_some_and(|next| next.trim_start().starts_with("mod "))
        {
            break;
        }
        let code = match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        };
        out.push_str(code);
        out.push('\n');
    }
    out
}

/// `use crate::…;` and `pub use crate::…;` statements, each joined onto one
/// line, with the remainder of the code beside them.
fn split_uses(code: &str) -> (Vec<String>, String) {
    let mut uses = Vec::new();
    let mut rest = String::new();
    let mut pending: Option<String> = None;
    for line in code.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = pending.as_mut() {
            open.push(' ');
            open.push_str(trimmed);
            if trimmed.ends_with(';') {
                uses.push(pending.take().unwrap());
            }
            continue;
        }
        if trimmed.starts_with("use crate::") || trimmed.starts_with("pub use crate::") {
            if trimmed.ends_with(';') {
                uses.push(trimmed.to_string());
            } else {
                pending = Some(trimmed.to_string());
            }
            continue;
        }
        rest.push_str(line);
        rest.push('\n');
    }
    assert!(pending.is_none(), "unterminated use statement: {pending:?}");
    (uses, rest)
}

/// What `lib.rs` re-exports, name → module, so `use crate::{Error, Result}`
/// resolves to the module that defines each name.
fn reexports() -> BTreeMap<String, String> {
    let code = production("lib");
    let mut map = BTreeMap::new();
    map.insert("Result".to_string(), "lib".to_string());
    let joined = code.replace('\n', " ");
    for stmt in joined.split(';') {
        let Some(path) = stmt.trim().strip_prefix("pub use ") else { continue };
        let (module, names) = path.split_once("::").unwrap();
        let names = names.trim().trim_start_matches('{').trim_end_matches('}');
        for name in names.split(',') {
            let name = name.trim();
            if !name.is_empty() {
                assert!(map.insert(name.to_string(), module.to_string()).is_none(), "{name}");
            }
        }
    }
    map
}

/// The modules one `use crate::…;` statement depends on.
fn targets(stmt: &str, reexports: &BTreeMap<String, String>) -> Vec<String> {
    let path = stmt
        .trim_start_matches("pub ")
        .trim_start_matches("use crate::")
        .trim_end_matches(';')
        .trim();
    let (head, _) = path.split_once("::").unwrap_or((path, ""));
    if LAYERS.iter().any(|(name, _)| *name == head) {
        return vec![head.to_string()];
    }
    let names = head.trim_start_matches('{').trim_end_matches('}');
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            reexports
                .get(name)
                .unwrap_or_else(|| {
                    panic!("`{stmt}` names `{name}`, which lib.rs does not re-export")
                })
                .clone()
        })
        .collect()
}

fn modules() -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(src_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_string())
        .collect();
    found.sort();
    found
}

#[test]
fn every_module_has_a_layer_and_every_layer_entry_a_module() {
    let found = modules();
    for module in &found {
        layer_of(module);
    }
    for (name, _) in LAYERS {
        assert!(
            found.iter().any(|m| m == name),
            "tests/layering.rs lists src/{name}.rs, which is gone"
        );
    }
}

#[test]
fn use_points_down_or_sideways_except_the_recorded_deviations() {
    let reexports = reexports();
    let mut upward = Vec::new();
    for module in modules() {
        let (uses, _) = split_uses(&production(&module));
        for stmt in uses {
            for target in targets(&stmt, &reexports) {
                if layer_of(&target) > layer_of(&module) {
                    upward.push((module.clone(), target));
                }
            }
        }
    }
    upward.sort();
    upward.dedup();
    let mut recorded: Vec<(String, String)> =
        DEVIATIONS.iter().map(|(from, to)| (from.to_string(), to.to_string())).collect();
    recorded.sort();
    assert_eq!(
        upward, recorded,
        "upward `use` edges (left) differ from the deviations D68 records (right)"
    );
}

#[test]
fn every_crate_dependency_is_in_a_use() {
    for module in modules() {
        let (_, rest) = split_uses(&production(&module));
        for line in rest.lines() {
            assert!(
                !line.contains("crate::"),
                "src/{module}.rs reaches `crate::` inline rather than in a `use`, hiding an edge from this check: {}",
                line.trim()
            );
        }
    }
}

#[test]
fn l1_never_names_arrow() {
    for (module, layer) in LAYERS {
        if *layer != 1 {
            continue;
        }
        let code = production(module);
        for needle in ["arrow::", "arrow_schema", "use arrow"] {
            assert!(!code.contains(needle), "src/{module}.rs (L1) names `{needle}` (D74)");
        }
    }
}

#[test]
fn l2_names_datatypes_only_and_is_synchronous() {
    for (module, layer) in LAYERS {
        if *layer != 2 {
            continue;
        }
        let code = production(module);
        for (at, _) in code.match_indices("arrow::") {
            let after = &code[at + "arrow::".len()..];
            assert!(
                after.starts_with("datatypes"),
                "src/{module}.rs (L2) reaches past `arrow::datatypes` (D74): arrow::{}",
                after.chars().take(24).collect::<String>()
            );
        }
        for needle in ["async fn", "ByteRangeSource", "std::fs", "tokio"] {
            assert!(!code.contains(needle), "src/{module}.rs (L2) names `{needle}` (D74)");
        }
    }
}
