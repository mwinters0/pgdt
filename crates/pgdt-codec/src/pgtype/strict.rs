//! What a strict parse reads a column's fields by where its comparison plan
//! orders them by nothing, and what it checks nothing of: not the register's
//! API, which says what a declared type resolves to and how it compares, but
//! what a crate above calls, so `internal` re-exports it.

use pgdt_model::{TypeDef, TypeKind};

use super::{
    CompareKind, ComparisonPlan, NestedCompare, TextGrammar, UnanswerableReason, Unchecked, Visits,
    array_element, builtin_name, companion_range, domain_terminal, find_type, split_typmod,
    text_grammar,
};

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
