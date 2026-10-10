use super::*;
use arrow::datatypes::{Field, Fields};
use std::sync::Arc;

/// A build says it is unreleased exactly when the release variable is
/// absent, in the place the spec gives it: straight after the crate
/// version, the markers the harness reads following it unchanged.
#[test]
fn the_version_says_unreleased_unless_built_as_a_release() {
    let crate_version = env!("CARGO_PKG_VERSION");
    let unreleased = format!("{crate_version} (unreleased) {}", alloc::MARKERS);
    let released = format!("{crate_version} {}", alloc::MARKERS);
    let expected = if RELEASE_VARIABLE.is_none() { &unreleased } else { &released };
    assert!(version().starts_with(expected.as_str()), "{}", version());
    assert_eq!(RELEASED, RELEASE_VARIABLE.is_some());
}

/// The comparison the release variable is held to, which is `const` and
/// so hand-written.
#[test]
fn same_str_is_str_equality() {
    for (a, b) in
        [("0.1.0", "0.1.0"), ("0.1.0", "0.1.1"), ("0.1.0", "0.1.0 "), ("", "0.1.0"), ("", "")]
    {
        assert_eq!(same_str(a, b), a == b, "{a:?} against {b:?}");
    }
}

/// The `--strict-identity` grammar, both ways: what a selection binds, and
/// that `advisory` and `none` are exclusive rather than overrides — naming
/// either beside another term is a contradiction, and a contradiction is
/// refused rather than resolved (`docs/design/decisions.md`, "D60" is the
/// precedent for refusing at the flag). Every selection reads back as it
/// is written.
#[test]
fn the_strict_identity_selection_is_read_and_its_contradictions_refused() {
    let binds = |text: &str| parse_strict_identity(text).unwrap();
    assert_eq!(binds("time"), StrictIdentity::binding(true, false));
    assert_eq!(binds("location"), StrictIdentity::binding(false, true));
    assert_eq!(binds("time,location"), StrictIdentity::binding(true, true));
    assert_eq!(binds("location, time"), StrictIdentity::binding(true, true));
    assert_eq!(binds("none"), StrictIdentity::NONE);
    assert_eq!(binds("advisory"), StrictIdentity::ADVISORY);
    // The in-flight check is on under every selection but `none`.
    assert!(binds("time").in_flight() && binds("location").in_flight());
    assert!(binds("advisory").in_flight());
    assert!(!binds("none").in_flight());
    for text in ["time", "location", "time,location", "advisory", "none"] {
        assert_eq!(binds(text).to_string(), text, "a selection reads back as written");
    }
    // What the bare flag means, spelled as clap substitutes it (the test
    // below reads the attribute).
    assert_eq!(binds("time,location"), StrictIdentity::binding(true, true));

    assert!(parse_strict_identity("none,time").is_err(), "`none` is exclusive");
    assert!(parse_strict_identity("advisory,time").is_err(), "`advisory` is exclusive");
    assert!(parse_strict_identity("advisory,none").is_err(), "and so is the pair");
    let unknown = parse_strict_identity("path").unwrap_err();
    assert!(unknown.contains("`advisory`"), "an unknown term names the four: {unknown}");
    assert!(parse_strict_identity("time,").is_err(), "an empty term is not silence");
    assert!(parse_strict_identity("").is_err());
}

