//! `pgdt` against gzip input, end to end: what the binary does with the file
//! `pg_dump -Fp --compress` writes.

use std::io::Read;
use std::path::Path;

mod common;
use common::{VERSIONS, gzip_fixture, run, run_ok, stderr_of};

/// `gz` decompressed beside itself as `<name>.sql`, by an encoder not ours.
fn plaintext_beside(gz: &Path) -> std::path::PathBuf {
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(gz).unwrap()).read_to_end(&mut plain).unwrap();
    let path = gz.with_extension("");
    std::fs::write(&path, plain).unwrap();
    path
}

/// **A gzip dump reads as its plaintext reads**, at every tested major: a
/// cold `query` runs the first pass and answers what the decompressed file
/// answers, saving a map that tiles what it covers, a `parse` writes the cache beside the dump, and a `query` through
/// that cache answers the same again, and `info` describes what it describes.
#[test]
fn a_gzip_dump_reads_as_its_plaintext() {
    for version in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        let gz = dir.path().join("dump.sql.gz");
        std::fs::copy(gzip_fixture(version, "edge_cases", "compress"), &gz).unwrap();
        let plain = plaintext_beside(&gz);
        let (gz_arg, plain_arg) = (gz.to_str().unwrap(), plain.to_str().unwrap());
        let cache = dir.path().join("dump.sql.gz.dtcache");

        for table in ["public.widgets", "logs.events", "public.escapes"] {
            let expected = run_ok(&["query", "--source", plain_arg, "--table", table]);
            let cold = run_ok(&["query", "--source", gz_arg, "--table", table]);
            assert_eq!(cold, expected, "{version}: {table}, cold");
            assert!(cache.exists(), "{version}: a cold query saves the map it built");
            // Stopped at its table, with the end met by the first pass's
            // reads: the map it saved tiles the file to the extent it saved.
            let info = run_ok(&["info", "--source", gz_arg]);
            assert!(!info.contains("does not tile"), "{version}: {table}: {info}");
            std::fs::remove_file(&cache).unwrap();
            run_ok(&["parse", "--source", gz_arg]);
            let warm = run_ok(&["query", "--source", gz_arg, "--table", table]);
            assert_eq!(warm, expected, "{version}: {table}, through the cache");
        }
        run_ok(&["parse", "--source", plain_arg]);
        let described = |source: &str| {
            let info = run_ok(&["info", "--source", source, "--json"]);
            let info: serde_json::Value = serde_json::from_str(&info).unwrap();
            assert!(info["metadata"].is_object(), "{version}: {source} describes its tables");
            (info["metadata"].clone(), info["scanned_through"].clone())
        };
        assert_eq!(described(gz_arg), described(plain_arg), "{version}: what info describes");

        // Under the spacing, so one unit by the reader's choice: never warned,
        // and described in gzip's words.
        let info: serde_json::Value =
            serde_json::from_str(&run_ok(&["info", "--source", gz_arg, "--json"])).unwrap();
        assert!(non_seekable_warning(&info).is_none(), "{version}: {}", info["diagnostics"]);
        let detail = run_ok(&["info", "--source", gz_arg, "--detail"]);
        assert!(
            detail.contains("compression: gzip — 1 unit(s) in 1 member(s), largest unit "),
            "{version}: {detail}"
        );
    }
}

/// The non-seekable warning an `info --json` document's `diagnostics` carry,
/// if any (`docs/design/decisions.md`, "D19").
fn non_seekable_warning(info: &serde_json::Value) -> Option<serde_json::Value> {
    let diagnostics = info["diagnostics"].as_array().expect("diagnostics is always an array");
    diagnostics.iter().find_map(|d| d["kind"].get("NonSeekableCompressedSource").cloned())
}

