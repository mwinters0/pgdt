//! Declared PostgreSQL type string -> Arrow `DataType`
//! (`docs/design/architecture.md`, "Type resolution").
//!
//! Pure, synchronous, no I/O — see `docs/design/layering.md`, L2. A declared
//! type is resolved against a single database's [`TypeDef`] list (already
//! selected by the caller — [`crate::resolve`] is the one that picks which
//! database), never against files or offsets.

use arrow::datatypes::DataType;

use crate::preamble::{TypeDef, TypeKind};

/// What a future nested-quoting decoder will handle (`docs/design/roadmap.md`,
/// Phase 4) — see
/// "`CREATE TYPE`: six emitted forms" in `docs/status/history/2026-08-22.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredKind {
    Array,
    Composite,
    Range,
}

/// The outcome of mapping one declared type string, mirroring
/// [`crate::resolve::ColumnResolution`] but at single-type granularity.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeOutcome {
    /// The dump alone determines the value; this is the Arrow type it maps
    /// to (`Utf8View` included — e.g. `text`, `interval`, are deliberately
    /// mapped there, not merely defaulted).
    Mapped(DataType),
    /// A declared type string this build has no mapping for at all — neither
    /// a built-in nor found in the database's `CREATE TYPE`/`DOMAIN` list.
    Unknown,
    /// Decodable in principle; deferred until the nested-quoting decoder exists.
    Deferred(DeferredKind),
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
/// Includes PostgreSQL's six built-in range types (`int4range`, `int8range`,
/// `numrange`, `tsrange`, `tstzrange`, `daterange`): unlike a user-defined
/// range (`CREATE TYPE ... AS RANGE`), these never appear schema-qualified,
/// so the array/composite/range trio's "declared as `public.x`" framing in
/// the phase doc's mapping table only covers the user-defined half — a
/// built-in range needs its own bare-name recognition here, or it would
/// wrongly fall through to `Unknown` (confirmed against
/// `fixtures/*/types/default.sql`'s `t_range.v_range int4range`).
fn map_builtin(base: &str, typmod: Option<&str>) -> Option<TypeOutcome> {
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
        "int4range" | "int8range" | "numrange" | "tsrange" | "tstzrange" | "daterange"
        | "int4multirange" | "int8multirange" | "nummultirange" | "tsmultirange"
        | "tstzmultirange" | "datemultirange" => {
            return Some(TypeOutcome::Deferred(DeferredKind::Range));
        }
        _ => return None,
    };
    Some(TypeOutcome::Mapped(mapped))
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
        // that's the only place left to look.
        let is_multirange_companion = types.iter().any(|t| {
            matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. } if n == name)
        });
        return if is_multirange_companion {
            TypeOutcome::Deferred(DeferredKind::Range)
        } else {
            TypeOutcome::Unknown
        };
    };
    match &def.kind {
        TypeKind::Enum { labels } if labels.is_empty() => TypeOutcome::EmptyEnum,
        TypeKind::Enum { .. } => TypeOutcome::Mapped(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        )),
        TypeKind::Domain { base_type } => resolve_declared_type(base_type, types),
        TypeKind::Composite { .. } => TypeOutcome::Deferred(DeferredKind::Composite),
        TypeKind::Range { .. } => TypeOutcome::Deferred(DeferredKind::Range),
        // Both genuinely information-free (see the phase doc's "`CREATE
        // TYPE`: six emitted forms") — one diagnostic bucket for both.
        TypeKind::Base | TypeKind::Shell => TypeOutcome::OpaqueBaseType,
    }
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
    if let Some(base) = declared.strip_suffix("[]") {
        let _ = base; // element type is the array decoder's concern, not recorded here
        return TypeOutcome::Deferred(DeferredKind::Array);
    }
    let (base, typmod) = split_typmod(declared);
    if base.contains('.') {
        return resolve_user_type(base, types);
    }
    map_builtin(base, typmod).unwrap_or(TypeOutcome::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(name: &str, kind: TypeKind) -> TypeDef {
        TypeDef { name: name.to_string(), kind }
    }

    #[test]
    fn maps_simple_builtins() {
        assert_eq!(resolve_declared_type("integer", &[]), TypeOutcome::Mapped(DataType::Int32));
        assert_eq!(resolve_declared_type("boolean", &[]), TypeOutcome::Mapped(DataType::Boolean));
        assert_eq!(resolve_declared_type("bytea", &[]), TypeOutcome::Mapped(DataType::Binary));
        assert_eq!(
            resolve_declared_type("uuid", &[]),
            TypeOutcome::Mapped(DataType::FixedSizeBinary(16))
        );
    }

    #[test]
    fn maps_case_insensitively_for_the_binary_upgrade_dummy_column_shape() {
        // `INTEGER /* dummy */` -> preamble.rs already strips the comment,
        // leaving bare uppercase "INTEGER" (I5) -- pg_dump's own literal
        // casing there, not something we get to normalize upstream.
        assert_eq!(resolve_declared_type("INTEGER", &[]), TypeOutcome::Mapped(DataType::Int32));
    }

    #[test]
    fn text_like_types_map_to_utf8view_deliberately() {
        for declared in ["text", "character varying(16)", "character(10)", "name"] {
            assert_eq!(
                resolve_declared_type(declared, &[]),
                TypeOutcome::Mapped(DataType::Utf8View)
            );
        }
    }

    #[test]
    fn timestamps_are_always_microsecond_regardless_of_typmod() {
        assert_eq!(
            resolve_declared_type("timestamp without time zone", &[]),
            TypeOutcome::Mapped(DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None))
        );
        assert_eq!(
            resolve_declared_type("timestamp with time zone", &[]),
            TypeOutcome::Mapped(DataType::Timestamp(
                arrow::datatypes::TimeUnit::Microsecond,
                Some("UTC".into())
            ))
        );
        assert_eq!(
            resolve_declared_type("time with time zone", &[]),
            TypeOutcome::Mapped(DataType::Utf8View)
        );
    }

    #[test]
    fn numeric_picks_decimal_width_by_precision() {
        assert_eq!(
            resolve_declared_type("numeric(38,10)", &[]),
            TypeOutcome::Mapped(DataType::Decimal128(38, 10))
        );
        assert_eq!(
            resolve_declared_type("numeric(39,0)", &[]),
            TypeOutcome::Mapped(DataType::Decimal256(39, 0))
        );
        assert_eq!(resolve_declared_type("numeric", &[]), TypeOutcome::Mapped(DataType::Utf8View));
        assert_eq!(
            resolve_declared_type("numeric(2,-2)", &[]),
            TypeOutcome::Mapped(DataType::Decimal128(2, -2))
        );
        assert_eq!(
            resolve_declared_type("numeric(77,0)", &[]),
            TypeOutcome::Mapped(DataType::Utf8View)
        );
    }

    #[test]
    fn unrecognized_builtin_is_unknown() {
        assert_eq!(resolve_declared_type("money", &[]), TypeOutcome::Unknown);
    }

    #[test]
    fn builtin_range_types_are_deferred_not_unknown() {
        for declared in ["int4range", "int8range", "numrange", "tsrange", "tstzrange", "daterange"]
        {
            assert_eq!(
                resolve_declared_type(declared, &[]),
                TypeOutcome::Deferred(DeferredKind::Range),
                "{declared}"
            );
        }
    }

    #[test]
    fn arrays_are_deferred_regardless_of_element_type() {
        assert_eq!(
            resolve_declared_type("integer[]", &[]),
            TypeOutcome::Deferred(DeferredKind::Array)
        );
        assert_eq!(
            resolve_declared_type("public.mood[]", &[]),
            TypeOutcome::Deferred(DeferredKind::Array)
        );
    }

    #[test]
    fn resolves_a_qualified_enum() {
        let types = [ty("public.mood", TypeKind::Enum { labels: vec!["sad".into()] })];
        assert_eq!(
            resolve_declared_type("public.mood", &types),
            TypeOutcome::Mapped(DataType::Dictionary(
                Box::new(DataType::Int32),
                Box::new(DataType::Utf8)
            ))
        );
    }

    #[test]
    fn empty_enum_is_its_own_outcome() {
        let types = [ty("public.mood", TypeKind::Enum { labels: vec![] })];
        assert_eq!(resolve_declared_type("public.mood", &types), TypeOutcome::EmptyEnum);
    }

    #[test]
    fn domain_resolves_transitively() {
        let types = [
            ty("public.a", TypeKind::Domain { base_type: "public.b".to_string() }),
            ty("public.b", TypeKind::Domain { base_type: "integer".to_string() }),
        ];
        assert_eq!(resolve_declared_type("public.a", &types), TypeOutcome::Mapped(DataType::Int32));
    }

    #[test]
    fn composite_and_range_are_deferred() {
        let types = [
            ty("public.point2d", TypeKind::Composite { fields: vec![] }),
            ty("public.myrange", TypeKind::Range { subtype: None, multirange_type_name: None }),
        ];
        assert_eq!(
            resolve_declared_type("public.point2d", &types),
            TypeOutcome::Deferred(DeferredKind::Composite)
        );
        assert_eq!(
            resolve_declared_type("public.myrange", &types),
            TypeOutcome::Deferred(DeferredKind::Range)
        );
    }

    #[test]
    fn builtin_multirange_types_are_deferred_not_unknown() {
        for declared in [
            "int4multirange",
            "int8multirange",
            "nummultirange",
            "tsmultirange",
            "tstzmultirange",
            "datemultirange",
        ] {
            assert_eq!(
                resolve_declared_type(declared, &[]),
                TypeOutcome::Deferred(DeferredKind::Range),
                "{declared}"
            );
        }
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
            TypeOutcome::Deferred(DeferredKind::Range)
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