/// **The source's name is the CLI's to add.** The library detects a
/// source moving underneath a run wherever the check is cheapest and has
/// no name for what it was reading; every other error keeps its own
/// words. Which is which is an exhaustive match, so the compiler is what
/// pins the coverage and this test pins the rendering — one case per
/// refusal that fires *because the source moved*, since each is raised by
/// a different mechanism and names a different thing by itself.
#[test]
fn only_the_refusals_about_a_source_are_given_its_name() {
    let origin = Origin::local("/tmp/koji.dump");
    let named = |err: pgdump_query::Error| naming_the_source(err, &origin).to_string();

    let said = named(pgdump_query::Error::SourceChangedWhileRead {
        differences: "its modification time moved".into(),
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    assert!(said.contains("while it was being read"), "{said}");

    // The cache names itself; what it cannot name is the dump it was
    // checked against, which is the whole point where the path was
    // derived from a URL.
    let said = named(pgdump_query::Error::CacheUnusable {
        path: PathBuf::from("koji.dump.dtcache"),
        unusable: Unusable::SourceChanged { cached_stored_size: 10, live_stored_size: 11 },
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    assert!(said.contains("koji.dump.dtcache"), "the cache is still named: {said}");

    // A bound weak signal that does not hold: the same derived cache
    // path, and a clause about times rather than sizes.
    let said = named(pgdump_query::Error::StrictIdentityUnmet {
        path: PathBuf::from("koji.dump.dtcache"),
        term: "time",
        unmet: "the modification time has moved".into(),
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    assert!(said.contains("koji.dump.dtcache"), "the cache is still named: {said}");

    // Both arms of the block refusal, the cache-less one naming nothing
    // at all by itself — the case with most to gain.
    let said = named(pgdump_query::Error::CachedBlockChanged {
        path: Some(PathBuf::from("koji.dump.dtcache")),
        header_offset: 4096,
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    let said = named(pgdump_query::Error::CachedBlockChanged { path: None, header_offset: 4096 });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");

    let other = pgdump_query::Error::CacheDisabled { operation: "parse" };
    let verbatim = other.to_string();
    assert_eq!(named(other), verbatim);
}

/// The bare flag is `time,location`, stated once — in the attribute — and
/// read back here, so what it binds is pinned.
#[test]
fn the_bare_strict_identity_flag_binds_both_terms() {
    let cli = Cli::try_parse_from(["pgdt", "parse", "--source", "d.sql", "--strict-identity"])
        .expect("the flag takes no value");
    let Invocation::Native(Command::Parse { identity, .. }) = cli.command else { panic!("parse") };
    assert_eq!(identity.resolve(), StrictIdentity::binding(true, true));

    let cli = Cli::try_parse_from(["pgdt", "parse", "--source", "d.sql"]).unwrap();
    let Invocation::Native(Command::Parse { identity, .. }) = cli.command else { panic!("parse") };
    assert_eq!(identity.resolve(), StrictIdentity::ADVISORY);
}

/// Every numeric flag on every command, classified — the check behind
/// `docs/design/roadmap.md`, "Two tunables fit pgdt to hardware: memory and
/// parallelism".
///
/// **Four classes, because the rule is about what a person must *state*,
/// not about what exists.** `Hardware` is the closed pair the rule names,
/// and a third one is a decision somebody has to write here first.
/// `Intent` says what work to do, `Contract` states a property of the
/// input, and `Expert` is an override already shipped that the rule
/// tolerates and does not extend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagClass {
    Hardware,
    Intent,
    Contract,
    Expert,
}

/// The value names a *number* is spelled with here. Pinned as a set so a
/// flag taking a count under some fresh spelling (`MIB`, `COUNT`) fails
/// [`every_numeric_flag_is_classified`] rather than slipping past the
/// allowlist by not looking numeric.
const NUMERIC_VALUE_NAMES: &[&str] = &["BYTES", "N", "ROWS"];

/// The value names that take something other than a number, listed for the
/// same reason: the two lists together are what makes the numeric one
/// exhaustive.
const NON_NUMERIC_VALUE_NAMES: &[&str] = &[
    "LEVELS",
    "EXPR",
    "NAME",
    "USE",
    "SOURCE",
    "TABLE",
    "DATABASE",
    "DTCACHE",
    "FILTER",
    "SCHEMA_MODE",
    "MODE",
    "TERMS",
];

/// The classification the two-tunable rule is checked against. Adding a
/// row is the decision the rule exists to make visible.
const FLAG_CLASSES: &[(&str, FlagClass)] = &[
    ("jobs", FlagClass::Hardware),
    ("memory", FlagClass::Hardware),
    ("row-group-size", FlagClass::Intent),
    ("row-group-min-rows", FlagClass::Intent),
    ("row-group-max-rows", FlagClass::Intent),
    ("max-line-bytes", FlagClass::Contract),
    ("chunk-size", FlagClass::Expert),
];

/// The subcommands the rule does not reach: `sql`'s flags are DataFusion's
/// CLI's as upstream ships it, bar two of ours taking no number
/// (`docs/design/roadmap.md`, "Two tunables fit pgdt to hardware: memory
/// and parallelism").
const UNCLASSIFIED_SUBCOMMANDS: &[&str] = &["sql"];

/// Every `(flag, value_name)` this CLI takes a value under, over every
/// subcommand but [`UNCLASSIFIED_SUBCOMMANDS`], deduplicated —
/// `--chunk-size` is on two commands and is one flag.
fn valued_flags() -> BTreeMap<String, String> {
    use clap::CommandFactory;
    fn walk(cmd: &clap::Command, into: &mut BTreeMap<String, String>) {
        for arg in cmd.get_arguments() {
            // A value-taking option, so a `SetTrue` switch is out: `clap`
            // gives every argument a value name, the flag's own spelling
            // upper-cased where none was stated.
            if !matches!(arg.get_action(), clap::ArgAction::Set | clap::ArgAction::Append) {
                continue;
            }
            if let Some(names) = arg.get_value_names()
                && let (Some(long), Some(value)) = (arg.get_long(), names.first())
            {
                into.insert(long.to_string(), value.to_string());
            }
        }
        for sub in cmd.get_subcommands() {
            if !UNCLASSIFIED_SUBCOMMANDS.contains(&sub.get_name()) {
                walk(sub, into);
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(&Cli::command(), &mut found);
    found
}

/// **A third hardware knob is a decision somebody wrote down, never one
/// that arrived quietly** (`docs/design/roadmap.md`, "Two tunables fit
/// pgdt to hardware: memory and parallelism").
///
/// Closed at both ends, which is what makes it a check rather than a
/// reminder: a numeric flag nobody classified fails, a classification
/// naming a flag that no longer exists fails, and a value name in neither
/// list fails — the last being how a new flag would otherwise escape by
/// spelling its count something new.
#[test]
fn every_numeric_flag_is_classified() {
    let flags = valued_flags();

    let unknown: Vec<_> = flags
        .iter()
        .filter(|(_, value)| {
            !NUMERIC_VALUE_NAMES.contains(&value.as_str())
                && !NON_NUMERIC_VALUE_NAMES.contains(&value.as_str())
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "a value name in neither list: {unknown:?} — say whether it is a number"
    );

    let classified: BTreeMap<_, _> = FLAG_CLASSES.iter().copied().collect();
    let numeric: BTreeSet<_> = flags
        .iter()
        .filter(|(_, value)| NUMERIC_VALUE_NAMES.contains(&value.as_str()))
        .map(|(flag, _)| flag.as_str())
        .collect();

    let unclassified: Vec<_> = numeric.iter().filter(|f| !classified.contains_key(*f)).collect();
    assert!(
        unclassified.is_empty(),
        "a flag taking a number that nobody classified: {unclassified:?}"
    );
    let stale: Vec<_> = classified.keys().filter(|f| !numeric.contains(*f)).collect();
    assert!(stale.is_empty(), "a classification naming no flag: {stale:?}");

    // The rule's own content: the hardware pair is closed.
    let hardware: BTreeSet<_> = FLAG_CLASSES
        .iter()
        .filter(|(_, class)| *class == FlagClass::Hardware)
        .map(|(flag, _)| *flag)
        .collect();
    assert_eq!(
        hardware,
        BTreeSet::from(["jobs", "memory"]),
        "a third hardware knob needs the rule changed first, not this list"
    );
}

/// **The exemption is `sql`'s alone**: one subcommand, which exists, and
/// whose two flags of ours take no number — so neither a command of
/// `pgdt`'s own nor a knob added to `sql` escapes the rule through it.
#[test]
fn the_two_tunables_rule_skips_sql_alone() {
    use clap::CommandFactory;
    assert_eq!(UNCLASSIFIED_SUBCOMMANDS, ["sql"]);
    let cli = Cli::command();
    let sql = cli.find_subcommand("sql").expect("`pgdt sql` exists");
    let ours: Vec<_> = sql
        .get_arguments()
        .filter(|arg| matches!(arg.get_long(), Some("dump" | "strict-identity")))
        .map(|arg| {
            let value = arg.get_value_names().and_then(|n| n.first()).map(|n| n.to_string());
            (arg.get_long().unwrap_or_default(), value.unwrap_or_default())
        })
        .collect();
    assert_eq!(ours.len(), 2, "`sql` lost one of its flags of ours: {ours:?}");
    for (flag, value) in ours {
        assert!(
            !NUMERIC_VALUE_NAMES.contains(&value.as_str()),
            "`sql --{flag}` takes a number, which the rule's exemption does not cover"
        );
    }
}

/// One parsed comparison term, or the message it was refused with.
fn filter(spec: &str) -> Result<(String, PredicateOp, Option<String>), String> {
    match parse_filter(spec) {
        Ok(Expr::Term(p)) => Ok((p.column, p.op, p.value)),
        Ok(other) => panic!("`{spec}` should be a comparison, parsed as {other:?}"),
        Err(e) => Err(e.to_string()),
    }
}

/// The column and values of a membership term that must parse.
fn membership(spec: &str) -> (String, Vec<String>) {
    match parse_filter(spec) {
        Ok(Expr::In(m)) => (m.column, m.values.into_iter().map(Option::unwrap).collect()),
        other => panic!("`{spec}` should be a membership, got {other:?}"),
    }
}

/// The message a membership term was refused with.
fn membership_err(spec: &str) -> String {
    match parse_filter(spec) {
        Err(e) => e.to_string(),
        Ok(parsed) => panic!("`{spec}` should be refused, parsed as {parsed:?}"),
    }
}

/// **`IN` is a worded operator before a list**, found where the others
/// are, the earliest winning; each value is read as `=`'s is — trimmed,
/// or quoted to keep its spaces, its commas or its parens — and `null`
/// is the text `null`, `=` having no NULL either.
#[test]
fn in_reads_a_list_of_values_as_equality_reads_one() {
    let own = |c: &str, vs: &[&str]| (c.to_string(), vs.iter().map(|v| v.to_string()).collect());
    assert_eq!(membership("id in (1, 2,3)"), own("id", &["1", "2", "3"]));
    assert_eq!(membership("id IN(7)"), own("id", &["7"]));
    assert_eq!(membership("id   In   ( 7 )  "), own("id", &["7"]));
    assert_eq!(
        membership(r#"note in (' a, b ', "it""s", '(1,2)', null, '')"#),
        own("note", &[" a, b ", r#"it"s"#, "(1,2)", "null", ""])
    );
    // An unquoted value keeps a quote that does not open it, as `=` does.
    assert_eq!(membership("note in (don't, x)"), own("note", &["don't", "x"]));
    assert_eq!(membership(r#""a in (b)" in (c)"#), own("a in (b)", &["c"]));
    // The earliest operator wins: an `=` before the word keeps the term
    // a comparison, and the word needs whitespace before it and a paren
    // after it to be one.
    assert_eq!(ok("note=x in (y)"), ("note".into(), PredicateOp::Eq, Some("x in (y)".into())));
    assert!(err("tin (y)").contains("--filter must be"));
    assert!(err("x in y").contains("--filter must be"));
    for (spec, says) in [
        ("x in ()", "at least one value"),
        ("x in ( )", "at least one value"),
        ("x in (1,,2)", "empty value"),
        ("x in (1,)", "empty value"),
        ("x in (1, 2", "never closed"),
        ("x in ('1)", "never closed"),
        ("x in (1) 2", "follows the `IN` list"),
        ("x in ((1,2))", "holds a `(`"),
        ("x in ('a'b)", "unbalanced `'` quote"),
    ] {
        let message = membership_err(spec);
        assert!(message.contains(says), "`{spec}`: {message}");
    }
}

/// `column`, `op` and `value` of a term that must parse.
fn ok(spec: &str) -> (String, PredicateOp, Option<String>) {
    filter(spec).unwrap_or_else(|e| panic!("`{spec}` should parse: {e}"))
}

/// The message a term that must not parse was refused with.
fn err(spec: &str) -> String {
    match filter(spec) {
        Err(message) => message,
        Ok(parsed) => panic!("`{spec}` should be refused, parsed as {parsed:?}"),
    }
}

/// A source recommending whatever it is built with, so that
/// [`ParallelArgs::resolve`]'s two halves — ask the source, or honour the
/// flag — can be told apart without a real file of either shape. The three
/// required methods answer nothing; `resolve` touches no byte.
struct Recommends {
    jobs: usize,
    memory: Option<pgdump_query::WorkerMemory>,
}

impl Recommends {
    /// A source that recommends a worker count and no cost of its own —
    /// the plain file's shape, which asks for no budget.
    fn jobs(jobs: usize) -> Self {
        Self { jobs, memory: None }
    }

    /// A source that recommends both, which is what a compressed one does:
    /// a count, and what **one** of those workers holds.
    fn reader(jobs: usize, per_worker: u64) -> Self {
        Self { jobs, memory: Some(pgdump_query::WorkerMemory::per_worker(per_worker)) }
    }

    /// A block-decoding source's shape: a per-worker charge, and the
    /// retention list the pool shares over a `depth`-slot pool.
    fn block_reader(jobs: usize, per_worker: u64, unit: u64, depth: usize) -> Self {
        Self {
            jobs,
            memory: Some(pgdump_query::WorkerMemory::per_worker(per_worker).pooling(unit, depth)),
        }
    }
}

impl ByteRangeSource for Recommends {
    fn read_range(
        &self,
        _offset: u64,
        _len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
    > {
        Box::pin(async { Ok(bytes::Bytes::new()) })
    }
    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>>
    {
        Box::pin(async { Ok(0) })
    }
    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async { Ok(None) })
    }
    fn default_workers(&self) -> usize {
        self.jobs
    }
    fn default_worker_memory(&self) -> Option<pgdump_query::WorkerMemory> {
        self.memory
    }
}

/// One of the committed runtime roots
/// (`pgdt/tests/data/runtime/README.md`) — a filesystem tree shaped like a
/// Linux one, so a resolution can be pinned against an environment this
/// machine is not in.
fn runtime_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/runtime").join(name)
}

/// One reader's charge in the flat shape every source but the
/// block-decoding one states; that shape is `BLOCK_READER` beside its
/// unit.
const READER: u64 = 58 << 20;

/// **A discovered limit is what a flagless run resolves inside, on both
/// cgroup versions** — the v1 arm being the one no machine here can
/// produce (`RT4`, `RT6`), reached through `mountinfo`. The pair is
/// consistent in both: the count is what the allowance affords under the
/// margin and the budget is what that many readers spend, never the cap
/// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
/// permits").
#[test]
fn a_flagless_run_resolves_inside_a_discovered_limit_on_either_cgroup_version() {
    let flagless = ParallelArgs { jobs: None, memory: None };

    // v2, a 1 GiB `memory.max`: 640 MiB after the reserve, and 563.2 MiB
    // once the margin is left as well — nine readers of 58 MiB.
    let v2 = flagless.resolve_in(&runtime_root("v2-limit"), &Recommends::reader(24, READER));
    assert_eq!(v2.parallelism().jobs(), 9);
    assert_eq!(v2.parallelism().memory_bytes(), Some(9 * READER));
    assert_eq!(v2.limit.as_ref().map(|l| l.bytes), Some(1 << 30));

    // v1, a 512 MiB `memory.limit_in_bytes`: 128 MiB after the reserve,
    // and the margin is inert below the 640 MiB crossover — it allows
    // 153.6, which is more than the cap — so the cap is what two readers
    // of 58 MiB are fitted inside.
    let v1 = flagless.resolve_in(&runtime_root("v1-limit"), &Recommends::reader(24, READER));
    assert_eq!(v1.parallelism().jobs(), 2);
    assert_eq!(v1.parallelism().memory_bytes(), Some(2 * READER));
    assert_eq!(
        v1.limit.as_ref().map(|l| l.read_from.clone()),
        Some(runtime_root("v1-limit").join("sys/fs/cgroup/memory/svc/memory.limit_in_bytes")),
        "the v1 mount is located through mountinfo, not assumed"
    );

    // A plain source recommends nothing and is left where it was, capped
    // by the same limit.
    let plain = flagless.resolve_in(&runtime_root("v2-limit"), &Recommends::jobs(1));
    assert!(plain.parallelism().is_serial());
    assert_eq!(plain.parallelism().memory_bytes(), Some(pgdump_query::DEFAULT_MEMORY_BUDGET));
}

/// **A shared pool lowers the recommended count at every allocation.** A
/// block-decoding source's pool retains a unit for every slot but the one
/// a reader is filling — `POOL_DEPTH - 1` below four readers and
/// `jobs - 1` above — so the allowance buys fewer than a division by the
/// per-reader charge would say. Pinned at both ends: billing no pool would
/// bind nowhere, billing two units a reader would over-bill.
#[test]
fn a_shared_pool_lowers_a_recommended_count_at_every_allocation() {
    let flagless = ParallelArgs { jobs: None, memory: None };
    const UNIT: u64 = 24 << 20;
    // What one reader of a 24 MiB-block dump holds: the block it is
    // decoding and then retains, the chunk buffer a straddling read is
    // assembled into, and the decoder.
    const BLOCK_READER: u64 = 34 << 20;
    let source = || Recommends::block_reader(24, BLOCK_READER, UNIT, 4);

    // v1, a 512 MiB limit: 128 MiB after the reserve, which a division by
    // the per-reader charge calls three readers. The three units the pool
    // retains beside a single reader leave too little for a second, so one
    // is the honest answer.
    let tight = flagless.resolve_in(&runtime_root("v1-limit"), &source());
    assert_eq!(tight.parallelism().jobs(), 1);
    assert_eq!(tight.parallelism().memory_bytes(), Some(BLOCK_READER + 3 * UNIT));

    // v2, a 1 GiB limit, once the reserve and the margin are both left. A
    // division says sixteen readers; the pool slot each of them past the
    // first also takes is what makes it ten.
    let roomy = flagless.resolve_in(&runtime_root("v2-limit"), &source());
    assert_eq!(roomy.parallelism().jobs(), 10);
    assert_eq!(roomy.parallelism().memory_bytes(), Some(10 * BLOCK_READER + 9 * UNIT));
}

/// **No limit found leaves the source's recommendation standing**, capped
/// only by half of `MemAvailable` (`RT8`) — a `min` against the fallback
/// constant would make a flagless compressed scan serial on the machine
/// most likely to run it.
#[test]
fn no_limit_found_takes_the_sources_own_answer_under_the_memavailable_cap() {
    let flagless = ParallelArgs { jobs: None, memory: None };

    // Enough available that half of it does not bind, so all twenty-four
    // readers stand.
    let roomy = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::reader(24, READER));
    assert_eq!(roomy.parallelism().jobs(), 24);
    assert_eq!(roomy.parallelism().memory_bytes(), Some(24 * READER));
    assert!(roomy.limit.is_none(), "every limit file states max");

    // Less available on the same unlimited arrangement: half of it is
    // four readers — the count coming down with the budget rather than
    // being printed beside one it cannot spend.
    let cramped = flagless.resolve_in(&runtime_root("cramped"), &Recommends::reader(24, READER));
    assert_eq!(cramped.parallelism().jobs(), 4);
    assert_eq!(cramped.parallelism().memory_bytes(), Some(4 * READER));

    // And a source recommending nothing on an unlimited host is the one
    // arrangement that states no budget at all — what the status line
    // renders `(default: no limit found)`.
    let plain = flagless.resolve_in(&runtime_root("no-limit"), &Recommends::jobs(1));
    assert_eq!(plain.parallelism(), Parallelism::default());
    assert_eq!(plain.budget_display(), format!("{} (default: no limit found)", 64 << 20));
}

/// **An allocation at or under the reserve resolves to a budget of zero,
/// a resolved value rather than an error.** Zero produces one reader on
/// the streaming path, and the `PlanNote` beside it tells the user their
/// allocation bound the scan (`pgdump_query/tests/partitioned_replay.rs`,
/// `a_budget_below_one_readers_worth_says_the_allocation_bound_it`).
#[test]
fn an_allocation_under_the_reserve_resolves_to_one_reader_and_no_bytes() {
    let root = runtime_root("below-reserve");
    let flagless = ParallelArgs { jobs: None, memory: None };

    for source in [Recommends::reader(24, READER), Recommends::jobs(1)] {
        let resolved = flagless.resolve_in(&root, &source);
        assert_eq!(resolved.parallelism().memory_bytes(), Some(0));
        assert_eq!(resolved.parallelism().jobs(), 1, "the floor is on the count");
    }

    // A stated allowance still wins outright here: it *replaces* the
    // allocation discovery found rather than being capped by it. It pays
    // the same reserve and the same margin, whose ceiling binds below the
    // 400 MiB past the reserve, and the count beside it is nobody's statement and is cut to
    // what those bytes afford.
    let stated =
        ParallelArgs { jobs: None, memory: Some(pgdump_query::MEMORY_RESERVE + (400 << 20)) };
    let resolved = stated.resolve_in(&root, &Recommends::reader(24, READER));
    assert_eq!(resolved.parallelism().jobs(), 6);
    assert_eq!(resolved.parallelism().memory_bytes(), Some(6 * READER));
}

/// **A recommended count answers to the allowance however it arrived**,
/// the rule being scoped to the absence of `--jobs`
/// (`docs/design/roadmap.md`, "A default runs as fast as the allocation
/// permits").
///
/// **The bytes move too, and that is what `--memory` changed**: the
/// allowance is carved, so neither number the run reports is the one that
/// was typed (`docs/design/decisions.md`, "D83").
#[test]
fn a_stated_allowance_lowers_a_recommended_count_and_is_carved_not_kept() {
    let root = runtime_root("no-limit");
    let source = Recommends::reader(24, READER);
    // An allowance leaving 32 MiB after the reserve — and 32 MiB affords
    // no whole reader at all, which is the floor: one.
    let allowance = pgdump_query::MEMORY_RESERVE + (32 << 20);

    let tight = ParallelArgs { jobs: None, memory: Some(allowance) };
    let resolved = tight.resolve_in(&root, &source);
    assert_eq!(resolved.parallelism().jobs(), 1);
    assert_eq!(resolved.parallelism().memory_bytes(), Some(32 << 20));
    assert_eq!(
        resolved.jobs_display(),
        "1 (recommended by the source; lowered from 24 by the stated allowance)",
        "the clause names the flag to raise, not a cgroup nobody set"
    );
    assert_eq!(
        resolved.budget_display(),
        format!("{} (stated: --memory allows {allowance} resident byte(s))", 32u64 << 20),
        "the line carries both numbers, the typed one no longer being the budget"
    );

    // Room for more than the source asked for leaves the recommendation
    // standing, and the line says nothing about a lowering.
    let roomy = ParallelArgs { jobs: None, memory: Some(64 << 30) };
    let resolved = roomy.resolve_in(&root, &source);
    assert_eq!(resolved.parallelism().jobs(), 24);
    assert_eq!(resolved.jobs_display(), "24 (recommended by the source)");

    // A stated count is still printed as typed and never lowered, with
    // the same budget beside it — the asymmetry this arm preserves.
    let both = ParallelArgs { jobs: Some(24), memory: Some(allowance) };
    let resolved = both.resolve_in(&root, &source);
    assert_eq!(resolved.parallelism().jobs(), 24);
    assert_eq!(resolved.jobs_display(), "24 (stated)");
    assert_eq!(resolved.parallelism().memory_bytes(), Some(32 << 20));

    // A source recommending no per-worker cost has nothing to divide by,
    // and is left on the library's constant under the cap — exactly what a
    // discovered limit leaves it.
    let plain = tight.resolve_in(&root, &Recommends::jobs(1));
    assert_eq!(plain.parallelism().jobs(), 1);
    assert_eq!(plain.parallelism().memory_bytes(), Some(32 << 20));
}

/// **The statistics a cache hands the scan are carved around, not after**
/// (`docs/design/decisions.md`, "D85"): billed as held against the
/// margin's ceiling, they lower a recommended count first, and where the
/// count cannot fall — a source recommending no cost — the budget, so the
/// statistics allowance still covers what the cache holds. A discovered
/// limit and the same number stated carve them alike.
#[test]
fn a_cache_s_statistics_lower_the_count_and_then_the_budget() {
    let reader = Recommends::reader(24, READER);
    let limited = runtime_root("v2-limit");
    let flagless = ParallelArgs { jobs: None, memory: None };
    assert_eq!(flagless.discover_in(&limited).resolve(&reader, 0).parallelism().jobs(), 9);
    // Two readers' worth held: the 563.2 MiB ceiling less 116 MiB affords
    // seven, and the room seven leave absorbs the holding whole.
    let holding = flagless.discover_in(&limited).resolve(&reader, 2 * READER);
    assert_eq!(holding.parallelism().jobs(), 7);
    assert_eq!(holding.parallelism().memory_bytes(), Some(7 * READER));

    let allowance = 1u64 << 30;
    let unlimited = runtime_root("no-limit");
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    assert_eq!(
        stated.discover_in(&unlimited).resolve(&reader, 2 * READER).parallelism(),
        holding.parallelism(),
        "the same number stated is the same carving"
    );

    // A plain source's count cannot fall, so what the room beside its
    // budget cannot absorb comes off the budget, and the statistics
    // allowance is then exactly what the cache holds.
    let plain = Recommends::jobs(1);
    let budget = pgdump_query::DEFAULT_MEMORY_BUDGET;
    let room = pgdump_query::statistics_allowance(allowance, budget);
    let absorbed = stated.discover_in(&unlimited).resolve(&plain, room);
    assert_eq!(absorbed.parallelism().memory_bytes(), Some(budget));
    let short = 16 << 20;
    let squeezed = stated.discover_in(&unlimited).resolve(&plain, room + short);
    assert_eq!(squeezed.parallelism().memory_bytes(), Some(budget - short));
    assert_eq!(squeezed.statistics_allowance(), Some(room + short));
}

/// **`query`'s replay is carved with nothing held**, its mapping pass
/// around the cache's statistics: the same holding that lowers the
/// mapping pass's count leaves the replay the arrangement a cache without
/// statistics would, under a discovered limit and a stated one alike.
#[test]
fn a_query_s_replay_is_carved_around_no_statistics() {
    let reader = Recommends::reader(24, READER);
    let flagless = ParallelArgs { jobs: None, memory: None };
    let limited = runtime_root("v2-limit");
    let stated = ParallelArgs { jobs: None, memory: Some(1 << 30) };
    let unlimited = runtime_root("no-limit");
    for discovered in [flagless.discover_in(&limited), stated.discover_in(&unlimited)] {
        let passes = discovered.resolve_query(&reader, 2 * READER);
        let bare = discovered.resolve(&reader, 0).parallelism();
        assert_eq!(
            passes.mapping.parallelism(),
            discovered.resolve(&reader, 2 * READER).parallelism()
        );
        assert_eq!(passes.mapping.parallelism().jobs(), 7);
        assert_eq!(passes.replay.parallelism(), bare);
        assert_eq!(bare.jobs(), 9);
        // What the mapping span's `replay_jobs=` says: seven of the nine,
        // cut by the statistics; with none held the passes agree.
        assert_eq!(passes.mapping_shortfall(), Some((7, 9)));
        assert_eq!(discovered.resolve_query(&reader, 0).mapping_shortfall(), None);
    }
    // A stated count is never lowered, so only its budget can differ.
    let typed = ParallelArgs { jobs: Some(9), memory: None };
    let passes = typed.discover_in(&limited).resolve_query(&reader, 2 * READER);
    assert_eq!(passes.mapping_shortfall(), None);
}

/// **What a mapping pass's line reads as, in the format `pgdt` installs**:
/// `held_bytes` wherever the cache holds statistics, `replay_jobs` only
/// where they cut the count, and no span at all where it holds none. The
/// binary's own test (`a_query_s_mapping_pass_says_what_it_was_carved_around`)
/// reaches this through the real `await` on a plain source, whose count
/// cannot fall; the cut count is reached here, under a runtime root's
/// limit.
#[test]
fn the_mapping_span_carries_the_holding_and_a_cut_count() {
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let rendered = |passes: &QueryPasses| -> String {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_target(false)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            passes.mapping_span().in_scope(|| tracing::info!(jobs = 7, "scan started"));
        });
        String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
    };

    let reader = Recommends::reader(24, READER);
    let limited = runtime_root("v2-limit");
    let flagless = ParallelArgs { jobs: None, memory: None }.discover_in(&limited);
    let cut = rendered(&flagless.resolve_query(&reader, 2 * READER));
    let held = 2 * READER;
    assert!(
        cut.contains(&format!("mapping{{held_bytes={held} replay_jobs=9}}: scan started jobs=7")),
        "{cut}"
    );

    let typed = ParallelArgs { jobs: Some(9), memory: None }.discover_in(&limited);
    let uncut = rendered(&typed.resolve_query(&reader, held));
    assert!(uncut.contains(&format!("mapping{{held_bytes={held}}}: scan started")), "{uncut}");

    let none = rendered(&flagless.resolve_query(&reader, 0));
    assert!(!none.contains("mapping"), "{none}");
    assert!(none.contains("scan started"), "{none}");
}

/// **The check `introspect` owes: the instrument must not move the plan it
/// reports on.** It changes what the process *holds*, which is the point;
/// what it must not change is what the process *resolves*.
///
/// Compiled into both configurations against the same literals, so
/// `cargo test -p pgdt` and `cargo test -p pgdt --features introspect` are
/// the two halves of the comparison. The roots are the committed ones
/// rather than this machine's `/` (`tests/data/runtime/README.md`), and
/// every one of them is swept: the arms differ in which term binds — a
/// discovered ceiling, `RT8`'s half-`MemAvailable` cap, the reserve
/// leaving nothing.
#[test]
fn the_instrument_build_resolves_what_the_default_build_resolves() {
    let flagless = ParallelArgs { jobs: None, memory: None };
    let source = Recommends::reader(24, READER);

    let resolved: Vec<(usize, Option<u64>)> =
        ["v2-limit", "v1-limit", "no-limit", "cramped", "below-reserve"]
            .iter()
            .map(|name| {
                let r = flagless.resolve_in(&runtime_root(name), &source);
                (r.parallelism().jobs(), r.parallelism().memory_bytes())
            })
            .collect();

    assert_eq!(
        resolved,
        vec![
            (9, Some(9 * READER)),
            (2, Some(2 * READER)),
            (24, Some(24 * READER)),
            (4, Some(4 * READER)),
            (1, Some(0)),
        ],
        "the resolved arrangement moved: `--features introspect` and the default build must \
             read this identically"
    );
}

/// **`jobs=` reads differently by provenance, and the report says so**: a
/// recommended count is printed lowered to what the allowance affords, a
/// stated `--jobs` as typed — so the same two readers can appear under
/// either, and only the recommended line says what will run.
#[test]
fn the_report_says_which_of_the_two_counts_a_reader_is_looking_at() {
    let root = runtime_root("cramped");
    let source = Recommends::reader(24, READER);

    let flagless = ParallelArgs { jobs: None, memory: None };
    let recommended = flagless.resolve_in(&root, &source);
    assert_eq!(recommended.parallelism().jobs(), 4);
    assert_eq!(
        recommended.jobs_display(),
        "4 (recommended by the source; lowered from 24 by the allocation)"
    );

    let asked = ParallelArgs { jobs: Some(24), memory: None };
    let stated = asked.resolve_in(&root, &source);
    assert_eq!(stated.parallelism().jobs(), 24, "a stated count is not lowered");
    assert_eq!(stated.jobs_display(), "24 (stated)");
    // Both arrangements hold the same bytes, which is why the two lines
    // have to read differently.
    assert_eq!(stated.parallelism().memory_bytes(), recommended.parallelism().memory_bytes());
}

/// **The budget's provenance has four spellings and each names a different
/// way of arriving at a number** — the flag, a limit that was read, a
/// source's own answer under no limit, and the library's constant
/// (`docs/design/decisions.md`, "D64").
#[test]
fn the_budget_line_names_where_its_number_came_from() {
    let reader = Recommends::reader(24, READER);
    let plain = Recommends::jobs(1);

    let allowance = pgdump_query::MEMORY_RESERVE + (400 << 20);
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    assert_eq!(
        stated.resolve_in(&runtime_root("no-limit"), &reader).budget_display(),
        format!("{} (stated: --memory allows {allowance} resident byte(s))", 6 * READER)
    );

    let flagless = ParallelArgs { jobs: None, memory: None };
    let discovered = flagless.resolve_in(&runtime_root("v2-limit"), &reader);
    let line = discovered.budget_display();
    assert!(line.starts_with(&format!("{} (discovered:", 9 * READER)), "{line}");
    assert!(line.contains("sys/fs/cgroup/pgdt/memory.max"), "{line}");
    assert!(line.contains(&format!("{}", 1u64 << 30)), "{line}");

    assert_eq!(
        flagless.resolve_in(&runtime_root("no-limit"), &reader).budget_display(),
        format!("{} (no limit found: what this source asks for)", 24 * READER)
    );
    assert_eq!(
        flagless.resolve_in(&runtime_root("no-limit"), &plain).budget_display(),
        format!("{} (default: no limit found)", 64 << 20)
    );
}

/// **The statistics line reads `(none: …)` only where `MemAvailable`
/// cannot be read**: a host reporting none available still states a
/// number, half of nothing, and says where it came from.
#[test]
fn the_statistics_line_is_none_only_where_available_memory_is_unreadable() {
    let flagless = ParallelArgs { jobs: None, memory: None };
    let reader = Recommends::reader(24, READER);

    let unreadable = tempfile::tempdir().unwrap();
    assert_eq!(
        flagless.resolve_in(unreadable.path(), &reader).statistics_allowance_display(),
        "(none: no limit found and the machine's available memory could not be read)"
    );

    let exhausted = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(exhausted.path().join("proc")).unwrap();
    std::fs::write(exhausted.path().join("proc/meminfo"), "MemAvailable: 0 kB\n").unwrap();
    let line = flagless.resolve_in(exhausted.path(), &reader).statistics_allowance_display();
    assert!(line.ends_with("(half of what the machine reports available)"), "{line}");
}

/// **A plan note names the budget that bound the plan; the clause beside
/// it names where that budget came from** — under discovery the number to
/// change is the *allocation*, which the note cannot know about
/// (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// **A stated allowance earns one too**, the note naming the carved budget
/// rather than the number that person typed
/// (`docs/design/decisions.md`, "D83").
#[test]
fn a_plan_note_says_where_the_budget_that_bound_it_came_from() {
    let reader = Recommends::reader(24, READER);

    let flagless = ParallelArgs { jobs: None, memory: None };
    let clause = flagless.resolve_in(&runtime_root("v1-limit"), &reader).plan_note_origin();
    assert!(clause.starts_with(" — the budget in force is "), "{clause}");
    assert!(clause.contains("memory.limit_in_bytes"), "the file that stated it: {clause}");
    assert!(clause.contains(&format!("{}", 512u64 << 20)), "the limit itself: {clause}");

    // No limit found still earns a clause: the budget is the source's own
    // ask, equally not the user's.
    let unlimited = flagless.resolve_in(&runtime_root("no-limit"), &reader);
    assert_eq!(
        unlimited.plan_note_origin(),
        format!(
            " — the budget in force is {} (no limit found: what this source asks for)",
            24 * READER
        )
    );

    let allowance = pgdump_query::MEMORY_RESERVE + (400 << 20);
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    assert_eq!(
        stated.resolve_in(&runtime_root("v1-limit"), &reader).plan_note_origin(),
        format!(
            " — the budget in force is {} (stated: --memory allows {allowance} resident byte(s))",
            6 * READER
        )
    );
}

/// The budget a flagless resolution lands on **here**, on whatever machine
/// the suite is running on: `None` — nobody stated one — only where no
/// memory limit is discovered, and the discovered allowance capped at the
/// library's own constant where one is.
///
/// **The branch is the thing being pinned, not the number**: the answer
/// depends on the cgroup the test process is in, and a test naming the
/// constant would pass on a bare host and fail in a container. Every
/// precedence claim below is asserted against this rather than around it.
fn flagless_budget() -> Option<u64> {
    pgdump_query::discover_memory_limit().map(|limit| {
        pgdump_query::DEFAULT_MEMORY_BUDGET
            .min(limit.bytes.saturating_sub(pgdump_query::MEMORY_RESERVE))
    })
}

/// **Stating neither flag asks the source**, which is the one thing about
/// `--jobs` no integration test can see: a partitioned run and a serial
/// one produce the same bytes, so a default that silently reverted to a
/// constant would pass every other test in the tree
/// (`docs/design/measurements.md`, "The apparatus"). Which source
/// recommends which count is `ByteRangeSource::default_workers`'s test.
#[test]
fn stating_no_parallelism_flag_asks_the_source() {
    let budget = flagless_budget();
    let filled = budget.unwrap_or(pgdump_query::DEFAULT_MEMORY_BUDGET);
    let stated = ParallelArgs { jobs: None, memory: None };

    // A source recommending the serial path gets it, carrying whatever
    // the environment allows — and `None`, rendered `(default)`, where no
    // limit was found.
    assert!(stated.resolve(&Recommends::jobs(1)).parallelism().is_serial());
    assert_eq!(stated.resolve(&Recommends::jobs(1)).parallelism().memory_bytes(), budget);

    // `Parallelism::Workers` has nowhere to record that nobody stated a
    // budget, so it carries the fallback bare.
    assert_eq!(stated.resolve(&Recommends::jobs(8)).parallelism(), Parallelism::workers(8, filled));

    // A stated flag wins outright over a recommendation in either
    // direction.
    let asked = ParallelArgs { jobs: Some(8), memory: None };
    assert_eq!(asked.resolve(&Recommends::jobs(1)).parallelism(), Parallelism::workers(8, filled));
    let serial = ParallelArgs { jobs: Some(1), memory: None };
    assert!(serial.resolve(&Recommends::jobs(24)).parallelism().is_serial());
    assert_eq!(serial.resolve(&Recommends::jobs(24)).parallelism().memory_bytes(), budget);
}

/// **A carved budget survives a serial worker count**: the collapse that
/// makes `--jobs 1` serial takes the *worker count* down and not the bytes
/// beside it, so `--memory` alone is the whole recourse for a
/// compressed file whose blocks the default cannot hold
/// (`docs/design/decisions.md`, "I/O, memory and parallelism").
///
/// The source here recommends nothing, which is the plain-file shape, so
/// the budget it lands on is the library's constant under the allowance's
/// cap — the same number a discovered limit of that size leaves it
/// (`docs/design/decisions.md`, "D83").
#[test]
fn a_carved_budget_reaches_the_library_at_a_serial_job_count() {
    let allowance = pgdump_query::MEMORY_RESERVE + (400 << 20);
    let stated = ParallelArgs { jobs: None, memory: Some(allowance) };
    let serial = Recommends::jobs(1);
    assert!(
        stated.resolve(&serial).parallelism().is_serial(),
        "one worker is still the serial path"
    );
    assert_eq!(
        stated.resolve(&serial).parallelism().memory_bytes(),
        Some(pgdump_query::DEFAULT_MEMORY_BUDGET)
    );

    // Stated explicitly rather than taken from the source: the same
    // value either way.
    let one = ParallelArgs { jobs: Some(1), memory: Some(allowance) };
    assert_eq!(
        one.resolve(&Recommends::jobs(24)).parallelism(),
        stated.resolve(&serial).parallelism()
    );

    // A stated count with no budget beside it is the other half of the
    // pair, and the budget then comes from the environment.
    let jobs_only = ParallelArgs { jobs: Some(1), memory: None };
    assert_eq!(jobs_only.resolve(&serial).parallelism().memory_bytes(), flagless_budget());
}

/// The bare spelling: no whitespace anywhere means nothing to trim and no
/// quote to strip.
#[test]
fn a_bare_term_parses_as_it_reads() {
    assert_eq!(ok("name=alpha"), ("name".into(), PredicateOp::Eq, Some("alpha".into())));
    assert_eq!(ok("v>=5"), ("v".into(), PredicateOp::Ge, Some("5".into())));
    assert_eq!(ok("v!=5"), ("v".into(), PredicateOp::Ne, Some("5".into())));
}

/// Whitespace outside quotes is not data, on **both** sides of the
/// operator — untrimmed, the value side fails loudly on a typed column and
/// silently on a text one.
#[test]
fn whitespace_outside_quotes_is_trimmed_on_both_sides() {
    assert_eq!(ok(" name = alpha "), ("name".into(), PredicateOp::Eq, Some("alpha".into())));
    assert_eq!(ok("v >= 5"), ("v".into(), PredicateOp::Ge, Some("5".into())));
}

/// Whitespace is `str::trim`'s, not ASCII space's, so a non-breaking
/// space pasted out of a web page is caught rather than searched for.
#[test]
fn trimming_is_unicode_whitespace() {
    assert_eq!(
        ok("name\u{a0}=\u{a0}alpha\u{a0}"),
        ("name".into(), PredicateOp::Eq, Some("alpha".into()))
    );
}

/// An all-whitespace value collapses to the empty string, which needs no
/// special handling: it is a legitimate value to search for and the
/// quoted spelling is there for anyone who meant the spaces.
#[test]
fn an_all_whitespace_value_collapses_to_empty() {
    assert_eq!(ok("name=   "), ("name".into(), PredicateOp::Eq, Some(String::new())));
}

/// A quoted value is taken exactly as written, which is what keeps a
/// space-padded `char(n)` value askable from the command line.
#[test]
fn a_quoted_value_is_taken_as_written() {
    assert_eq!(ok(r#"name = " x""#), ("name".into(), PredicateOp::Eq, Some(" x".into())));
    assert_eq!(ok("name = 'x '"), ("name".into(), PredicateOp::Eq, Some("x ".into())));
    assert_eq!(ok("name=''"), ("name".into(), PredicateOp::Eq, Some(String::new())));
}

/// Both quote characters open a value: which one a user reaches for is
/// decided by the shell, the term normally being inside shell single
/// quotes already.
#[test]
fn both_quote_characters_open_a_value() {
    let (_, _, double) = ok(r#"name="the answer""#);
    let (_, _, single) = ok("name='the answer'");
    assert_eq!(double, Some("the answer".into()));
    assert_eq!(single, double);
}

/// A quote inside a quoted part is doubled, as SQL does it. Two quote
/// characters also give a lazier escape for free: a value holding one can
/// be written in the other, with no doubling at all.
#[test]
fn an_interior_quote_is_doubled_or_written_in_the_other_quote() {
    assert_eq!(ok("note='it''s'").2, Some("it's".into()));
    assert_eq!(ok(r#"note="say ""hi""""#).2, Some(r#"say "hi""#.into()));
    assert_eq!(ok(r#"note="it's""#).2, Some("it's".into()));
}

/// A quote that does not open the value is ordinary data, to this parser
/// and to `--where`'s tokenizer alike.
#[test]
fn a_quote_inside_an_unquoted_value_is_data() {
    assert_eq!(ok("note=don't").2, Some("don't".into()));
    assert_eq!(ok(r#"note=a"b"#).2, Some(r#"a"b"#.into()));
}

/// Quotes work on the column side too, and the operator split skips them,
/// which is what makes a column named `a=b` askable.
#[test]
fn a_quoted_column_name_survives_the_split() {
    assert_eq!(ok(r#""my column"=x"#).0, "my column");
    assert_eq!(ok(r#""a=b"=x"#), ("a=b".into(), PredicateOp::Eq, Some("x".into())));
    assert_eq!(ok(r#" "a=b" = "y=z" "#).2, Some("y=z".into()));
}

/// The split rule is otherwise unchanged: earliest position, longest
/// spelling, so a value carrying an operator byte still cannot steal it.
#[test]
fn the_earliest_operator_outside_quotes_still_wins() {
    assert_eq!(ok("name=alpha>x"), ("name".into(), PredicateOp::Eq, Some("alpha>x".into())));
}

/// The two worded infix operators, in every case and with any run of
/// whitespace between their words — the spelling `PredicateOp::symbol`
/// names them by, so the grammar and every refusal message agree.
#[test]
fn the_distinct_from_forms_parse() {
    assert_eq!(
        ok("v is distinct from 1"),
        ("v".into(), PredicateOp::IsDistinctFrom, Some("1".into()))
    );
    assert_eq!(
        ok("v IS NOT DISTINCT FROM 1"),
        ("v".into(), PredicateOp::IsNotDistinctFrom, Some("1".into()))
    );
    assert_eq!(
        ok("v Is  Not   Distinct\tFrom  ' x'"),
        ("v".into(), PredicateOp::IsNotDistinctFrom, Some(" x".into()))
    );
}

/// **A worded operator is a candidate at a position, not a pass of its
/// own**, so the earliest operator still wins in both directions: the
/// punctuation one when it is to the left, the phrase when it is.
#[test]
fn the_earliest_operator_wins_against_a_worded_one_too() {
    assert_eq!(
        ok("note=a is distinct from b"),
        ("note".into(), PredicateOp::Eq, Some("a is distinct from b".into()))
    );
    assert_eq!(
        ok("a is distinct from b=c"),
        ("a".into(), PredicateOp::IsDistinctFrom, Some("b=c".into()))
    );
}

/// The phrase needs whitespace on both sides: a column named
/// `is distinct from` is still askable unquoted, what follows the phrase
/// there being `=` rather than a space.
#[test]
fn a_worded_operator_needs_whitespace_around_it() {
    assert_eq!(
        ok("is distinct from=x"),
        ("is distinct from".into(), PredicateOp::Eq, Some("x".into()))
    );
    // Only the exact words, and only with a value after them: a prefix
    // match on `distinctly`, or a `FROM` with nothing behind it, falls
    // through to the usage message rather than splitting.
    for spec in ["v is distinctly from x", "v is distinct from"] {
        let message = err(spec);
        assert!(message.contains("--filter must be"), "`{spec}`: {message}");
    }
}

/// **The `IS` forms are the fallback**: an operator outside quotes is
/// looked for first, so this is the equality it plainly reads as rather
/// than an `IS NULL` on a column `note=this`.
#[test]
fn a_value_ending_in_is_null_is_not_an_is_null_term() {
    assert_eq!(
        ok("note=this is null"),
        ("note".into(), PredicateOp::Eq, Some("this is null".into()))
    );
}

/// The `IS` forms parse case-insensitively and take a quoted column name,
/// which is what lets a column called `is null` be named.
#[test]
fn the_is_forms_parse_on_a_term_with_no_operator() {
    assert_eq!(ok("created_at IS NULL"), ("created_at".into(), PredicateOp::IsNull, None));
    assert_eq!(ok("created_at is not null"), ("created_at".into(), PredicateOp::IsNotNull, None));
    assert_eq!(ok(r#" "my column" Is Null "#).0, "my column");
    assert_eq!(
        ok("v_date IS UNREPRESENTABLE"),
        ("v_date".into(), PredicateOp::IsUnrepresentable, None)
    );
    assert_eq!(
        ok("v_date is not unrepresentable"),
        ("v_date".into(), PredicateOp::IsNotUnrepresentable, None)
    );
    assert_eq!(ok(r#""is null" = x"#), ("is null".into(), PredicateOp::Eq, Some("x".into())));
}

/// A malformed quote is refused, never reinterpreted. Unterminated and
/// trailing-text are one fault with one message.
#[test]
fn a_malformed_quote_is_refused() {
    for spec in ["name='x", "name='x'y", "'name=x", r#"name = "x'"#, "'name' 'is null"] {
        let message = err(spec);
        assert!(message.contains("unbalanced"), "`{spec}`: {message}");
        assert!(message.contains(spec), "`{spec}`: {message}");
    }
}

/// A term with neither an operator nor an `IS` form is a usage fault, and
/// the message quotes the term back.
#[test]
fn a_term_with_no_operator_at_all_is_a_usage_fault() {
    let message = err("nonsense");
    assert!(message.contains("--filter must be"), "{message}");
    assert!(message.contains("nonsense"), "{message}");
}

/// **The structural refusal runs before the term grammar**, so a term
/// both structural and unparseable is told which of the two it is:
/// `and is null` is refused for the reserved spelling, and the remedy is
/// the quoting the grammar already teaches.
#[test]
fn the_flag_refuses_structure_before_it_parses_a_term() {
    let structural = parse_filter_flag("and is null").expect_err("a reserved spelling");
    let message = format!("{structural:#}");
    assert!(message.contains("`AND`"), "{message}");
    assert!(!message.contains("--filter must be"), "{message}");
    assert_eq!(
        column_of(parse_filter_flag(r#""and" is null"#).expect("the quoted column is askable")),
        "and"
    );
    // A term with no structure still reaches the term grammar, refusal
    // and all.
    assert!(
        format!("{:#}", parse_filter_flag("nonsense").expect_err("no operator"))
            .contains("--filter must be")
    );
    let Expr::Term(term) = parse_filter_flag("name=alpha").expect("a plain term") else {
        panic!("a comparison")
    };
    assert_eq!(term.value.as_deref(), Some("alpha"));
    // A membership is one term under either flag, its paren no structure.
    let Expr::In(membership) = parse_filter_flag("name in (alpha, 'and')").expect("a list") else {
        panic!("a membership")
    };
    assert_eq!(membership.values, [Some("alpha".to_string()), Some("and".to_string())]);
}

/// The column a comparison term names.
fn column_of(expr: Expr) -> String {
    match expr {
        Expr::Term(term) => term.column,
        other => panic!("a comparison, not {other:?}"),
    }
}

/// The note a name that was not found earns when it looks quoted —
/// `--column` and `--table` take their names verbatim, so the quote marks
/// were part of what was looked for.
#[test]
fn a_quoted_looking_name_earns_the_verbatim_note() {
    let note = quoted_name_note("--column", "\"id\"").expect("a quoted-looking name");
    assert!(note.contains("--column"), "{note}");
    assert!(note.contains("matched literally"), "{note}");
    assert_eq!(quoted_name_note("--column", "id"), None);
    assert_eq!(quoted_name_note("--table", "\"id"), None, "one quote is not a pair");
    assert_eq!(quoted_name_note("--table", "\""), None, "one character is not a pair");
}

fn list_of(child: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", child, true)))
}

fn range_struct(bound: DataType) -> DataType {
    DataType::Struct(Fields::from(vec![
        Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
        Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
        Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
    ]))
}

/// A scalar column's Arrow type is arrow's own `Display`, unmodified —
/// the reason the line is printed for every mapped non-`Utf8View` column
/// rather than only for nested ones.
#[test]
fn a_scalar_column_renders_as_arrows_own_display() {
    assert_eq!(
        arrow_type_label(&DataType::Decimal128(38, 10), &NestedPlan::Scalar),
        "Decimal128(38, 10)"
    );
}

#[test]
fn arrays_and_composites_render_through_arrows_display() {
    assert_eq!(
        arrow_type_label(&list_of(DataType::Utf8View), &NestedPlan::array(NestedPlan::Scalar)),
        "List(Utf8View)"
    );
    let point = DataType::Struct(Fields::from(vec![
        Field::new("x", DataType::Int32, true),
        Field::new("y", DataType::Utf8View, true),
    ]));
    assert_eq!(
        arrow_type_label(&point, &NestedPlan::record(vec![NestedPlan::Scalar; 2])),
        r#"Struct("x": Int32, "y": Utf8View)"#
    );
}

/// The one substitution: the five-field range struct is identical in
/// every dump, so only its bound type is worth printing.
#[test]
fn the_range_struct_collapses_to_its_bound_type() {
    assert_eq!(
        arrow_type_label(
            &range_struct(DataType::Int32),
            &NestedPlan::Range(Box::new(NestedPlan::Scalar))
        ),
        "Range<Int32>"
    );
}

/// A built-in multirange and an array of the matching range are the
/// *same* Arrow type and different plans, so rendering identically is
/// correct — the declared PostgreSQL type, on the table's `columns:` line
/// and beside it in `--json`, tells them apart.
#[test]
fn a_multirange_and_an_array_of_the_matching_range_render_identically() {
    let multirange = arrow_type_label(
        &list_of(range_struct(DataType::Int32)),
        &NestedPlan::Multirange(Box::new(NestedPlan::Scalar)),
    );
    let array_of_range = arrow_type_label(
        &list_of(range_struct(DataType::Int32)),
        &NestedPlan::array(NestedPlan::Range(Box::new(NestedPlan::Scalar))),
    );
    assert_eq!(multirange, "List(Range<Int32>)");
    assert_eq!(array_of_range, multirange);
}

/// Dispatch is the plan's, never the field names': a user composite may
/// declare five fields with the range struct's names and must still print
/// as the struct it is.
#[test]
fn a_composite_wearing_the_range_structs_field_names_is_not_collapsed() {
    let impostor = range_struct(DataType::Int32);
    let rendered = arrow_type_label(&impostor, &NestedPlan::record(vec![NestedPlan::Scalar; 5]));
    assert!(rendered.starts_with(r#"Struct("lower": Int32"#), "{rendered}");
    assert!(!rendered.contains("Range<"), "{rendered}");
}

/// Every `TypeKind` arm renders, with whatever payload it carries. Two
/// are shapes `pg_dump` does not write — a composite whose body held an
/// unparseable fragment, a range whose parameter list named no `subtype` —
/// so this is the only place they are pinned.
#[test]
fn every_type_kind_renders_with_its_own_payload() {
    use pgdump_query::ColumnDef;

    let labels = |ls: &[&str]| TypeKind::Enum {
        labels: ls.iter().map(|l| l.to_string()).collect(),
        exact: true,
    };
    assert_eq!(type_kind_summary(&labels(&["sad", "has'quote"])), "enum: 'sad', 'has''quote'");
    assert_eq!(type_kind_summary(&labels(&[])), "enum: (no labels)");
    let inexact = TypeKind::Enum { labels: vec!["sad".to_string()], exact: false };
    assert_eq!(type_kind_summary(&inexact), "enum: 'sad' (labels not read exactly)");
    let unread = TypeKind::Enum { labels: Vec::new(), exact: false };
    assert_eq!(type_kind_summary(&unread), "enum: (labels not read)");
    assert_eq!(type_kind_summary(&TypeKind::domain("integer")), "domain over integer");
    assert_eq!(
        type_kind_summary(&TypeKind::Domain {
            base_type: "text".to_string(),
            collation: Some(r#"pg_catalog."C""#.to_string()),
            not_null: false,
            check: false,
        }),
        r#"domain over text COLLATE pg_catalog."C""#
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Domain {
            base_type: "integer".to_string(),
            collation: None,
            not_null: true,
            check: true,
        }),
        "domain over integer NOT NULL CHECK"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Composite {
            fields: Some(vec![
                ColumnDef::new("x", "integer"),
                ColumnDef {
                    name: "c".to_string(),
                    declared_type: "text".to_string(),
                    collation: Some(r#"pg_catalog."C""#.to_string()),
                    not_null: None,
                },
            ]),
        }),
        r#"composite: x integer, c text COLLATE pg_catalog."C""#
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Composite { fields: Some(vec![]) }),
        "composite: (no fields)"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Composite { fields: None }),
        "composite: (fields not parsed)"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Range {
            subtype: Some("double precision".to_string()),
            multirange_type_name: Some("public.myrange_multi".to_string()),
            canonical: None,
        }),
        "range over double precision"
    );
    assert_eq!(
        type_kind_summary(&TypeKind::Range {
            subtype: None,
            multirange_type_name: None,
            canonical: None
        }),
        "range (subtype not parsed)"
    );
    // A declared `canonical` function is named: it is why every filter
    // operator refuses a column of this type.
    assert_eq!(
        type_kind_summary(&TypeKind::Range {
            subtype: Some("integer".to_string()),
            multirange_type_name: None,
            canonical: Some("public.canonrange_canonical".to_string()),
        }),
        "range over integer, canonical public.canonrange_canonical"
    );
    assert_eq!(type_kind_summary(&TypeKind::base()), "base type");
    assert_eq!(type_kind_summary(&TypeKind::Shell), "shell type");
}
