//! The statements declaring a type, a domain, a collation or an extension,
//! and the bodies of the `ALTER TYPE` statements the map builder spans apart:
//! an enum label added, a composite's attribute dropped, and one that could
//! change an enum's labels unread.

use super::lex::{
    find_ci, holds_word, matching_paren, parse_string_literal, split_top_level_commas, strip_kw,
};
use super::table::{
    check_constraints, extract_collation, extract_type_words, not_null_clause,
    parse_column_fragment,
};
use pgdt_core::internal::Cursor;
use pgdt_core::internal::{canonical_type_name, parse_type_name};
use pgdt_model::{CollationDef, Extension, TypeDef, TypeKind};

/// `CREATE TYPE <name>` in any of its six shapes, which reach five of
/// [`TypeKind`]'s variants — [`TypeKind::Domain`] comes from
/// [`parse_create_domain`] instead.
pub(super) fn parse_create_type(rest: &str) -> Option<TypeDef> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = rest[consumed..].trim_start();

    if after.is_empty() || after.starts_with(';') {
        return Some(TypeDef { name, kind: TypeKind::Shell });
    }
    if let Some(body) = strip_kw(after, "AS ENUM") {
        let open = body.find('(')?;
        let close = matching_paren(body.as_bytes(), open)?;
        // A fragment that is no plain string literal — an `E''` one, say — is
        // a label this build cannot read, so the set it leaves is short.
        let fragments = split_top_level_commas(&body[open + 1..close]);
        let labels: Vec<String> =
            fragments.iter().copied().filter_map(parse_string_literal).collect();
        let exact = labels.len() == fragments.len();
        return Some(TypeDef { name, kind: TypeKind::Enum { labels, exact } });
    }
    if let Some(body) = strip_kw(after, "AS RANGE") {
        let open = body.find('(')?;
        let close = matching_paren(body.as_bytes(), open)?;
        let mut subtype = None;
        let mut multirange_type_name = None;
        let mut canonical = None;
        for kv in split_top_level_commas(&body[open + 1..close]) {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let k = k.trim();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("subtype") {
                subtype = Some(v);
            } else if k.eq_ignore_ascii_case("multirange_type_name") {
                multirange_type_name = Some(canonical_type_name(&v).unwrap_or(v));
            } else if k.eq_ignore_ascii_case("canonical") {
                canonical = Some(v);
            }
        }
        return Some(TypeDef {
            name,
            kind: TypeKind::Range { subtype, multirange_type_name, canonical },
        });
    }
    if let Some(body) = strip_kw(after, "AS") {
        let body = body.trim_start();
        if body.starts_with('(') {
            let close = matching_paren(body.as_bytes(), 0)?;
            let inner = &body[1..close];
            // All-or-nothing (see [`TypeKind::Composite`]): one unparseable
            // fragment discards the whole list rather than shortening it. An
            // empty body is a zero-field composite, not a failure (I23).
            let fields = if inner.trim().is_empty() {
                Some(Vec::new())
            } else {
                split_top_level_commas(inner).into_iter().map(parse_column_fragment).collect()
            };
            return Some(TypeDef { name, kind: TypeKind::Composite { fields } });
        }
        return Some(TypeDef { name, kind: TypeKind::Base { delimiter: None } });
    }
    // `CREATE TYPE x (INPUT = ..., OUTPUT = ...);` — the C-level base type
    // shape, with no `AS` at all.
    Some(TypeDef { name, kind: TypeKind::Base { delimiter: base_type_delimiter(after) } })
}

/// The `typdelim` a base type's parameter list `body` declares: the first
/// byte of its `DELIMITER` literal, as `DefineType` takes `p[0]` of it, and
/// `,` where it names none (I22). `None` where the list is not closed or the
/// value is no plain string literal — `pg_dump` writing one through
/// `appendStringLiteralAH`, whose doubled backslash under
/// `standard_conforming_strings = off` leaves the first byte as it was. A v15+
/// server writes a non-ASCII `typdelim` as `charout`'s octal escape,
/// `DELIMITER = '\377'`, which a restore takes as `\`; this reads that byte,
/// and the split declines it.
fn base_type_delimiter(body: &str) -> Option<u8> {
    if !body.starts_with('(') {
        return None;
    }
    let close = matching_paren(body.as_bytes(), 0)?;
    let Some(value) = split_top_level_commas(&body[1..close]).into_iter().find_map(|kv| {
        let (key, value) = kv.split_once('=')?;
        key.trim().eq_ignore_ascii_case("delimiter").then_some(value.trim())
    }) else {
        return Some(b',');
    };
    let literal = parse_string_literal(value)?;
    // The literal alone, nothing after its closing quote.
    (format!("'{}'", literal.replace('\'', "''")) == value)
        .then(|| literal.bytes().next().unwrap_or(0))
}

/// `CREATE DOMAIN <name> AS <basetype> [COLLATE ...] [constraints...];`
pub(super) fn parse_create_domain(rest: &str) -> Option<TypeDef> {
    let (name, consumed) = parse_type_name(rest)?;
    let after_as = strip_kw(rest[consumed..].trim_start(), "AS")?;
    let base_type = extract_type_words(after_as);
    if base_type.is_empty() {
        return None;
    }
    let collation = extract_collation(after_as);
    let not_null = not_null_clause(after_as).is_some();
    let check = !check_constraints(after_as).is_empty();
    Some(TypeDef { name, kind: TypeKind::Domain { base_type, collation, not_null, check } })
}

