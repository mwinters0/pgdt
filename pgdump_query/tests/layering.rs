//! The crate check: `docs/design/decisions.md`, "D68" and "D74", read off
//! `src/*.rs` and each module's production submodules. Production code only —
//! a test module's imports are not the module's dependencies — with comments
//! stripped, so a doc link to `crate::foo` is not an edge.
//!
//! Every `crate::` dependency has to be in a `use` for this to be complete,
//! which is itself one of the assertions: an inline `crate::a::b` at a use
//! site is an edge the import list does not show.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Each module's crate. A `src/*.rs` file absent from this table fails: a
/// module is given a crate before it is written (D68). `lib` is the facade
/// re-exporting the rest, and holds `Result` beside the error type.
const CRATES: &[(&str, &str)] = &[
    ("lib", "core"),
    ("error", "core"),
    ("instrument", "core"),
    ("lex", "core"),
    ("copy", "core"),
    ("model", "model"),
    ("io", "io"),
    ("scan", "dump"),
    ("map", "dump"),
    ("preamble", "dump"),
    ("cache", "dump"),
    ("load", "dump"),
    ("leader", "dump"),
    ("pgtype", "codec"),
    ("resolve", "codec"),
    ("decode", "codec"),
    ("datetime_in", "codec"),
    ("nested", "codec"),
    ("unrepresentable", "codec"),
    ("batch", "query"),
    ("stream", "query"),
    ("predicate", "query"),
    ("gather", "query"),
    ("prune", "query"),
    ("summary", "query"),
    ("statistics", "query"),
];

/// The graph: what each crate may depend on besides itself. The dump's
/// parsers and the codec are siblings, neither reaching the other.
const DEPENDS: &[(&str, &[&str])] = &[
    ("core", &[]),
    ("model", &["core"]),
    ("io", &["core", "model"]),
    ("dump", &["core", "model", "io"]),
    ("codec", &["core", "model"]),
    ("query", &["core", "model", "io", "dump", "codec"]),
];

/// What a crate below the codec never names: Arrow, and in the core every
/// dependency whose error the core's error type would otherwise carry (D74).
const NO_ARROW: &[&str] = &["arrow::", "arrow_schema", "use arrow"];
const CORE_NAMES_NO_DEPENDENCY: &[&str] = &["xz_seek", "bincode", "tokio", "object_store"];

/// What the model and the codec never name: they do no I/O, and the codec is
/// synchronous (D74).
const NO_IO: &[&str] = &["async fn", "ByteRangeSource", "std::fs", "tokio", "xz_seek"];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn crate_of(module: &str) -> &'static str {
    CRATES
        .iter()
        .find(|(name, _)| *name == module)
        .map(|(_, krate)| *krate)
        .unwrap_or_else(|| panic!("src/{module}.rs has no crate in tests/layering.rs (D68)"))
}

fn depends_on(from: &str, to: &str) -> bool {
    from == to
        || DEPENDS
            .iter()
            .find(|(krate, _)| *krate == from)
            .unwrap_or_else(|| panic!("crate {from} has no entry in DEPENDS"))
            .1
            .contains(&to)
}

/// One file's production code: everything before its first `#[cfg(test)]
/// mod`, with `//` comments removed.
fn production_of(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
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

/// A module's production code: its own file and each production submodule it
/// declares (`mod name;` outside `#[cfg(test)]`), in `src/<module>/`. The
/// crate root's declarations are the modules themselves.
fn production(module: &str) -> String {
    let mut code = production_of(&src_dir().join(format!("{module}.rs")));
    if module == "lib" {
        return code;
    }
    let submodules: Vec<String> = code
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("pub ").unwrap_or(line);
            line.strip_prefix("mod ")?.strip_suffix(';').map(str::to_string)
        })
        .collect();
    for submodule in submodules {
        code.push_str(&production_of(&src_dir().join(module).join(format!("{submodule}.rs"))));
    }
    code
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
    if CRATES.iter().any(|(name, _)| *name == head) {
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

fn modules_in(krate: &str) -> impl Iterator<Item = &'static str> {
    CRATES.iter().filter(move |(_, k)| *k == krate).map(|(module, _)| *module)
}

#[test]
fn every_module_has_a_crate_and_every_crate_entry_a_module() {
    let found = modules();
    for module in &found {
        crate_of(module);
    }
    for (name, _) in CRATES {
        assert!(
            found.iter().any(|m| m == name),
            "tests/layering.rs lists src/{name}.rs, which is gone"
        );
    }
}

#[test]
fn every_use_stays_inside_the_crate_graph() {
    let reexports = reexports();
    let mut crossing = Vec::new();
    for module in modules() {
        let (uses, _) = split_uses(&production(&module));
        for stmt in uses {
            for target in targets(&stmt, &reexports) {
                let (from, to) = (crate_of(&module), crate_of(&target));
                if !depends_on(from, to) {
                    crossing.push(format!("{module} ({from}) uses {target} ({to})"));
                }
            }
        }
    }
    crossing.sort();
    crossing.dedup();
    assert!(crossing.is_empty(), "edges the crate graph does not draw (D68): {crossing:#?}");
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
fn the_crates_below_the_codec_never_name_arrow() {
    for krate in ["core", "model", "io", "dump"] {
        for module in modules_in(krate) {
            let code = production(module);
            for needle in NO_ARROW {
                assert!(!code.contains(needle), "src/{module}.rs ({krate}) names `{needle}` (D74)");
            }
        }
    }
}

#[test]
fn the_core_names_no_dependency_error_could_carry() {
    for module in modules_in("core") {
        let code = production(module);
        for needle in CORE_NAMES_NO_DEPENDENCY {
            assert!(!code.contains(needle), "src/{module}.rs (core) names `{needle}` (D74)");
        }
    }
}

#[test]
fn the_model_does_no_io() {
    for module in modules_in("model") {
        let code = production(module);
        for needle in NO_IO {
            assert!(!code.contains(needle), "src/{module}.rs (model) names `{needle}` (D74)");
        }
    }
}

#[test]
fn the_codec_names_datatypes_only_and_is_synchronous() {
    for module in modules_in("codec") {
        let code = production(module);
        for (at, _) in code.match_indices("arrow::") {
            let after = &code[at + "arrow::".len()..];
            assert!(
                after.starts_with("datatypes"),
                "src/{module}.rs (codec) reaches past `arrow::datatypes` (D74): arrow::{}",
                after.chars().take(24).collect::<String>()
            );
        }
        for needle in NO_IO {
            assert!(!code.contains(needle), "src/{module}.rs (codec) names `{needle}` (D74)");
        }
    }
}
