use super::*;
use std::collections::{BTreeMap, BTreeSet};

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
        .filter(|arg| matches!(arg.get_long(), Some("source" | "strict-identity")))
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