/// `CREATE COLLATION <name> (provider = …[, deterministic = false][, locale =
/// …][, rules = …]);` — or the `CREATE COLLATION <name> FROM <other>;` copy
/// form, which carries no option list at all.
///
/// Only `deterministic` is read, and only at the option list's top level: a
/// `locale = 'x, deterministic = false'` literal is stepped over, since
/// [`split_top_level_commas`] respects quoting and an ICU locale is an
/// arbitrary string the server never re-quotes on the way out.
///
/// The copy form parses as `deterministic = true`, which is *not* the same
/// claim as reading the source collation's own determinism: `CREATE COLLATION
/// x FROM y` copies `collisdeterministic`, so a copy of a non-deterministic
/// collation would be called deterministic here. `pg_dump` never writes the
/// copy form (I42), so the shape is reachable only from a hand-written file,
/// where under-claiming costs an unprinted note rather than a wrong row
/// set.
pub(super) fn parse_create_collation(rest: &str) -> Option<CollationDef> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = rest[consumed..].trim_start();
    let mut deterministic = true;
    if after.starts_with('(') {
        let close = matching_paren(after.as_bytes(), 0)?;
        for option in split_top_level_commas(&after[1..close]) {
            let Some((key, value)) = option.split_once('=') else { continue };
            if key.trim().eq_ignore_ascii_case("deterministic")
                && value.trim().eq_ignore_ascii_case("false")
            {
                deterministic = false;
            }
        }
    }
    Some(CollationDef { name, deterministic })
}

/// `CREATE EXTENSION [IF NOT EXISTS] <name> [WITH] [SCHEMA <schema>];`
pub(super) fn parse_create_extension(rest: &str) -> Option<Extension> {
    let rest = strip_kw(rest, "IF NOT EXISTS").unwrap_or(rest);
    let mut cur = Cursor::new(rest.as_bytes());
    cur.skip_spaces();
    let name = cur.parse_ident()?;
    let after = &rest[cur.pos()..];
    let schema = find_ci(after, "SCHEMA").and_then(|idx| {
        let mut c = Cursor::new(&after.as_bytes()[idx + "SCHEMA".len()..]);
        c.skip_spaces();
        c.parse_ident()
    });
    Some(Extension { name, schema })
}

/// Parse the body of `ALTER TYPE <name> ADD VALUE '<label>' [BEFORE|AFTER
/// '<other>'];` — the `--binary-upgrade` shape for enum labels (I6) — after
/// `ALTER TYPE` has already been stripped. `BEFORE`/`AFTER` is ignored: a
/// `--binary-upgrade` dump only ever emits these in declaration order
/// (`fixtures/*/types/binary-upgrade.sql`), so appending is equivalent to
/// respecting them. Used by [`crate::map::classify`] to recognize the statement
/// as its own [`pgdt_model::SpanBody::AlterTypeAddValue`] span, since that
/// module has no already-open `TypeDef` to fold into the way
/// `fold_alter_type_add_value` does for
/// [`dump_metadata_from_spans`](super::dump_metadata_from_spans).
// deficiency: KD89 — a hand-written `ADD VALUE … BEFORE` or `AFTER` placing
// the label anywhere but last is folded as appended, so the label orders after
// every other: an ordering term, a group's bounds and the pruning they drive
// place it where the server does not, and a query can lose its rows.
pub(crate) fn parse_alter_type_add_value_body(rest: &str) -> Option<(String, String)> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = strip_kw(strip_kw(rest[consumed..].trim_start(), "ADD")?, "VALUE")?;
    let label = parse_string_literal(after)?;
    Some((name, label))
}

/// Whether the body of an `ALTER TYPE`, after `ALTER TYPE` has been stripped,
/// is one that could change an enum's labels — `ADD VALUE`, `RENAME VALUE`,
/// in any form, the keyword `VALUE` being in no other subcommand, so the
/// word anywhere after the name counts, an identifier's included — answered
/// where [`parse_alter_type_add_value_body`] does not read it, as the type it
/// names, `None` where it names none this grammar reads. Such a statement
/// leaves the labels inexact (`fold_enum_labels_unread`). `pub(crate)` for
/// [`crate::map::classify`], as [`parse_alter_type_add_value_body`] is.
pub(crate) fn alter_type_labels_unread(rest: &str) -> Option<Option<String>> {
    let (name, after) = match parse_type_name(rest) {
        Some((name, consumed)) => (Some(name), &rest[consumed..]),
        None => (None, rest),
    };
    holds_word(after, "VALUE").then_some(name)
}

/// Parse the body of `ALTER TYPE <name> DROP ATTRIBUTE <attr>;`, after `ALTER
/// TYPE` has been stripped — the statement a `--binary-upgrade` dump drops a
/// composite's placeholder for a dropped attribute with (I5) — as the type
/// and the attribute. `None` for anything more than that
/// one subcommand, which `dumpCompositeType` never writes. `pub(crate)` for
/// [`crate::map::classify`], as [`parse_alter_type_add_value_body`] is.
pub(crate) fn parse_alter_type_drop_attribute_body(rest: &str) -> Option<(String, String)> {
    let (name, consumed) = parse_type_name(rest)?;
    let after = strip_kw(rest[consumed..].trim_start(), "DROP ATTRIBUTE")?;
    let mut cur = Cursor::new(after.as_bytes());
    let attribute = cur.parse_ident()?;
    (after[cur.pos()..].trim() == ";").then_some((name, attribute))
}