/// `plain` as one gzip member holding one dynamic deflate block, as no zlib
/// level writes across this much and another encoder may: each byte a
/// literal but where the next 258 repeat those a 32-byte row back, which is
/// one match of deflate's longest at that distance.
fn one_dynamic_block(plain: &[u8]) -> Vec<u8> {
    struct Bits {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }
    impl Bits {
        /// `bits` low bits of `value`, low bit first, as deflate's fields are.
        fn put(&mut self, value: u32, bits: u32) {
            self.acc |= u64::from(value) << self.n;
            self.n += bits;
            while self.n >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.n -= 8;
            }
        }
        /// A Huffman code, its high bit first.
        fn code(&mut self, code: u32, len: u32) {
            self.put(code.reverse_bits() >> (32 - len), len);
        }
    }
    const ROW: usize = 32;
    // The literal/length code: literals 0–253 of eight bits, and 254, 255,
    // the end of the block and length 258 (symbol 285) of nine — complete,
    // so canonical codes 0–253 and 508–511.
    let nine = [254, 255, 256, 285];
    let symbol = |s: usize| match nine.iter().position(|&n| n == s) {
        Some(i) => (508 + i as u32, 9),
        None => (s as u32, 8),
    };
    let mut lengths = vec![8u8; 254];
    lengths.extend([9, 9, 9]);
    lengths.extend([0; 28]);
    lengths.push(9);
    // The distance code: symbol 9 (25–32, three extra bits) alone, one bit.
    lengths.extend([0; 9]);
    lengths.push(1);
    let mut bits = Bits { out: vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff], acc: 0, n: 0 };
    bits.put(1, 1); // the final block
    bits.put(2, 2); // dynamic
    bits.put(286 - 257, 5);
    bits.put(10 - 1, 5);
    bits.put(18 - 4, 4);
    // The code-length code: 0, 1, 8 and 9, each two bits — 00, 01, 10, 11.
    for s in [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1] {
        bits.put(if [0, 1, 8, 9].contains(&s) { 2 } else { 0 }, 3);
    }
    for &len in &lengths {
        bits.code([0, 1, 8, 9].iter().position(|&l| l == len).unwrap() as u32, 2);
    }
    let mut i = 0;
    while i < plain.len() {
        if i >= ROW && i + 258 <= plain.len() && plain[i..i + 258] == plain[i - ROW..i + 258 - ROW]
        {
            let (code, len) = symbol(285);
            bits.code(code, len);
            bits.code(0, 1);
            bits.put((ROW - 25) as u32, 3);
            i += 258;
        } else {
            let (code, len) = symbol(usize::from(plain[i]));
            bits.code(code, len);
            i += 1;
        }
    }
    let (code, len) = symbol(256);
    bits.code(code, len);
    bits.put(0, 7);
    let mut crc = flate2::Crc::new();
    crc.update(plain);
    let mut member = bits.out;
    member.extend(crc.sum().to_le_bytes());
    member.extend((plain.len() as u32).to_le_bytes());
    member
}

/// **A member whose writer coded twice the shipped spacing in one dynamic
/// block is warned, in gzip's words**: the warning names that unit against
/// the spacing and the remedy.
#[test]
fn a_gzip_dump_its_writer_ended_no_block_in_is_warned() {
    let dir = tempfile::tempdir().unwrap();
    let mut plain = String::from("SET client_encoding = 'UTF8';\n\n");
    plain.push_str(
        "CREATE TABLE public.t (\n    id integer,\n    name text\n);\n\n\
         COPY public.t (id, name) FROM stdin;\n",
    );
    let row = "7\tthe same row, over and over!!\n";
    assert_eq!(row.len(), 32);
    plain.push_str(&row.repeat((52 << 20) / row.len()));
    plain.push_str("\\.\n\n");
    let mut check = Vec::new();
    let member = one_dynamic_block(plain.as_bytes());
    flate2::read::GzDecoder::new(&member[..]).read_to_end(&mut check).unwrap();
    assert!(check == plain.as_bytes(), "the member decodes, by an inflater not ours");
    let gz = dir.path().join("one-block.sql.gz");
    std::fs::write(&gz, member).unwrap();
    let gz_arg = gz.to_str().unwrap();

    let parsed = run_ok(&["parse", "--source", gz_arg]);
    assert!(parsed.contains("[warning] this gzip source has no seek structure"), "{parsed}");
    let info: serde_json::Value =
        serde_json::from_str(&run_ok(&["info", "--source", gz_arg, "--json"])).unwrap();
    let warning = non_seekable_warning(&info).unwrap_or_else(|| panic!("{}", info["diagnostics"]));
    let spacing = warning["container"]["Gzip"]["spacing"].as_u64().expect("gzip's container");
    let largest = warning["largest_unit"].as_u64().unwrap();
    assert!(largest > 2 * spacing, "{warning}");
    assert_eq!(warning["block_count"], info["compression"]["blocks"], "{warning}");
    assert_eq!(largest, info["compression"]["max_block_uncompressed"].as_u64().unwrap());
}

