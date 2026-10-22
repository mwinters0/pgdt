//! `gzip-inflate` run as the harness runs it, on members `flate2` writes: each
//! backend held to the plaintext's length, and to refusing what is not one
//! whole, intact member.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

use flate2::{Compression, GzBuilder};

const GZIP_INFLATE: &str = env!("CARGO_BIN_EXE_gzip-inflate");

/// Every backend the instrument takes, as `--backend` spells it.
const BACKENDS: [&str; 3] = ["zlib-ng", "zlib-rs", "miniz-oxide"];

/// Several of the instrument's read and write buffers, and compressible the
/// way a dump is, so a member spans many calls and many deflate blocks.
fn plaintext() -> Vec<u8> {
    let mut text = Vec::new();
    let mut i: u64 = 0;
    while text.len() < 5 << 20 {
        writeln!(text, "{i}\trow {}\t{}", i.wrapping_mul(2654435761) % 1_000_003, i % 7).unwrap();
        i += 1;
    }
    text
}

/// One member, its header carrying a name, as `pg_dump -Z` does not but a
/// header parser has to skip.
fn member(plain: &[u8]) -> Vec<u8> {
    let mut out = GzBuilder::new().filename("control").write(Vec::new(), Compression::new(6));
    out.write_all(plain).unwrap();
    out.finish().unwrap()
}

fn run(file: &Path, backend: &str) -> Output {
    Command::new(GZIP_INFLATE)
        .args(["--source", file.to_str().unwrap(), "--backend", backend])
        .output()
        .expect("gzip-inflate starts")
}

fn written(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

#[test]
fn every_backend_delivers_the_whole_plaintext() {
    let plain = plaintext();
    let gz = member(&plain);
    let file = written(&gz);
    for backend in BACKENDS {
        let out = run(file.path(), backend);
        assert!(out.status.success(), "{backend}: {}", stderr(&out));
        assert_eq!(
            stdout(&out),
            format!("backend={backend}\ncompressed={}\ndelivered={}\n", gz.len(), plain.len()),
        );
    }
}

#[test]
fn a_second_member_is_refused_by_every_backend() {
    let gz = member(&plaintext());
    let file = written(&[gz.as_slice(), gz.as_slice()].concat());
    for backend in BACKENDS {
        let out = run(file.path(), backend);
        assert_eq!(out.status.code(), Some(1), "{backend}");
        assert!(stderr(&out).contains("follow the first member"), "{backend}: {}", stderr(&out));
    }
}

#[test]
fn a_wrong_crc_is_refused_by_every_backend() {
    let mut gz = member(&plaintext());
    let crc = gz.len() - 8;
    gz[crc] ^= 0xff;
    let file = written(&gz);
    for backend in BACKENDS {
        let out = run(file.path(), backend);
        assert_eq!(out.status.code(), Some(1), "{backend}: {}", stdout(&out));
    }
}

#[test]
fn a_truncated_member_is_refused_by_every_backend() {
    let gz = member(&plaintext());
    let file = written(&gz[..gz.len() / 2]);
    for backend in BACKENDS {
        let out = run(file.path(), backend);
        assert_eq!(out.status.code(), Some(1), "{backend}: {}", stdout(&out));
        assert!(stderr(&out).contains("ends inside the member"), "{backend}: {}", stderr(&out));
    }
}

#[test]
fn an_unknown_backend_is_a_usage_error_naming_every_backend() {
    let file = written(&member(b"x"));
    let out = run(file.path(), "zlib");
    assert_eq!(out.status.code(), Some(2));
    for backend in BACKENDS {
        assert!(stderr(&out).contains(backend), "{}", stderr(&out));
    }
}
