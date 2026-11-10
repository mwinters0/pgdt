//! What a strict parse checks a column's fields by: its grammar, whether a
//! field is refused — a scalar through the reader its order key is built by,
//! a field read off its bytes by [`FieldWalk`] — and what it leaves
//! unchecked.

use pgdt_model::{TypeDef, TypeKind};

use crate::compare::{
    CompareKind, ComparisonDivergence, ComparisonPlan, NestedCompare, UnanswerableReason,
};
use crate::declared::{
    Visits, array_element, builtin_name, companion_range, domain_terminal, find_type, split_typmod,
};
use crate::decode::Unread;
use crate::grammar::input::{TextGrammar, text_grammar};
use crate::grammar::walk::FieldWalk;
use crate::key::order::field_key;
use crate::value::text::char_typmod_refuses;

/// **Why a strict parse checks nothing of a value, or of a position within
/// one**: what PostgreSQL refuses there is decided by something besides the
/// field and its column's declaration, or this build reads no input of its
/// type ([`unchecked_positions`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unchecked {
    /// The column is declared nowhere in the dump, so read as no type.
    Undeclared,
    /// `xml`, which `xml_in` parses by the restoring server's libxml under
    /// its `xmloption`.
    Xml,
    /// `money`, which `cash_in` reads by the restoring server's
    /// `lc_monetary`, a setting `pg_dump` writes nowhere (`KD13`).
    Money,
    /// A user base type, read by an input function only the restoring server
    /// runs.
    BaseType,
    /// A range declaring its own `canonical` function, which only the
    /// restoring server runs (I46).
    RangeCanonical { function: String },
    /// A `reg*` type or `aclitem`, whose input function looks the name it
    /// reads up in the restoring server's catalog.
    Catalog,
    /// A type the dump does not declare, as an extension's is not, read by
    /// whatever the restoring server holds under its name.
    UndeclaredType,
    /// A built-in type this build has no reader of — `tsvector`, `tsquery`,
    /// `pg_lsn`, `jsonpath`, `tid`, `oidvector`, `pg_snapshot`,
    /// `txid_snapshot` — outside a strict parse's promise by scope
    /// (`docs/design/roadmap.md`, "A literal is guaranteed in `*_out`'s form
    /// and never read past `*_in`'s").
    NoReader,
    /// A type whose declaration this build did not read in full — a
    /// composite whose fields, or a range whose subtype, the preamble did not
    /// parse, a built-in read by a grammar here but for a typmod no server
    /// holds, or a domain whose chain never ends — so it reads the field as
    /// its text.
    Unparsed,
    /// An array whose element type is itself an array, which this build
    /// reads as its text (`KD3`).
    ArrayShape,
    /// An enum whose labels the preamble does not hold exactly, so a label
    /// it lacks is not refused (I70).
    LabelsInexact,
    /// A domain declaring a `CHECK`, an expression `domain_in` evaluates
    /// (I77).
    DomainCheck,
}

impl Unchecked {
    /// The reason in words, for a listing.
    pub fn describe(&self) -> String {
        match self {
            Self::Undeclared => "declared nowhere in the dump".to_string(),
            Self::Xml => "`xml_in` parses it by the restoring server's libxml and `xmloption`"
                .to_string(),
            Self::Money => "`cash_in` reads it by the restoring server's `lc_monetary`".to_string(),
            Self::BaseType => {
                "a base type, read by an input function only the restoring server runs".to_string()
            }
            Self::RangeCanonical { function } => {
                format!("a range whose canonical function, {function}, only the restoring server runs")
            }
            Self::Catalog => "read against the restoring server's catalog".to_string(),
            Self::UndeclaredType => {
                "a type the dump does not declare, read by the restoring server".to_string()
            }
            Self::NoReader => "a built-in type pgdt has no reader of".to_string(),
            Self::Unparsed => "a type whose declaration pgdt did not read in full".to_string(),
            Self::ArrayShape => "an array of arrays, which pgdt reads as its text".to_string(),
            Self::LabelsInexact => {
                "an enum whose labels pgdt does not hold exactly, so a label it lacks is not refused"
                    .to_string()
            }
            Self::DomainCheck => "the domain's CHECK, an expression".to_string(),
        }
    }
}

/// The grammar a strict parse reads a field of a column compared by `plan`
/// and declared `declared` by in place of `plan`: its [`text_grammar`] where
/// `plan` orders it by nothing, and `None` elsewhere.
pub fn column_grammar(
    plan: &ComparisonPlan,
    declared: &str,
    types: &[TypeDef],
) -> Option<TextGrammar> {
    match plan {
        ComparisonPlan::Refused => text_grammar(declared, types),
        _ => None,
    }
}

