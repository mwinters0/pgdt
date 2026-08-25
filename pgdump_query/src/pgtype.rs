//! Declared PostgreSQL type string -> Arrow `DataType`
//! (`docs/design/architecture.md`, "Type resolution").
//!
//! Pure, synchronous, no I/O — see `docs/design/layering.md`, L2. A declared
//! type is resolved against a single database's [`TypeDef`] list (already
//! selected by the caller — [`crate::resolve`] is the one that picks which
//! database), never against files or offsets.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Fields};

use crate::preamble::{TypeDef, TypeKind};

/// The outcome of mapping one declared type string, mirroring
/// [`crate::resolve::ColumnResolution`] but at single-type granularity.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeOutcome {
    /// The dump alone determines the value; this is the Arrow type it maps
    /// to (`Utf8View` included — e.g. `text`, `interval`, are deliberately
    /// mapped there, not merely defaulted), paired with the [`NestedPlan`]
    /// that says which literal form fills it.
    ///
    /// **The pair has one producer.** Nothing outside this module builds
    /// either half of a nested column's pairing, which is what keeps the two
    /// trees in agreement without a third structure enforcing it.
    Mapped(DataType, NestedPlan),
    /// A declared type string this build has no mapping for at all — neither
    /// a built-in, nor found in the database's `CREATE TYPE`/`DOMAIN` list,
    /// nor a composite whose declared field list survived parsing
    /// ([`TypeKind::Composite`] is all-or-nothing).
    Unknown,
    /// An array whose element type is opaque by construction — `box`, a
    /// C-level base type, or a shell type, through any chain of domains.
    ///
    /// Refused rather than mapped to `List<Utf8View>` because the array
    /// separator is the *element type's* `typdelim` (I22) and `box`'s is
    /// `;`: splitting such a literal on `,` silently invents element
    /// boundaries, and the elements it would recover are opaque text
    /// anyway. Held apart from [`Self::OpaqueBaseType`] so `pgdq info` can
    /// say which of the two happened.
    OpaqueElementType,
    /// A C-level base type or a shell/undefined type — genuinely
    /// information-free, not merely unimplemented (see the phase doc's
    /// "`CREATE TYPE`: six emitted forms").
    OpaqueBaseType,
    /// A `CREATE TYPE ... AS ENUM ()` with no labels at all.
    EmptyEnum,
}

/// Which PostgreSQL literal form fills a resolved Arrow type, at every
/// position in it.
///
/// **The Arrow type alone cannot say.** `int4range[]` and `int4multirange`
/// both resolve to `List<Struct{lower, upper, …}>`, and they are written
/// differently — `{"[1,10)","[2,3)"}` with array quoting versus `{[1,10),
/// [2,3)}` with none at all. A composite that happens to have the range
/// struct's five fields is the same collision one level down. So "which of
/// [`crate::nested`]'s codecs applies here" travels beside the `DataType`
/// rather than being inferred from it, and it is a tree because the answer
/// differs per nesting level.
///
/// A `Scalar` leaf is anything [`crate::decode`] handles (`Utf8View`
/// included), which is where every branch bottoms out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NestedPlan {
    /// Filled by `crate::decode`'s per-type decoders, or held as text.
    #[default]
    Scalar,
    /// `array_out` → `List<child>`. Nested `Array`s are the multi-dimensional
    /// case: the plan's depth is the dimensionality the column was resolved
    /// at, and a value that disagrees is a decode failure.
    Array(Box<NestedPlan>),
    /// `record_out` → `Struct<…>`, one plan per declared field, in
    /// declaration order.
    Record(Vec<NestedPlan>),
    /// `range_out` → the five-field range struct. The plan is the *bound*
    /// type's, shared by `lower` and `upper`; the three flags are always
    /// `Boolean`.
    Range(Box<NestedPlan>),
    /// `multirange_out` → `List<` the range struct `>`. The plan is again the
    /// bound type's.
    Multirange(Box<NestedPlan>),
}

/// The field names of the range struct, in order. Reserved: a composite type
/// resolves to a `Struct` too, and only the [`NestedPlan`] tells them apart —
/// these names are for a human reading `pgdq info`, never for dispatch.
pub const RANGE_STRUCT_FIELDS: [&str; 5] =
    ["lower", "upper", "lower_inclusive", "upper_inclusive", "empty"];

