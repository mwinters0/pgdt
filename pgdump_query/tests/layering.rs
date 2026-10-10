//! The crate check: `docs/design/decisions.md`, "D68" and "D74", read off
//! `crates/pgdt-*/`. Cargo refuses a cycle but not an edge the graph does not
//! draw, so each crate's `[dependencies]` are held to [`DEPENDS`]; and what a
//! dependency list cannot say — that the model and the codec do no I/O, that
//! the core names no dependency its error would carry — is read off each
//! crate's production code, comments stripped, so a doc link is not a use.

use std::fs;
use std::path::{Path, PathBuf};

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

/// What a crate below the query layer never depends on: Arrow, whose schema
/// types and `i256` alone the codec takes, from `arrow-schema` and
/// `arrow-buffer` (D74).
const ARROW_ABOVE_THE_CODEC: &[&str] = &["arrow"];
const ARROW_BELOW_THE_CODEC: &[&str] = &["arrow", "arrow-schema", "arrow-buffer"];

/// What the core never names: every dependency whose error the core's error
/// type would otherwise carry (D74).
const CORE_NAMES_NO_DEPENDENCY: &[&str] = &["xz_seek", "bincode", "tokio", "object_store"];

/// What the model and the codec never name: they do no I/O, and the codec is
/// synchronous (D74).
const NO_IO: &[&str] = &["async fn", "ByteRangeSource", "std::fs", "tokio", "xz_seek"];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates")
}

fn crate_dir(krate: &str) -> PathBuf {
    crates_dir().join(format!("pgdt-{krate}"))
}

/// The names under one crate's `[dependencies]` table, its dev-dependencies
/// not among them: a test may reach what the library may not.
fn dependencies(krate: &str) -> Vec<String> {
    let manifest = fs::read_to_string(crate_dir(krate).join("Cargo.toml")).unwrap();
    let mut names = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[dependencies]";
            continue;
        }
        if inside
            && !line.starts_with('#')
            && let Some((name, _)) = line.split_once('=')
        {
            let name = name.trim();
            if !name.is_empty() && !name.contains(' ') {
                names.push(name.to_string());
            }
        }
    }
    names
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

/// A crate's production code: each file a production `mod name;` declares,
/// from the crate root down, at the file a `#[path = "…"]` before it names
/// where one does. A module declared under `#[cfg(test)]` is a test file and
/// is not read.
fn production(krate: &str) -> Vec<(PathBuf, String)> {
    let src = crate_dir(krate).join("src");
    let mut out = Vec::new();
    let mut pending = vec![(src.join("lib.rs"), src.clone())];
    while let Some((file, dir)) = pending.pop() {
        let code = production_of(&file);
        let mut named: Option<&str> = None;
        for line in code.lines() {
            let line = line.trim();
            if let Some(path) = line.strip_prefix("#[path = \"").and_then(|l| l.strip_suffix("\"]"))
            {
                named = Some(path);
                continue;
            }
            let line = line.strip_prefix("pub ").unwrap_or(line);
            if let Some(name) = line.strip_prefix("mod ").and_then(|l| l.strip_suffix(';')) {
                let child = match named {
                    Some(path) => dir.join(path),
                    None => dir.join(format!("{name}.rs")),
                };
                pending.push((child, dir.join(name)));
            }
            named = None;
        }
        out.push((file, code));
    }
    out
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

#[test]
fn every_crate_is_in_the_graph() {
    let mut found: Vec<String> = fs::read_dir(crates_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter_map(|name| name.strip_prefix("pgdt-").map(str::to_string))
        .collect();
    found.sort();
    let mut listed: Vec<String> = DEPENDS.iter().map(|(krate, _)| krate.to_string()).collect();
    listed.sort();
    assert_eq!(found, listed, "every crates/pgdt-* has an entry in DEPENDS, and only those (D68)");
}

#[test]
fn every_dependency_stays_inside_the_crate_graph() {
    let mut crossing = Vec::new();
    for (from, _) in DEPENDS {
        for name in dependencies(from) {
            if let Some(to) = name.strip_prefix("pgdt-")
                && !depends_on(from, to)
            {
                crossing.push(format!("pgdt-{from} depends on {name}"));
            }
        }
    }
    assert!(crossing.is_empty(), "edges the crate graph does not draw (D68): {crossing:#?}");
}

#[test]
fn only_the_query_layer_depends_on_arrow() {
    for (krate, _) in DEPENDS {
        let refused = match *krate {
            "query" => continue,
            "codec" => ARROW_ABOVE_THE_CODEC,
            _ => ARROW_BELOW_THE_CODEC,
        };
        for name in dependencies(krate) {
            assert!(!refused.contains(&name.as_str()), "pgdt-{krate} depends on {name} (D74)");
        }
    }
}

#[test]
fn the_core_names_no_dependency_its_error_could_carry() {
    for (file, code) in production("core") {
        for needle in CORE_NAMES_NO_DEPENDENCY {
            assert!(!code.contains(needle), "{} (core) names `{needle}` (D74)", file.display());
        }
    }
}

#[test]
fn the_model_and_the_codec_do_no_io() {
    for krate in ["model", "codec"] {
        for (file, code) in production(krate) {
            for needle in NO_IO {
                assert!(
                    !code.contains(needle),
                    "{} ({krate}) names `{needle}` (D74)",
                    file.display()
                );
            }
        }
    }
}
