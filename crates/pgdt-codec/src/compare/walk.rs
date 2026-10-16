//! The register's walk: a declared type's [`ComparisonPlan`] reached through
//! the catalog and the built-in table, an array's element, a range's subtype,
//! a domain's terminal type and a composite's fields each walked to a plan of
//! its own. `crate::compare` declares [`comparison_for`] and reads nothing
//! here.

use pgdt_model::{CollationDef, TypeDef, TypeKind};

use crate::compare::{
    CompareKind, ComparisonDivergence, ComparisonPlan, Discrete, NestedCompare, UnanswerableReason,
};
use crate::declared::{
    Position, Visits, array_element, builtin_name, companion_range, domain_terminal, find_type,
    is_box, split_typmod, typdelim,
};
use crate::grammar::input::text_grammar;
use crate::pgtype::{builtin_range_subtype, builtin_scalar};

/// The comparison for an array column, from the same walk [`resolve_array`]
/// makes and with the same two refusals: an element type that is opaque by
/// construction (I22), and one that is itself an array (I26). Both resolve the
/// *column* to `Utf8View` with its plan refused (`crate::resolve`), so a
/// column of either never reaches this plan — but the register is asked directly too, and must not
/// answer "ordered" for a column the resolver declines.
///
/// The column's own `COLLATE` clause is passed **down to the element**: an
/// array type is not collatable, and `pg_dump` writes the clause on a `text[]`
/// column to state the collation its *elements* are compared under (I37).
///
/// [`resolve_array`]: crate::pgtype
fn array_comparison(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let Some(element) = array_element(declared) else {
        return ComparisonPlan::Refused;
    };
    let terminal = domain_terminal(element, types);
    let opaque = is_box(terminal)
        || matches!(
            find_type(terminal, types).map(|t| &t.kind),
            Some(TypeKind::Base { .. } | TypeKind::Shell)
        );
    let child = if opaque || array_element(terminal).is_some() {
        // No divergence: a position only this build declines makes no claim
        // about the server's `=` (I22, I26). A column of either shape resolves
        // to text with its plan refused, so never reaches this node; a
        // composite's field of either shape does, and its array is split at
        // its element type's `typdelim`. A `box` element carries `box_in`'s
        // grammar.
        NestedCompare::Uncomparable {
            declared: element.to_string(),
            divergence: None,
            grammar: text_grammar(element, types).filter(|_| is_box(terminal)),
            delimiter: typdelim(element, types),
        }
    } else {
        match nested_position(element, collation, types, collations, visits) {
            Ok(child) => child,
            Err(reason) => return ComparisonPlan::Unanswerable(reason),
        }
    };
    ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::of(child, element, types))))
}

/// The comparison for a range or multirange column, from its bound type and
/// whether the range type canonicalizes.
///
/// **The bound is asked with no `COLLATE` clause**, which is the one place
/// this walk knowingly answers weaker than the file allows: a range type
/// carries its own `collation` parameter and the preamble grammar keeps only
/// `subtype`, `multirange_type_name` and `canonical` (I10), so a `text`-bounded
/// range reaches [`collated_text`](crate::compare::collated_text)'s no-clause
/// arm. That is the conservative direction — reading the parameter would move
/// the verdict and never the answer, so it is a property here rather than a
/// deficiency.
///
/// A range whose DDL stated no subtype at all is [`ComparisonPlan::Refused`]
/// outright rather than a tree with an unnameable position in it: there is no
/// declared type to put in the refusal.
fn range_comparison(
    subtype: Option<&str>,
    discrete: Option<Discrete>,
    multi: bool,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let Some(subtype) = subtype else { return ComparisonPlan::Refused };
    let bound = match nested_position(subtype, None, types, collations, visits) {
        Ok(bound) => Box::new(bound),
        Err(reason) => return ComparisonPlan::Unanswerable(reason),
    };
    ComparisonPlan::Nested(if multi {
        NestedCompare::Multirange { bound, discrete }
    } else {
        NestedCompare::Range { bound, discrete }
    })
}