/// Split `declared` into its base type name and typmod contents, if any
/// (`numeric(38,10)` -> `("numeric", Some("38,10"))`). Only `numeric` cares
/// about the typmod's *value* — every other typed mapping below is
/// `Microsecond`-precision or otherwise typmod-independent by design (see
/// "Type mapping" in the phase doc), so this split is enough to let every
/// other match ignore it entirely.
fn split_typmod(s: &str) -> (&str, Option<&str>) {
    match s.find('(') {
        Some(i) if s.ends_with(')') => (s[..i].trim_end(), Some(&s[i + 1..s.len() - 1])),
        _ => (s, None),
    }
}

/// `numeric(p,s)` -> `Decimal128`/`Decimal256` when `p` fits; bare `numeric`
/// or a precision beyond `Decimal256`'s 76-digit ceiling stays `Utf8View`,
/// same as arbitrary precision (I4: `NaN` is reachable through any numeric
/// column regardless, and is a decode-time concern, not a mapping one).
fn map_numeric(typmod: Option<&str>) -> DataType {
    let Some(typmod) = typmod else { return DataType::Utf8View };
    let mut parts = typmod.split(',').map(str::trim);
    let precision: Option<u8> = parts.next().and_then(|p| p.parse().ok());
    let scale: i8 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    match precision {
        Some(p) if p <= 38 => DataType::Decimal128(p, scale),
        Some(p) if p <= 76 => DataType::Decimal256(p, scale),
        _ => DataType::Utf8View,
    }
}

/// The built-in half of "Type mapping"'s table — everything with no `.` in
/// its declared name (I8). `None` means the base name isn't a built-in this
/// build recognises (e.g. `money`, never specified).
///
/// Includes PostgreSQL's twelve built-in range and multirange types: unlike a
/// user-defined range (`CREATE TYPE ... AS RANGE`), these never appear
/// schema-qualified and have no `CREATE TYPE` of their own anywhere in the
/// file, so they need bare-name recognition here or they would wrongly fall
/// through to `Unknown` (confirmed against `fixtures/*/types/default.sql`'s
/// `t_range.v_range int4range`).
fn map_builtin(base: &str, typmod: Option<&str>, types: &[TypeDef]) -> Option<TypeOutcome> {
    use DataType::*;
    use arrow::datatypes::TimeUnit::Microsecond;
    let mapped = match base.to_ascii_lowercase().as_str() {
        "smallint" => Int16,
        "integer" => Int32,
        "bigint" => Int64,
        "boolean" => Boolean,
        "real" => Float32,
        "double precision" => Float64,
        "numeric" => map_numeric(typmod),
        "text" | "character varying" | "character" | "name" => Utf8View,
        "date" => Date32,
        "timestamp without time zone" => Timestamp(Microsecond, None),
        "timestamp with time zone" => Timestamp(Microsecond, Some("UTC".into())),
        "time without time zone" => Time64(Microsecond),
        "time with time zone" => Utf8View,
        "interval" => Utf8View,
        "uuid" => FixedSizeBinary(16),
        "bytea" => Binary,
        "json" | "jsonb" => Utf8View,
        "inet" | "cidr" | "macaddr" | "macaddr8" => Utf8View,
        // Built-in ranges, and their PG14+ multirange counterparts (I10):
        // both appear bare, never schema-qualified, so both need this table
        // rather than the user-defined lookup below (I8).
        other => {
            let (subtype, multi) = builtin_range_subtype(other)?;
            let (bound, bound_plan) = resolve_nested(subtype, types);
            return Some(if multi {
                TypeOutcome::Mapped(
                    list_of(range_struct(bound)),
                    NestedPlan::Multirange(Box::new(bound_plan)),
                )
            } else {
                TypeOutcome::Mapped(range_struct(bound), NestedPlan::Range(Box::new(bound_plan)))
            });
        }
    };
    Some(TypeOutcome::Mapped(mapped, NestedPlan::Scalar))
}

