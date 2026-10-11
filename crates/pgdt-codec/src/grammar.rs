//! **Which grammar reads a column's text, and whether PostgreSQL refuses a
//! field** — the codec's part of a strict parse
//! (`docs/design/decisions.md`, "D103"). This module is the input grammar of
//! a type the register orders by nothing; [`check`] holds the refusal checks
//! a field is put to and what a strict parse checks nothing of and why, and
//! [`form`] the form a filter's literal has to take. A field with an order is
//! checked through its key's reader ([`crate::key`]).
//!
//! Not an API namespace: [`TextGrammar`], [`Geometric`] and
//! [`Unchecked`](crate::grammar::check::Unchecked) are declared by
//! [`crate::compare`], whose plans carry them, and `internal` re-exports what a
//! crate above calls.

pub(crate) mod check;
pub(crate) mod form;

use pgdt_model::{TypeDef, TypeKind};

use crate::declared::{
    array_element, builtin_name, domain_not_null, domain_terminal, find_type, is_box, split_typmod,
};

/// The input grammar of a built-in this build holds as its text and orders by
/// nothing
/// ([`ComparisonPlan::Refused`](crate::compare::ComparisonPlan::Refused)), or
/// of an enum whose labels are exactly none, which a strict parse reads a field
/// of it by all the same: a check needs the grammar alone, where an order would
/// need a comparison register row of its own (D40).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextGrammar {
    /// `bit_in`, or `varbit_in` where `varying`, handed the column's typmod
    /// (I73): `length` is the length a `bit(n)` field must have exactly and a
    /// `bit varying(n)` field may not pass, and `None` for `"bit"` and a bare
    /// `bit varying`, which bound no length.
    Bit { varying: bool, length: Option<u32> },
    /// One of the seven geometric types' input functions (I74).
    Geometric(Geometric),
    /// An array of `box`, which `array_in` splits at `box`'s `;` (I22) and
    /// whose elements `box_in` reads: the one array a column holds that no
    /// nested plan reads, its element type being opaque here. `not_null`
    /// is its element's, a `NOT NULL` domain over `box` refusing a NULL
    /// element ([`Position`](crate::declared::Position)).
    BoxArray { not_null: bool },
    /// A type whose input function refuses every value — the planner's and
    /// the extended statistics' internal types and `gtsvector` (I78), and an
    /// enum declaring no labels, which `enum_in` has none to read (I70) — so
    /// every field of it is refused.
    RefusesAll,
    /// A type whose input function refuses nothing at some supported major —
    /// `"char"`, `refcursor`, and `xid`, `xid8` and `cid`, which v16 narrowed
    /// (I79) — so a field of it is read and refused by nothing, as one is
    /// refused only where every supported major refuses it.
    RefusesNothing,
}

/// A geometric type, naming the input function a field of it is read by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometric {
    Point,
    Line,
    Lseg,
    Box,
    Path,
    Polygon,
    Circle,
}

/// The [`TextGrammar`] a field of a column declared `declared` is read by,
/// through any chain of domains, a domain handing its own typmod to its base
/// type's input function — `None` for a type with none here, an array among
/// them, whose elements a nested plan reads, but an array of `box`.
///
/// **`bit` alone is `bit(1)`**, the SQL word's length, and `"bit"` the catalog
/// type with no typmod, which is how `format_type` writes a `bit` column of
/// either (I8). A geometric type takes no typmod, so one written with one is
/// DDL no server holds, read as no grammar.
pub(crate) fn text_grammar(declared: &str, types: &[TypeDef]) -> Option<TextGrammar> {
    let terminal = domain_terminal(declared, types);
    let (base, _) = split_typmod(terminal);
    if let Some(element) = array_element(terminal) {
        return is_box(domain_terminal(element, types))
            .then(|| TextGrammar::BoxArray { not_null: domain_not_null(element, types) });
    }
    if base.contains('.') {
        // pg-refuses: I70 — an enum whose labels are exactly none.
        let labelless = matches!(
            find_type(base, types).map(|t| &t.kind),
            Some(TypeKind::Enum { labels, exact: true }) if labels.is_empty()
        );
        return labelless.then_some(TextGrammar::RefusesAll);
    }
    let (name, typmod, _) = builtin_name(terminal);
    // None of these takes a typmod, so one written with one is DDL no server
    // holds, read as no grammar.
    match &*name {
        // pg-refuses: I78 — an input function refusing every value.
        "pg_node_tree"
        | "pg_ndistinct"
        | "pg_dependencies"
        | "pg_mcv_list"
        | "pg_brin_bloom_summary"
        | "pg_brin_minmax_multi_summary"
        | "gtsvector" => return typmod.is_none().then_some(TextGrammar::RefusesAll),
        // `"char"` alone reaches here quoted, bare `char` being `character`.
        "char" | "refcursor" | "xid" | "xid8" | "cid" => {
            return typmod.is_none().then_some(TextGrammar::RefusesNothing);
        }
        _ => {}
    }
    let geometric = match &*name {
        "point" => Some(Geometric::Point),
        "line" => Some(Geometric::Line),
        "lseg" => Some(Geometric::Lseg),
        "box" => Some(Geometric::Box),
        "path" => Some(Geometric::Path),
        "polygon" => Some(Geometric::Polygon),
        "circle" => Some(Geometric::Circle),
        _ => None,
    };
    if let Some(geometric) = geometric {
        return typmod.is_none().then_some(TextGrammar::Geometric(geometric));
    }
    let varying = match &*name {
        "bit" => false,
        "bit varying" => true,
        _ => return None,
    };
    let length = match typmod {
        // `anybit_typmodin` takes one length of at least 1; a zero or
        // non-integer typmod is DDL no server holds, read as no grammar.
        Some(typmod) => Some(typmod.trim().parse::<u32>().ok().filter(|&n| n > 0)?),
        None if !varying && !base.starts_with('"') => Some(1),
        None => None,
    };
    Some(TextGrammar::Bit { varying, length })
}

#[cfg(test)]
mod tests;