/// The register's answer for a column whose type is, or contains, the range
/// type `range_type`, whose DDL declares the canonical function `function`.
///
/// **Knowing the function exists licenses declining the column, never
/// reproducing it** (I46): PostgreSQL rewrites every value through arbitrary
/// server-side code before storing or comparing it, so both operator families
/// are wrong rather than weak — `[1,10] = [1,11)` is true on such a server
/// and false under a bytewise comparison of the two `range_out` strings. That
/// is why the answer is [`ComparisonPlan::Unanswerable`] and not
/// [`ComparisonPlan::Refused`].
fn unanswerable_range(range_type: &str, function: &str) -> ComparisonPlan {
    ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
        range_type: range_type.to_string(),
        function: function.to_string(),
    })
}

/// One position *inside* a nested type — an array's element, a composite's
/// field — asked the same question the column was, so nesting composes and a
/// domain beneath a container bottoms out where a domain always does.
///
/// The two answers that are not a comparison collapse to
/// [`NestedCompare::Uncomparable`], carrying the divergence rather than
/// dropping it: `json` has no comparison at all inside a container, so the
/// `=` the column still falls back to is an answer PostgreSQL does not have,
/// where [`ComparisonPlan::Refused`] says only that *this build* has no order
/// for the position and makes no claim about the server's equality (`KD10`
/// one level down goes unannounced).
///
/// **[`ComparisonPlan::Unanswerable`] is the one answer a position cannot
/// hold**, and it is why this returns a `Result`: "no equality either" is a
/// fact about the whole column, not about the walk, so such a position
/// short-circuits out of the tree and becomes the column's own answer.
fn nested_position(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> Result<NestedCompare, UnanswerableReason> {
    Ok(match comparison_walk(declared, collation, types, collations, visits) {
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => {
            NestedCompare::Uncomparable {
                declared: declared.to_string(),
                divergence: Some(ComparisonDivergence::AsText),
                grammar: None,
                delimiter: typdelim(declared, types),
            }
        }
        ComparisonPlan::Refused => NestedCompare::Uncomparable {
            declared: declared.to_string(),
            divergence: None,
            grammar: text_grammar(declared, types),
            delimiter: typdelim(declared, types),
        },
        ComparisonPlan::Compared { kind, divergence } => {
            NestedCompare::Leaf { declared: declared.to_string(), kind, divergence }
        }
        ComparisonPlan::Nested(inner) => inner,
        ComparisonPlan::Unanswerable(reason) => return Err(reason),
    })
}

/// **The comparison register**: how a column declared `declared` compares, and
/// whether that is PostgreSQL's own order
/// (`docs/design/decisions.md`, "D40").
///
/// It walks the declared type exactly as [`resolve_declared_type`] does —
/// array first, then a schema-qualified name through the database's own
/// `CREATE TYPE`/`DOMAIN` list, then the built-in table — so the two answers are reached through one
/// spelling of the same string and a domain compares as whatever it bottoms
/// out at.
///
/// **A nested type answers [`ComparisonPlan::Nested`]**, one node per nesting
/// level, built by the same walk: an array's element, a composite's fields
/// and a range's bound are asked this same question in turn.
///
/// **`collation` is the column's own `COLLATE` clause**, verbatim as the DDL
/// wrote it ([`pgdt_model::ColumnDef::collation`]), or `None` where the
/// column carries none — which `pg_dump` writes exactly when the column's
/// collation is its type's default (I37). It is why the register is keyed per
/// *column*: two `text` columns of one table can compare differently.
///
/// [`resolve_declared_type`]: crate::pgtype::resolve_declared_type
pub fn comparison_for(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
) -> ComparisonPlan {
    comparison_walk(declared, collation, types, collations, Visits::over(types))
}

/// [`comparison_for`], at one step of its walk.
fn comparison_walk(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let declared = declared.trim();
    if array_element(declared).is_some() {
        return array_comparison(declared, collation, types, collations, visits);
    }
    let (base, _) = split_typmod(declared);
    if base.contains('.') {
        return comparison_user_type(base, collation, types, collations, visits);
    }
    let (base, typmod, qualifier) = builtin_name(declared);
    if let Some((_, plan)) = builtin_scalar(&base, typmod, qualifier, collation, collations) {
        return plan;
    }
    // The twelve built-in range and multirange names, which reach no arm of
    // `builtin_scalar` and appear in no `CREATE TYPE` (I10) — the same fourth
    // step `map_builtin` takes, so the two walks agree.
    match builtin_range_subtype(&base) {
        Some(range) => range_comparison(
            Some(range.subtype),
            range.discrete,
            range.multi,
            types,
            collations,
            visits,
        ),
        None => ComparisonPlan::Refused,
    }
}

/// The user-defined half of [`comparison_for`], over the same `TypeKind` list
/// [`resolve_user_type`] reads.
///
/// **Exhaustive over `TypeKind` with no wildcard arm**, so a kind added to the
/// preamble grammar has to choose a comparison rather than inheriting one.
/// Every visit is spent from `visits`, as [`resolve_user_type`]'s is.
///
/// [`resolve_user_type`]: crate::pgtype
fn comparison_user_type(
    name: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let Some(visits) = visits.spend() else { return ComparisonPlan::Refused };
    // Absent from the list: either an unknown type or a range's multirange
    // companion, which `pg_dump` emits no `CREATE TYPE` for (I10). The
    // companion is found through the range whose DDL names it, exactly as
    // `resolve_user_type` finds it.
    let Some(def) = find_type(name, types) else {
        return match companion_range(name, types) {
            // A multirange over a range that canonicalizes is unanswerable
            // for the same reason the range is, and names the *range* type:
            // the companion has no DDL of its own to name (I10).
            Some(TypeDef { name, kind: TypeKind::Range { subtype, canonical, .. } }) => {
                match canonical {
                    Some(function) => unanswerable_range(name, function),
                    None => {
                        range_comparison(subtype.as_deref(), None, true, types, collations, visits)
                    }
                }
            }
            _ => ComparisonPlan::Refused,
        };
    };
    match &def.kind {
        // An enum with no labels resolves to no Arrow type at all, so no
        // column of it is ever asked how it compares.
        TypeKind::Enum { labels, .. } if labels.is_empty() => ComparisonPlan::Refused,
        // PostgreSQL orders an enum by `pg_enum.enumsortorder`, assigned from
        // *declaration* order (I33) — so a label's position in this list is
        // the order and its text is not.
        TypeKind::Enum { labels, exact } => ComparisonPlan::agrees(CompareKind::Enum {
            name: def.name.as_str().into(),
            labels: labels.iter().cloned().collect(),
            exact: *exact,
        }),
        // A domain compares as what it bottoms out at, through any chain —
        // the same recursion `resolve_declared_type` makes, bounded the same
        // way. The collation walks down with it and the *column's* clause
        // wins: a domain's own `COLLATE` is its type default, which a
        // column-level clause only overrides (I37).
        TypeKind::Domain { base_type, collation: domain_collation, .. } => comparison_walk(
            base_type,
            collation.or(domain_collation.as_deref()),
            types,
            collations,
            visits,
        ),
        // Field-wise in declaration order, which is `record_cmp`'s rule and
        // the order `record_out` writes them in (I23). A field list the
        // grammar could not read is all-or-nothing: a composite parsed short
        // would compare field 3's text as field 2's type.
        TypeKind::Composite { fields } => match fields {
            Some(fields) => {
                let positions: Result<Vec<_>, _> = fields
                    .iter()
                    .map(|f| {
                        // A composite is not collatable, so the *column's*
                        // clause cannot reach a field; each attribute
                        // carries its own (I37).
                        nested_position(
                            &f.declared_type,
                            f.collation.as_deref(),
                            types,
                            collations,
                            visits,
                        )
                        .map(|plan| (f.name.clone(), Position::of(plan, &f.declared_type, types)))
                    })
                    .collect();
                match positions {
                    Ok(positions) => ComparisonPlan::Nested(NestedCompare::Record(positions)),
                    Err(reason) => ComparisonPlan::Unanswerable(reason),
                }
            }
            None => ComparisonPlan::Refused,
        },
        // Bound-wise, with no canonicalization — the whole answer only
        // because the range declares no `canonical` function. Where it
        // declares one the column is unanswerable instead (I46);
        // `fixtures/*`'s `public.myrange` and `public.textrange` declare
        // none.
        TypeKind::Range { subtype, canonical, .. } => match canonical {
            Some(function) => unanswerable_range(&def.name, function),
            None => range_comparison(subtype.as_deref(), None, false, types, collations, visits),
        },
        TypeKind::Base { .. } | TypeKind::Shell => ComparisonPlan::Refused,
    }
}