/// The subtype of one of PostgreSQL's twelve built-in range/multirange types,
/// plus whether the name was the multirange half. `None` for anything else.
///
/// **Hardcoded because the catalog holds it and the DDL does not.**
/// `TypeKind::Range::subtype` is populated only for a user-defined range;
/// `pg_dump` writes no `CREATE TYPE` at all for a built-in one. The six
/// multirange names carry the *same* subtypes as their range counterparts
/// and a different literal form, which is why they are told apart here
/// rather than sharing one answer (I10).
fn builtin_range_subtype(name: &str) -> Option<(&'static str, bool)> {
    let (subtype, multi) = match name {
        "int4range" => ("integer", false),
        "int8range" => ("bigint", false),
        "numrange" => ("numeric", false),
        "tsrange" => ("timestamp without time zone", false),
        "tstzrange" => ("timestamp with time zone", false),
        "daterange" => ("date", false),
        "int4multirange" => ("integer", true),
        "int8multirange" => ("bigint", true),
        "nummultirange" => ("numeric", true),
        "tsmultirange" => ("timestamp without time zone", true),
        "tstzmultirange" => ("timestamp with time zone", true),
        "datemultirange" => ("date", true),
        _ => return None,
    };
    Some((subtype, multi))
}

/// `List<child>`, with the element field named and nullable the way every
/// nested position is.
fn list_of(child: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", child, true)))
}

/// The five-field range struct, in [`RANGE_STRUCT_FIELDS`] order. The two
/// bounds share `bound` by construction; the three flags are always present,
/// so they are the one non-nullable thing this module builds.
fn range_struct(bound: DataType) -> DataType {
    DataType::Struct(Fields::from(vec![
        Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
        Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
        Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
    ]))
}

/// Resolve a type sitting *inside* a nested one — an array's element, a
/// composite's field, a range's bound.
///
/// Every non-`Mapped` outcome becomes `Utf8View` **in that position**, which
/// is exactly what the same type would have become at top level; the
/// recursion introduces no failure mode of its own, since it bottoms out on
/// the same mapping table whose worst answer is already a string.
fn resolve_nested(declared: &str, types: &[TypeDef]) -> (DataType, NestedPlan) {
    match resolve_declared_type(declared, types) {
        TypeOutcome::Mapped(data_type, plan) => (data_type, plan),
        _ => (DataType::Utf8View, NestedPlan::Scalar),
    }
}

/// The user-defined half: look `name` up in `types` (already schema-qualified,
/// matching how [`crate::preamble::TypeDef::name`] is stored) and resolve by
/// kind. A domain recurses on its base type — legal to nest (a domain over a
/// domain), and always finite: PostgreSQL cannot create a domain over a type
/// that does not exist yet, so there is no cycle to guard against.
fn resolve_user_type(name: &str, types: &[TypeDef]) -> TypeOutcome {
    let Some(def) = types.iter().find(|t| t.name == name) else {
        // Not a type of its own — but it might be a range's auto-created
        // multirange companion, which `pg_dump` never emits a `CREATE TYPE`
        // for at all (I10). Its only trace in the file is the
        // `multirange_type_name` parameter inside the range's own DDL, so
        // that's the only place left to look — and the range it names is
        // also where the companion's bound type comes from.
        let companion_of = types.iter().find(|t| {
            matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. } if n == name)
        });
        return match companion_of.map(|t| &t.kind) {
            Some(TypeKind::Range { subtype, .. }) => {
                let (bound, plan) = range_bound(subtype.as_deref(), types);
                TypeOutcome::Mapped(list_of(range_struct(bound)), NestedPlan::Multirange(plan))
            }
            _ => TypeOutcome::Unknown,
        };
    };
    match &def.kind {
        TypeKind::Enum { labels } if labels.is_empty() => TypeOutcome::EmptyEnum,
        TypeKind::Enum { .. } => TypeOutcome::Mapped(
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            NestedPlan::Scalar,
        ),
        TypeKind::Domain { base_type } => resolve_declared_type(base_type, types),
        // The field list is all-or-nothing: `None` means the grammar could
        // not read the body, and a `Struct` built from a short list would
        // refuse every valid row (see `TypeKind::Composite`). A zero-field
        // composite is a real type and maps to a zero-field `Struct` (I23).
        TypeKind::Composite { fields: None } => TypeOutcome::Unknown,
        TypeKind::Composite { fields: Some(fields) } => {
            let mut arrow_fields = Vec::with_capacity(fields.len());
            let mut plans = Vec::with_capacity(fields.len());
            for (field_name, declared) in fields {
                let (data_type, plan) = resolve_nested(declared, types);
                arrow_fields.push(Field::new(field_name, data_type, true));
                plans.push(plan);
            }
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(arrow_fields)),
                NestedPlan::Record(plans),
            )
        }
        TypeKind::Range { subtype, .. } => {
            let (bound, plan) = range_bound(subtype.as_deref(), types);
            TypeOutcome::Mapped(range_struct(bound), NestedPlan::Range(plan))
        }
        // Both genuinely information-free (see the phase doc's "`CREATE
        // TYPE`: six emitted forms") — one diagnostic bucket for both.
        TypeKind::Base | TypeKind::Shell => TypeOutcome::OpaqueBaseType,
    }
}