/// **Every position of a column declared `declared` and compared by `plan`
/// that a strict parse checks nothing of**, as `(path, declared type there,
/// why)`: first each a field is read at by no grammar — the column itself
/// where `plan` orders by nothing and `grammar` is `None`, or is
/// unanswerable, a position
/// of a nested `plan` holding a type it orders by nothing and has no grammar
/// for, an enum whose labels are not exact — in walk order, then each domain
/// at or beneath the column declaring a `CHECK`. A path is spelled as [`NestedCompare::uncomparable`]
/// spells one, `""` naming the column.
///
/// `grammar` is [`column_grammar`]'s, which a strict parse reads by, so what
/// this names and what the parse checks are one reading of one plan.
pub fn unchecked_positions(
    declared: &str,
    plan: &ComparisonPlan,
    grammar: Option<TextGrammar>,
    types: &[TypeDef],
) -> Vec<(String, String, Unchecked)> {
    let mut out = Vec::new();
    match plan {
        ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
            range_type,
            function,
        }) => {
            out.push((
                String::new(),
                range_type.clone(),
                Unchecked::RangeCanonical { function: function.clone() },
            ));
        }
        ComparisonPlan::Refused if grammar.is_none() => {
            out.push((String::new(), declared.trim().to_string(), unread(declared, types)));
        }
        ComparisonPlan::Compared { kind: CompareKind::Enum { exact: false, .. }, .. } => {
            out.push((String::new(), declared.trim().to_string(), Unchecked::LabelsInexact));
        }
        ComparisonPlan::Nested(nested) => nested.walk(&mut String::new(), &mut |path, leaf| {
            let why = match leaf {
                NestedCompare::Leaf { kind: CompareKind::Enum { exact: false, .. }, .. } => {
                    Unchecked::LabelsInexact
                }
                // A position holding an array is an array's element, every
                // other array being a node of its own.
                NestedCompare::Uncomparable {
                    divergence: None, grammar: None, declared, ..
                } if array_element(domain_terminal(declared, types)).is_some() => {
                    Unchecked::ArrayShape
                }
                NestedCompare::Uncomparable {
                    divergence: None, grammar: None, declared, ..
                } => unread(declared, types),
                _ => return,
            };
            out.push((path.to_string(), leaf.declared().to_string(), why));
        }),
        _ => {}
    }
    domains_beneath(declared, types, &mut String::new(), Visits::over(types), &mut out);
    out
}

/// Why a field of `declared`, read by no grammar, is checked for nothing: its
/// terminal type's input is decided by the restoring server, or this build
/// reads none of it. An array is its element's reason, but an array of arrays.
fn unread(declared: &str, types: &[TypeDef]) -> Unchecked {
    let terminal = domain_terminal(declared, types);
    if let Some(element) = array_element(terminal) {
        if array_element(domain_terminal(element, types)).is_some() {
            return Unchecked::ArrayShape;
        }
        return unread(element, types);
    }
    let (base, _) = split_typmod(terminal);
    if base.contains('.') {
        return match find_type(base, types).map(|t| &t.kind) {
            // A shell is completed only as a base type.
            Some(TypeKind::Base { .. } | TypeKind::Shell) => Unchecked::BaseType,
            Some(TypeKind::Range { canonical: Some(function), .. }) => {
                Unchecked::RangeCanonical { function: function.clone() }
            }
            Some(TypeKind::Enum { exact: false, .. }) => Unchecked::LabelsInexact,
            None if companion_range(base, types).is_none() => Unchecked::UndeclaredType,
            // A composite or range read short, its multirange companion, and
            // a domain whose chain never ends.
            _ => Unchecked::Unparsed,
        };
    }
    if text_grammar(base, types).is_some() {
        return Unchecked::Unparsed;
    }
    match &*builtin_name(terminal).0 {
        "xml" => Unchecked::Xml,
        "money" => Unchecked::Money,
        "aclitem" | "regclass" | "regcollation" | "regconfig" | "regdictionary"
        | "regnamespace" | "regoper" | "regoperator" | "regproc" | "regprocedure" | "regrole"
        | "regtype" => Unchecked::Catalog,
        "tsvector" | "tsquery" | "pg_lsn" | "jsonpath" | "tid" | "oidvector" | "pg_snapshot"
        | "txid_snapshot" => Unchecked::NoReader,
        // A name `pg_catalog` holds no type under, as a hand-written dump may
        // write a type its search path finds.
        _ => Unchecked::UndeclaredType,
    }
}