/// The status lines naming a gzip first pass in `stderr`: those saying it
/// started, and those saying it completed.
fn first_pass_lines(stderr: &str) -> (Vec<&str>, Vec<&str>) {
    let lines = |what: &str| stderr.lines().filter(|l| l.contains(what)).collect::<Vec<_>>();
    (lines("first pass started"), lines("first pass complete"))
}

/// `args` run to success, and what it wrote to stderr.
fn stderr_ok(args: &[&str]) -> String {
    let output = run(args);
    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{args:?}: {stderr}");
    stderr
}

/// **A gzip first pass says it ran, once, and why the run is serial**, as the
/// `.xz` walk does (`docs/design/decisions.md`, "D64"): a cold `parse` of a
/// file of one chunk says it started, naming the codec and that it reads
/// serially while it builds the index, and that it completed, naming the units and members the index
/// holds; a `parse` and an `info` over the complete cache it saved say
/// nothing of it.
#[test]
fn a_gzip_first_pass_is_announced_once_started_and_complete() {
    let dir = tempfile::tempdir().unwrap();
    let gz = dir.path().join("dump.sql.gz");
    std::fs::copy(gzip_fixture(16, "edge_cases", "compress"), &gz).unwrap();
    let gz_arg = gz.to_str().unwrap();

    let stderr = stderr_ok(&["parse", "--source", gz_arg]);
    let (started, complete) = first_pass_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 1), "{stderr}");
    assert!(started[0].contains("codec=gzip"), "{stderr}");
    assert!(started[0].contains("reading serially while it builds the index"), "{stderr}");
    assert!(!started[0].contains("resum"), "a fresh pass resumes nothing: {stderr}");
    assert!(complete[0].contains("units=1 members=1"), "{stderr}");
    assert!(stderr.find("first pass started") < stderr.find("first pass complete"), "{stderr}");

    for args in [["parse", "--source", gz_arg], ["info", "--source", gz_arg]] {
        let stderr = stderr_ok(&args);
        assert!(!stderr.contains("first pass"), "{args:?}: {stderr}");
    }
}

/// A dump of two tables, `small` then `large` of `rows` rows, compressed as
/// one gzip member by `flate2` at its default level.
fn two_table_gzip(dir: &Path, rows: u64) -> std::path::PathBuf {
    use std::io::Write as _;
    let mut plain = String::from("SET client_encoding = 'UTF8';\n\n");
    for (table, rows) in [("small", 50u64), ("large", rows)] {
        plain.push_str(&format!(
            "CREATE TABLE public.{table} (\n    id integer,\n    name text\n);\n\n\
             COPY public.{table} (id, name) FROM stdin;\n"
        ));
        for id in 1..=rows {
            plain.push_str(&format!(
                "{id}\trow {id} of {table}, {}\n",
                id.wrapping_mul(7919) % 10_007
            ));
        }
        plain.push_str("\\.\n\n");
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plain.as_bytes()).unwrap();
    let gz = dir.join("dump.sql.gz");
    std::fs::write(&gz, encoder.finish().unwrap()).unwrap();
    gz
}

/// **A first pass says whether it inflates in parallel**: over a local file
/// of several chunks, a cold `parse` at three workers says it inflates in
/// parallel on them, its scan one reader, and at one that it reads serially;
/// both build one index.
#[test]
fn a_gzip_first_pass_says_whether_it_inflates_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let gz = two_table_gzip(dir.path(), 1_000_000);
    assert!(std::fs::metadata(&gz).unwrap().len() > 2 << 20, "three chunks or more");
    let gz_arg = gz.to_str().unwrap();
    let mut described = Vec::new();
    for (jobs, says) in [
        ("3", "inflating in parallel while it builds the index, its scan one reader"),
        ("1", "reading serially while it builds the index"),
    ] {
        let cache = dir.path().join(format!("at-{jobs}.dtcache"));
        let cache_arg = cache.to_str().unwrap();
        let stderr =
            stderr_ok(&["parse", "--source", gz_arg, "--jobs", jobs, "--dtcache", cache_arg]);
        let (started, complete) = first_pass_lines(&stderr);
        assert_eq!((started.len(), complete.len()), (1, 1), "{jobs}: {stderr}");
        assert!(started[0].contains(says), "{jobs}: {stderr}");
        assert_eq!(started[0].contains("workers=3"), jobs == "3", "{jobs}: {stderr}");
        let info = run_ok(&["info", "--source", gz_arg, "--dtcache", cache_arg, "--detail"]);
        described.push(info);
    }
    assert_eq!(described[0], described[1]);
}