/// A range's bound type, from the `subtype = ...` parameter the DDL carried.
/// A range whose parameter list the grammar could not read keeps its struct
/// shape with `Utf8View` bounds — the bounds are still exactly the text the
/// file holds, which is what every other unmapped position falls back to.
fn range_bound(subtype: Option<&str>, types: &[TypeDef]) -> (DataType, Box<NestedPlan>) {
    let (data_type, plan) = match subtype {
        Some(subtype) => resolve_nested(subtype, types),
        None => (DataType::Utf8View, NestedPlan::Scalar),
    };
    (data_type, Box::new(plan))
}

/// Whether an array of `element` must stay a whole-column string: its element
/// type is opaque by construction, through any chain of domains.
///
/// **The test runs on the terminal of the domain walk, not on the declared
/// spelling** (I22). A domain inherits its base type's `typdelim` and its own
/// DDL records nothing about it, so `CREATE DOMAIN d AS box` makes `d[]` a
/// semicolon-separated literal named neither `box` nor `TypeKind::Base`.
/// `box` is checked by name because it is a built-in with no `CREATE TYPE` of
/// its own; a user-defined base type sets its delimiter in DDL this build
/// does not read, so `TypeKind::Base`/`Shell` are refused wholesale.
fn element_is_opaque(element: &str, types: &[TypeDef]) -> bool {
    let mut name = element.trim();
    // A domain chain visits each `CREATE DOMAIN` at most once, so the type
    // list's own length bounds the walk. `resolve_declared_type` recurses
    // through domains unbounded on the grounds that PostgreSQL cannot create
    // a cycle; the bound here costs nothing and keeps a hand-edited file from
    // spinning rather than merely failing.
    for _ in 0..=types.len() {
        if name.eq_ignore_ascii_case("box") {
            return true;
        }
        match types.iter().find(|t| t.name == name).map(|t| &t.kind) {
            Some(TypeKind::Domain { base_type }) => name = base_type.trim(),
            Some(TypeKind::Base | TypeKind::Shell) => return true,
            _ => return false,
        }
    }
    false
}

/// Map one declared type string — exactly as `pg_dump` wrote it, e.g. from
/// [`crate::preamble::DatabaseMetadata::tables`] — against `types`, that
/// same database's `CREATE TYPE`/`CREATE DOMAIN` list.
///
/// The array check runs first because a declared array type still carries
/// its element type's own qualification (`public.mood[]` contains a `.` too)
/// — I21: `pg_dump` never preserves dimensionality, so a single trailing
/// `[]` covers every array shape regardless of underlying dimensions.
pub fn resolve_declared_type(declared: &str, types: &[TypeDef]) -> TypeOutcome {
    let declared = declared.trim();
    if let Some(element) = declared.strip_suffix("[]") {
        // The element resolves through this same function, so nesting
        // composes with no special case: `public.comp[]` is
        // `List<Struct<…>>` and `integer[][]` (which `pg_dump` never writes,
        // I21) would be `List<List<Int32>>`.
        if element_is_opaque(element, types) {
            return TypeOutcome::OpaqueElementType;
        }
        let (data_type, plan) = resolve_nested(element, types);
        return TypeOutcome::Mapped(list_of(data_type), NestedPlan::Array(Box::new(plan)));
    }
    let (base, typmod) = split_typmod(declared);
    if base.contains('.') {
        return resolve_user_type(base, types);
    }
    map_builtin(base, typmod, types).unwrap_or(TypeOutcome::Unknown)
}

