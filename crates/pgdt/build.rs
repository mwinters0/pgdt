//! Names the Cargo profile this binary is built under, for `pgdt --version`
//! (`src/profile.rs`), and compiles the shim naming mimalloc's purge delay
//! where mimalloc is linked (`src/alloc/purge.c`).
//!
//! Cargo tells a build script no profile name: `PROFILE` is `release` for
//! every profile inheriting `release`, so the three `release-profile` legs
//! would all read alike. What does differ is the directory Cargo builds a
//! profile into, which `OUT_DIR` sits under —
//! `<target-dir>/[<triple>/]<profile-dir>/build/<package>-<hash>/out` — and
//! that directory is the profile's name, bar `dev`'s, which is `debug`. A
//! test builds into `dev`'s and a bench into `release`'s, and read as those.
//!
//! An `OUT_DIR` of any other shape names `unknown` rather than failing the
//! build, so a Cargo that moves its layout breaks no one's build of `pgdt`;
//! `profile.rs`'s test is what fails then, and the harness refuses a leg that
//! does not name the profile it was built under.
//!
//! The shim is compiled against the header `libmimalloc-sys` built mimalloc
//! from, which it exports as `DEP_MIMALLOC_INCLUDE_DIR` (`links = "mimalloc"`)
//! to a package depending on it directly, as this one does under `mimalloc`
//! and `introspect`; a build linking another allocator compiles none.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR for a build script");
    println!("cargo:rustc-env=PGDT_PROFILE={}", profile_of(Path::new(&out)));
    if std::env::var_os("CARGO_FEATURE_MIMALLOC").is_some()
        || std::env::var_os("CARGO_FEATURE_INTROSPECT").is_some()
    {
        purge_shim();
    }
}

/// `src/alloc/purge.c`, compiled against mimalloc's own `mimalloc.h`.
// upstream: UF6
fn purge_shim() {
    const SHIM: &str = "src/alloc/purge.c";
    println!("cargo:rerun-if-changed={SHIM}");
    let include = std::env::var_os("DEP_MIMALLOC_INCLUDE_DIR")
        .expect("libmimalloc-sys exports its include directory to a direct dependent");
    cc::Build::new().file(SHIM).include(include).warnings_into_errors(true).compile("pgdt_purge");
}

fn profile_of(out: &Path) -> &str {
    let mut up = out.ancestors().skip(2).map(Path::file_name);
    match (up.next().flatten(), up.next().flatten().and_then(|d| d.to_str())) {
        (Some(build), Some("debug")) if build == "build" => "dev",
        (Some(build), Some(dir)) if build == "build" => dir,
        _ => "unknown",
    }
}