/// **A resumed first pass names the checkpoint it resumes from**: a cold
/// `query` stopped at a small table ahead of a large one saves an index
/// reaching only as far as its pass got, saying it started and not that it
/// completed, and the `parse` that resumes it says from which checkpoint,
/// then that it completed.
#[test]
fn a_resumed_gzip_first_pass_names_its_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let gz = two_table_gzip(dir.path(), 200_000);
    let gz_arg = gz.to_str().unwrap();

    let output = run(&["query", "--source", gz_arg, "--table", "public.small"]);
    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{stderr}");
    let (started, complete) = first_pass_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 0), "stopped short of the end: {stderr}");

    let stderr = stderr_ok(&["parse", "--source", gz_arg]);
    let (started, complete) = first_pass_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 1), "{stderr}");
    assert!(started[0].contains("resuming from a saved checkpoint"), "{stderr}");
    assert!(started[0].contains("checkpoint=0 plaintext=0"), "{stderr}");
    assert!(complete[0].contains("members=1"), "{stderr}");
}

/// `plain` written as BGZF beside it, by `noodles-bgzf`'s writer — a member
/// to each 64 KiB of plaintext and the empty member that ends the file.
fn bgzf_beside(plain: &Path) -> std::path::PathBuf {
    use std::io::Write as _;
    let mut writer = noodles_bgzf::io::Writer::new(Vec::new());
    writer.write_all(&std::fs::read(plain).unwrap()).unwrap();
    let path = plain.with_extension("sql.bgz");
    std::fs::write(&path, writer.finish().unwrap()).unwrap();
    path
}

/// The status lines naming a BGZF walk in `stderr`: those saying it started,
/// and those saying it completed.
fn walk_lines(stderr: &str) -> (Vec<&str>, Vec<&str>) {
    let lines = |what: &str| stderr.lines().filter(|l| l.contains(what)).collect::<Vec<_>>();
    (lines("BGZF member walk started"), lines("BGZF member walk complete"))
}

/// **A BGZF dump is walked at open and reads as its plaintext**: a cold
/// `query` answers what the decompressed file answers, a cold `parse` says
/// once that its members' walk started and that it completed — counting every
/// member, the empty one ending the file included, not only the one unit's —
/// and runs no first pass, `info` describes
/// it in gzip's words with every member counted, and a `parse` and an `info`
/// over its cache say nothing of the walk (`docs/design/decisions.md`,
/// "D64").
#[test]
fn a_bgzf_dump_is_walked_at_open_and_reads_as_its_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let gz = dir.path().join("dump.sql.gz");
    std::fs::copy(gzip_fixture(16, "edge_cases", "compress"), &gz).unwrap();
    let plain = plaintext_beside(&gz);
    let bgz = bgzf_beside(&plain);
    let (bgz_arg, plain_arg) = (bgz.to_str().unwrap(), plain.to_str().unwrap());

    for table in ["public.widgets", "logs.events", "public.escapes"] {
        let expected = run_ok(&["query", "--source", plain_arg, "--table", table]);
        let cold = ["query", "--source", bgz_arg, "--table", table, "--dtcache", "none"];
        assert_eq!(run_ok(&cold), expected, "{table}, cold");
    }

    let stderr = stderr_ok(&["parse", "--source", bgz_arg]);
    let (started, complete) = walk_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 1), "{stderr}");
    assert!(started[0].contains("codec=gzip"), "{stderr}");
    let members: u64 = complete[0]
        .split_once("units=1 members=")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("one unit and its members: {stderr}"));
    // The plaintext's member or members, and the empty one ending the file.
    assert!(members >= 2, "{stderr}");
    assert!(!stderr.contains("first pass"), "a BGZF file is walked, not passed: {stderr}");
    let detail = run_ok(&["info", "--source", bgz_arg, "--detail"]);
    let shape = format!("compression: gzip — 1 unit(s) in {members} member(s), largest unit ");
    assert!(detail.contains(&shape), "{detail}");
    for args in [["parse", "--source", bgz_arg], ["info", "--source", bgz_arg]] {
        let stderr = stderr_ok(&args);
        assert!(!stderr.contains("BGZF member walk"), "{args:?}: {stderr}");
    }
    for table in ["public.widgets", "logs.events"] {
        let expected = run_ok(&["query", "--source", plain_arg, "--table", table]);
        let warm = run_ok(&["query", "--source", bgz_arg, "--table", table]);
        assert_eq!(warm, expected, "{table}, through the cache");
    }
}