/// Each domain at or beneath `declared` declaring a `CHECK`, appended to
/// `out` with `path`, walking domains, array elements, composite fields and
/// range bounds as [`resolve_declared_type`] walks them and spending `visits`
/// as it does.
///
/// [`resolve_declared_type`]: crate::pgtype::resolve_declared_type
fn domains_beneath(
    declared: &str,
    types: &[TypeDef],
    path: &mut String,
    visits: Visits,
    out: &mut Vec<(String, String, Unchecked)>,
) {
    let declared = declared.trim();
    let descend = |into: &str, step: &str, path: &mut String, visits, out: &mut Vec<_>| {
        let len = path.len();
        path.push_str(step);
        domains_beneath(into, types, path, visits, out);
        path.truncate(len);
    };
    if let Some(element) = array_element(declared) {
        return descend(element, "[]", path, visits, out);
    }
    let (base, _) = split_typmod(declared);
    if !base.contains('.') {
        return;
    }
    let Some(visits) = visits.spend() else { return };
    let Some(def) = find_type(base, types) else {
        if let Some(TypeDef { kind: TypeKind::Range { subtype: Some(subtype), .. }, .. }) =
            companion_range(base, types)
        {
            descend(subtype, "[].bound", path, visits, out);
        }
        return;
    };
    match &def.kind {
        TypeKind::Domain { base_type, check, .. } => {
            if *check {
                out.push((path.clone(), def.name.clone(), Unchecked::DomainCheck));
            }
            descend(base_type, "", path, visits, out);
        }
        TypeKind::Composite { fields: Some(fields) } => {
            for field in fields {
                descend(&field.declared_type, &format!(".{}", field.name), path, visits, out);
            }
        }
        TypeKind::Range { subtype: Some(subtype), .. } => {
            descend(subtype, ".bound", path, visits, out);
        }
        _ => {}
    }
}

/// **Whether a marked check finds PostgreSQL's input function refuses `text`
/// as a field of a column compared by `plan`**: what a strict parse checks
/// every field by ([`PostgresInvalidValues::Strict`]). A scalar is [`field_key`]'s refusal;
/// a nested value is read as the server reads one on its way in, through each
/// container's input grammar, every element as a field of its own type and a
/// range's bounds in order. `json`, held as its text, is read by `json_in`'s
/// grammar ([`json_in`](crate::value::json::json_in)), and a position holding
/// a type ordered by nothing by its [`TextGrammar`] where it has one; any
/// other plan naming no type this build reads by — another held as its text,
/// or a range declaring its own canonical function — refuses nothing, no
/// reader here decoding its fields. A column whose own type is ordered by
/// nothing is [`ComparisonPlan::Refused`] and is read by
/// [`grammar_refuses`](super::walk::grammar_refuses) instead.
///
/// A field whose plan [`walks`] is read off its bytes, as [`FieldWalk`]
/// reads one.
///
/// [`PostgresInvalidValues::Strict`]: pgdt_model::PostgresInvalidValues::Strict
pub fn field_refused(plan: &ComparisonPlan, text: &str) -> bool {
    // deficiency: KD164 — no check here refuses an array for its size, as
    // `path_in`'s and `poly_in`'s are refused (`value/geometric.rs`): a line
    // under I71's bound can hold an `array_in` value whose binary form passes
    // `MaxAllocSize` (I71's scope limit), which is read as a value, a strict
    // parse included, where a restore refuses it.
    if let Some(length) = text_typmod(plan) {
        return length.is_some_and(|length| char_typmod_refuses(text, length));
    }
    if walks(plan) {
        let mut walk = FieldWalk::new(plan);
        walk.text(text.as_bytes());
        return walk.finish();
    }
    match plan {
        ComparisonPlan::Compared { kind, .. } => {
            matches!(field_key(kind, text), Err(Unread::Refused))
        }
        ComparisonPlan::Nested(_) | ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_) => {
            false
        }
    }
}

/// **Whether [`field_refused`] reads a field of a column compared by `plan`
/// a byte at a time** ([`FieldWalk`]), holding no copy of it and no tree but
/// a range bound's (`docs/design/decisions.md`, "D142", "D143"): a nested
/// value, `json` and `jsonb`, a `bytea`, and a text held to its typmod.
pub fn walks(plan: &ComparisonPlan) -> bool {
    matches!(
        plan,
        ComparisonPlan::Nested(_)
            | ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. }
            | ComparisonPlan::Compared {
                kind: CompareKind::Jsonb
                    | CompareKind::Bytea
                    | CompareKind::Text { .. }
                    | CompareKind::PaddedText { .. },
                ..
            }
    )
}

/// **Where [`field_refused`] refuses a field of a column compared by `plan`
/// only past a text's typmod**, the typmod's length, `Some(None)` for a text
/// it refuses nothing of: a character type, but `json`, which is held as its
/// text. Every text within its length is a value of these, so a strict parse
/// reads one off its field's bytes, counting its characters with no copy of
/// it made ([`TextRead::typmod_refuses`]; `docs/design/decisions.md`,
/// "D142").
///
/// [`TextRead::typmod_refuses`]: crate::value::text::TextRead::typmod_refuses
pub fn text_typmod(plan: &ComparisonPlan) -> Option<Option<u32>> {
    match plan {
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => None,
        ComparisonPlan::Compared {
            kind: CompareKind::Text { length, .. } | CompareKind::PaddedText { length },
            ..
        } => Some(*length),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