#[cfg(test)]
mod tests {
    use arrow::datatypes::TimeUnit;

    use super::*;

    fn ty(name: &str, kind: TypeKind) -> TypeDef {
        TypeDef { name: name.to_string(), kind }
    }

    #[test]
    fn maps_simple_builtins() {
        assert_eq!(
            resolve_declared_type("integer", &[]),
            TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("boolean", &[]),
            TypeOutcome::Mapped(DataType::Boolean, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("bytea", &[]),
            TypeOutcome::Mapped(DataType::Binary, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("uuid", &[]),
            TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar)
        );
    }

    #[test]
    fn maps_case_insensitively_for_the_binary_upgrade_dummy_column_shape() {
        // `INTEGER /* dummy */` -> preamble.rs already strips the comment,
        // leaving bare uppercase "INTEGER" (I5) -- pg_dump's own literal
        // casing there, not something we get to normalize upstream.
        assert_eq!(
            resolve_declared_type("INTEGER", &[]),
            TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar)
        );
    }

    #[test]
    fn text_like_types_map_to_utf8view_deliberately() {
        for declared in ["text", "character varying(16)", "character(10)", "name"] {
            assert_eq!(
                resolve_declared_type(declared, &[]),
                TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
            );
        }
    }

    #[test]
    fn timestamps_are_always_microsecond_regardless_of_typmod() {
        assert_eq!(
            resolve_declared_type("timestamp without time zone", &[]),
            TypeOutcome::Mapped(
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None),
                NestedPlan::Scalar
            )
        );
        assert_eq!(
            resolve_declared_type("timestamp with time zone", &[]),
            TypeOutcome::Mapped(
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, Some("UTC".into())),
                NestedPlan::Scalar
            )
        );
        assert_eq!(
            resolve_declared_type("time with time zone", &[]),
            TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
        );
    }

    #[test]
    fn numeric_picks_decimal_width_by_precision() {
        assert_eq!(
            resolve_declared_type("numeric(38,10)", &[]),
            TypeOutcome::Mapped(DataType::Decimal128(38, 10), NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric(39,0)", &[]),
            TypeOutcome::Mapped(DataType::Decimal256(39, 0), NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric", &[]),
            TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric(2,-2)", &[]),
            TypeOutcome::Mapped(DataType::Decimal128(2, -2), NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric(77,0)", &[]),
            TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
        );
    }

    #[test]
    fn unrecognized_builtin_is_unknown() {
        assert_eq!(resolve_declared_type("money", &[]), TypeOutcome::Unknown);
    }

    /// The twelve built-in range/multirange names carry their subtypes in the
    /// catalog, never in DDL (I10), so this table is the only place they
    /// exist. The multirange half maps to a `List` of the *same* range struct
    /// — same subtype, different literal form.
    #[test]
    fn builtin_ranges_and_their_multirange_companions_map_to_their_hardcoded_subtypes() {
        let bounds = [
            ("int4range", "int4multirange", DataType::Int32),
            ("int8range", "int8multirange", DataType::Int64),
            // Bare `numeric` has no Arrow decimal representation, so a
            // `numrange`'s bounds are text — the subtype's own mapping, not a
            // special case here.
            ("numrange", "nummultirange", DataType::Utf8View),
            ("tsrange", "tsmultirange", DataType::Timestamp(TimeUnit::Microsecond, None)),
            (
                "tstzrange",
                "tstzmultirange",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            ),
            ("daterange", "datemultirange", DataType::Date32),
        ];
        for (range, multirange, bound) in bounds {
            assert_eq!(
                resolve_declared_type(range, &[]),
                TypeOutcome::Mapped(
                    range_struct(bound.clone()),
                    NestedPlan::Range(Box::new(NestedPlan::Scalar))
                ),
                "{range}"
            );
            assert_eq!(
                resolve_declared_type(multirange, &[]),
                TypeOutcome::Mapped(
                    list_of(range_struct(bound)),
                    NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
                ),
                "{multirange}"
            );
        }
    }

    /// The collision `NestedPlan` exists for, asserted where it is otherwise
    /// invisible: one Arrow type, two literal forms
    /// (`{"[1,10)","[2,3)"}` versus `{[1,10),[2,3)}`).
    #[test]
    fn an_array_of_ranges_and_a_multirange_agree_on_the_type_and_not_on_the_plan() {
        let TypeOutcome::Mapped(array_type, array_plan) = resolve_declared_type("int4range[]", &[])
        else {
            panic!("int4range[] maps")
        };
        let TypeOutcome::Mapped(multi_type, multi_plan) =
            resolve_declared_type("int4multirange", &[])
        else {
            panic!("int4multirange maps")
        };
        assert_eq!(array_type, multi_type);
        assert_ne!(array_plan, multi_plan);
    }

    #[test]
    fn an_array_maps_to_a_list_of_its_element_type() {
        assert_eq!(
            resolve_declared_type("integer[]", &[]),
            TypeOutcome::Mapped(
                list_of(DataType::Int32),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
        let types = [ty("public.mood", TypeKind::Enum { labels: vec!["sad".into()] })];
        assert_eq!(
            resolve_declared_type("public.mood[]", &types),
            TypeOutcome::Mapped(
                list_of(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
        // An element type with no mapping is `Utf8View` *in that position*,
        // exactly as it would be at top level — the element boundaries are
        // still recovered, which is what a whole-column string would lose.
        assert_eq!(
            resolve_declared_type("interval[]", &[]),
            TypeOutcome::Mapped(
                list_of(DataType::Utf8View),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
    }

    /// The delimiter trap (I22): an array's separator is its *element type's*
    /// `typdelim`, and `box`'s is `;`. The refusal has to run on the terminal
    /// of the domain walk, since a domain's own DDL records nothing about it.
    #[test]
    fn an_array_over_an_opaque_element_type_is_refused_through_any_chain_of_domains() {
        let types = [
            ty("public.mybase", TypeKind::Base),
            ty("public.shellonly", TypeKind::Shell),
            ty("public.box_domain", TypeKind::Domain { base_type: "box".to_string() }),
            ty(
                "public.box_domain2",
                TypeKind::Domain { base_type: "public.box_domain".to_string() },
            ),
        ];
        for declared in [
            "box[]",
            "public.mybase[]",
            "public.shellonly[]",
            "public.box_domain[]",
            "public.box_domain2[]",
        ] {
            assert_eq!(
                resolve_declared_type(declared, &types),
                TypeOutcome::OpaqueElementType,
                "{declared}"
            );
        }
        // The scalar cases are unchanged: only the *array* is refused for its
        // element, and `box` itself is simply a type this build never mapped.
        assert_eq!(resolve_declared_type("box", &types), TypeOutcome::Unknown);
        assert_eq!(
            resolve_declared_type("public.box_domain", &types),
            TypeOutcome::Unknown,
            "a domain resolves through to its base, which this build has no mapping for"
        );
    }

    #[test]
    fn a_composite_maps_to_a_struct_of_its_declared_fields() {
        let types = [
            ty(
                "public.point2d",
                TypeKind::Composite {
                    fields: Some(vec![
                        ("x".to_string(), "integer".to_string()),
                        ("y".to_string(), "text".to_string()),
                    ]),
                },
            ),
            ty("public.empty_comp", TypeKind::Composite { fields: Some(vec![]) }),
            ty("public.broken", TypeKind::Composite { fields: None }),
        ];
        assert_eq!(
            resolve_declared_type("public.point2d", &types),
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(vec![
                    Field::new("x", DataType::Int32, true),
                    Field::new("y", DataType::Utf8View, true),
                ])),
                NestedPlan::Record(vec![NestedPlan::Scalar, NestedPlan::Scalar])
            )
        );
        // A zero-field composite is a real type with a real value, `()`
        // (I23) — a zero-field `Struct` is well-formed Arrow.
        assert_eq!(
            resolve_declared_type("public.empty_comp", &types),
            TypeOutcome::Mapped(DataType::Struct(Fields::empty()), NestedPlan::Record(Vec::new()))
        );
        // A body the grammar could not read is *not* a short field list:
        // `record_out` is positional, so a `Struct` built from one would
        // refuse every valid row.
        assert_eq!(resolve_declared_type("public.broken", &types), TypeOutcome::Unknown);
    }

    /// Nesting composes through the same function, so a composite field's own
    /// array and an array of composites are the same recursion in two orders.
    #[test]
    fn nesting_composes_in_both_orders() {
        let types = [
            ty(
                "public.point2d",
                TypeKind::Composite {
                    fields: Some(vec![("x".to_string(), "integer".to_string())]),
                },
            ),
            ty(
                "public.tagged",
                TypeKind::Composite {
                    fields: Some(vec![("tags".to_string(), "text[]".to_string())]),
                },
            ),
        ];
        let point = DataType::Struct(Fields::from(vec![Field::new("x", DataType::Int32, true)]));
        assert_eq!(
            resolve_declared_type("public.point2d[]", &types),
            TypeOutcome::Mapped(
                list_of(point),
                NestedPlan::Array(Box::new(NestedPlan::Record(vec![NestedPlan::Scalar])))
            )
        );
        assert_eq!(
            resolve_declared_type("public.tagged", &types),
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(vec![Field::new(
                    "tags",
                    list_of(DataType::Utf8View),
                    true
                )])),
                NestedPlan::Record(vec![NestedPlan::Array(Box::new(NestedPlan::Scalar))])
            )
        );
    }

    /// A user-defined range's subtype comes from its own DDL, and a range
    /// whose parameter list the grammar could not read keeps the struct with
    /// text bounds rather than losing the shape.
    #[test]
    fn a_user_range_maps_through_its_declared_subtype() {
        let types = [
            ty(
                "public.myrange",
                TypeKind::Range {
                    subtype: Some("double precision".to_string()),
                    multirange_type_name: None,
                },
            ),
            ty("public.bare", TypeKind::Range { subtype: None, multirange_type_name: None }),
        ];
        assert_eq!(
            resolve_declared_type("public.myrange", &types),
            TypeOutcome::Mapped(
                range_struct(DataType::Float64),
                NestedPlan::Range(Box::new(NestedPlan::Scalar))
            )
        );
        assert_eq!(
            resolve_declared_type("public.bare", &types),
            TypeOutcome::Mapped(
                range_struct(DataType::Utf8View),
                NestedPlan::Range(Box::new(NestedPlan::Scalar))
            )
        );
    }

    /// I10: a user range's auto-created multirange companion has no
    /// `CREATE TYPE` of its own anywhere in the dump — its only trace is the
    /// `multirange_type_name` parameter inside the range's own DDL, so a
    /// column declared with that companion name must still resolve, not
    /// fall through to `Unknown`.
    #[test]
    fn a_ranges_multirange_companion_resolves_even_with_no_type_def_of_its_own() {
        let types = [ty(
            "public.myrange",
            TypeKind::Range {
                subtype: Some("double precision".to_string()),
                multirange_type_name: Some("public.myrange_multi".to_string()),
            },
        )];
        assert_eq!(
            resolve_declared_type("public.myrange_multi", &types),
            TypeOutcome::Mapped(
                list_of(range_struct(DataType::Float64)),
                NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
            ),
            "the companion's bound type comes from the range that names it"
        );
        // A name that merely resembles a companion but isn't named by any
        // range's `multirange_type_name` stays `Unknown`.
        assert_eq!(resolve_declared_type("public.not_a_companion", &types), TypeOutcome::Unknown);
    }

    #[test]
    fn base_and_shell_types_are_opaque() {
        let types = [ty("public.gtype", TypeKind::Base), ty("public.forward", TypeKind::Shell)];
        assert_eq!(resolve_declared_type("public.gtype", &types), TypeOutcome::OpaqueBaseType);
        assert_eq!(resolve_declared_type("public.forward", &types), TypeOutcome::OpaqueBaseType);
    }

    #[test]
    fn a_user_type_absent_from_the_list_is_unknown() {
        assert_eq!(resolve_declared_type("public.nope", &[]), TypeOutcome::Unknown);
    }
}
