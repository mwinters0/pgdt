//! Names the Cargo profile this binary is built under, for `pgdt --version`
//! (`src/profile.rs`).
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

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR for a build script");
    println!("cargo:rustc-env=PGDT_PROFILE={}", profile_of(Path::new(&out)));
}

fn profile_of(out: &Path) -> &str {
    let mut up = out.ancestors().skip(2).map(Path::file_name);
    match (up.next().flatten(), up.next().flatten().and_then(|d| d.to_str())) {
        (Some(build), Some("debug")) if build == "build" => "dev",
        (Some(build), Some(dir)) if build == "build" => dir,
        _ => "unknown",
    }
}
