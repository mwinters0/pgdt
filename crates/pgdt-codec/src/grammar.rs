//! **Which grammar reads a column's text, and whether PostgreSQL refuses a
//! field** — the codec's part of a strict parse
//! (`docs/design/decisions.md`, "D103"): the input grammar of a type the
//! register orders by nothing, what a strict parse checks nothing of and why,
//! the refusal checks a field is put to, and the form a filter's literal has
//! to take, beside the grammar it describes. A field with an order is checked
//! through its key's reader ([`crate::key`]).
//!
//! Not an API namespace: [`TextGrammar`], [`Geometric`] and [`Unchecked`] are
//! declared by [`crate::compare`], whose plans carry them, and `internal`
//! re-exports what a crate above calls.

use pgdt_model::{TypeDef, TypeKind};

use crate::compare::{
    CompareKind, ComparisonDivergence, ComparisonPlan, NestedCompare, UnanswerableReason,
};
use crate::decode;
use crate::decode::Unread;
use crate::key::order::{checked_key, field_key, int_range};
use crate::pgtype::{
    Position, Visits, array_element, builtin_name, companion_range, domain_not_null,
    domain_terminal, find_type, is_box, split_typmod,
};

/// The input grammar of a built-in this build holds as its text and orders by
/// nothing ([`ComparisonPlan::Refused`]), or of an enum whose labels are
/// exactly none, which a strict parse reads a field
/// of it by all the same: a check needs the grammar alone, where an order
/// would need a comparison register row of its own (D40).
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
    /// element ([`Position`]).
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
/// range's bounds in order ([`checked_key`]). `json`, held as its text, is
/// read by `json_in`'s grammar ([`decode::json_in`]), and a position holding
/// a type ordered by nothing by its [`TextGrammar`] where it has one; any
/// other plan naming no type this build reads by — another held as its text,
/// or a range declaring its own canonical function — refuses nothing, no
/// reader here decoding its fields. A column whose own type is ordered by
/// nothing is [`ComparisonPlan::Refused`] and is read by [`grammar_refuses`]
/// instead.
///
/// [`PostgresInvalidValues::Strict`]: pgdt_model::PostgresInvalidValues::Strict
pub fn field_refused(plan: &ComparisonPlan, text: &str) -> bool {
    match plan {
        // `AS_TEXT`'s one member is `json`.
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => {
            !decode::json_in(text)
        }
        // Every text within its length is a value of these, and keying one
        // copies it.
        ComparisonPlan::Compared {
            kind: CompareKind::Text { length } | CompareKind::PaddedText { length },
            ..
        } => length.is_some_and(|length| decode::char_typmod_refuses(text, length)),
        ComparisonPlan::Compared { kind, .. } => {
            matches!(field_key(kind, text), Err(Unread::Refused))
        }
        ComparisonPlan::Nested(nested) => checked_key(nested, text).is_err(),
        ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_) => false,
    }
}

/// Whether PostgreSQL's input function refuses `text` as a field read by
/// `grammar`, of a type this build holds as its text and orders by nothing.
pub fn grammar_refuses(grammar: TextGrammar, text: &str) -> bool {
    let reads = match grammar {
        TextGrammar::Bit { varying, length } => decode::bit_in(text, varying, length),
        TextGrammar::Geometric(Geometric::Point) => decode::point_in(text),
        TextGrammar::Geometric(Geometric::Line) => decode::line_in(text),
        TextGrammar::Geometric(Geometric::Lseg) => decode::lseg_in(text),
        TextGrammar::Geometric(Geometric::Box) => decode::box_in(text),
        TextGrammar::Geometric(Geometric::Path) => decode::path_in(text),
        TextGrammar::Geometric(Geometric::Polygon) => decode::poly_in(text),
        TextGrammar::Geometric(Geometric::Circle) => decode::circle_in(text),
        TextGrammar::BoxArray { not_null } => {
            let element = NestedCompare::Uncomparable {
                declared: "box".into(),
                divergence: None,
                grammar: Some(TextGrammar::Geometric(Geometric::Box)),
                delimiter: Some(b';'),
            };
            let element = Position { plan: element, not_null };
            checked_key(&NestedCompare::Array(Box::new(element)), text).is_ok()
        }
        TextGrammar::RefusesAll => false,
        TextGrammar::RefusesNothing => true,
    };
    !reads
}

/// The form a **nested** literal has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence: the container's own grammar, then
/// how to spell what is inside it — as the dump does, which is each leaf
/// type's own output form.
///
/// That second half is advice rather than the boundary of what is accepted:
/// [`order_key`]'s integer arms are `str::parse`, so they take a leading `+`
/// and leading zeros no `*_out` writes, one of the exceptions [`accepted_form`]
/// lists (`docs/design/decisions.md`, "D55"). There is no per-leaf clause list, [`nested_key`]
/// answering only "not a value of this type".
///
/// [`order_key`]: crate::key::order::order_key
/// [`nested_key`]: crate::key::order::nested_key
pub fn nested_accepted_form(plan: &NestedCompare) -> String {
    // The one form with no leaf clause to add, because it has no leaf: an
    // `int2vector`'s elements are read by `int2vectorin` itself, so the
    // superset reaches all the way down and the sentence below would be false
    // here.
    if matches!(plan, NestedCompare::Int2Vector) {
        return "as whole numbers from -32768 to 32767 separated by spaces, and as nothing at \
                all for the empty vector"
            .to_string();
    }
    let container = match plan {
        NestedCompare::Array(element) if element.not_null => {
            "as an array literal — `{a,b}`, `{}` — with no `NULL` element, which its element \
             type, a `NOT NULL` domain, refuses"
        }
        NestedCompare::Array(_) => "as an array literal — `{a,b}`, `{}`, a bare `NULL` element",
        NestedCompare::Record(fields) if fields.iter().any(|(_, field)| field.not_null) => {
            "as a composite literal — `(a,b)`, a field left empty for NULL but where its type is \
             a `NOT NULL` domain"
        }
        NestedCompare::Record(_) => "as a composite literal — `(a,b)`, a field left empty for NULL",
        NestedCompare::Range { .. } => {
            "as a range literal — `[a,b)`, `empty`, a bound left empty for unbounded — whose \
             lower bound is not above its upper"
        }
        NestedCompare::Multirange { .. } => {
            "as a multirange literal — `{[a,b),[c,d)}`, `{}` — each member a range whose lower \
             bound is not above its upper"
        }
        // Answered above, before the leaf clause this arm cannot carry.
        NestedCompare::Int2Vector => "as an int2vector literal",
        // Neither is reachable: a leaf plan is never a column's whole
        // comparison, and an uncomparable one refuses before a literal is
        // read.
        NestedCompare::Leaf { .. } | NestedCompare::Uncomparable { .. } => "as a nested literal",
    };
    format!(
        "{container} — with each element, field or bound spelled as the dump spells it, in that \
         type's own output form"
    )
}

/// The form a literal of `kind` has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence — read after "which is written".
///
/// It lives beside the grammar rather than beside [`CompareKind`] because it
/// describes what [`literal_key`] and `pgdt_query::predicate::equality_comparison` accept, which is
/// each type's `*_out` form, widened by an integer's sign and leading zeros,
/// a float's sign, bare point, exponent case and `inf` or `nan` in any case,
/// a `uuid` or `macaddr` hex digit's case, a `uuid`'s omitted hyphens,
/// hyphens after any group of four and enclosing braces,
/// either `numeric` kind's leading or trailing point and leading zeros, a
/// network value's full-width or zero-padded netmask, an IPv6 address's
/// uncompressed, zero-padded or upper-case groups, a `cidr` written with no
/// netmask, and a time's trailing fractional zeros (`docs/design/decisions.md`, "D55").
/// `jsonb` needs the least here, its grammar being the whole of `jsonb_in`.
///
/// Two arms answer with the kind's own payload, because there the payload
/// *is* the answer: an enum's declared labels, and a `numeric(p,s)`'s scale
/// — see [`ENUM_LABELS_SHOWN`] and [`decimal_accepted_form`].
///
/// [`CompareKind::Text`] and [`CompareKind::PaddedText`] never refuse a
/// literal, and an enum with no labels is refused by [`comparison_for`]
/// outright, so both arms are unreachable; they are written out rather than
/// `unreachable!` so a future kind's mistake is not a panic on a diagnostic
/// path.
///
/// [`literal_key`]: crate::key::order::literal_key
/// [`comparison_for`]: crate::compare::comparison_for
pub fn accepted_form(kind: &CompareKind) -> String {
    use CompareKind as K;
    match kind {
        K::Bool => "`t` or `f`".into(),
        // The width is half the sentence, as for `oid`: past it is the
        // refusal a well-formed literal meets (I60).
        K::Int { bytes } => {
            let range = int_range(*bytes);
            format!("as a whole number from {} to {}", range.start(), range.end())
        }
        // The width *is* the refusal: `oidin` wraps a negative and this does
        // not, so the range is the useful half of the sentence.
        K::UnsignedInt => "as a whole number from 0 to 4294967295".into(),
        K::Float32 => {
            "as a number within `real`'s range, or `Infinity`, `-Infinity` or `NaN`".into()
        }
        K::Float64 => {
            "as a number within `double precision`'s range, or `Infinity`, `-Infinity` or `NaN`"
                .into()
        }
        // A typmod rejects an infinity (I34), so a `numeric(p,s)` — and a
        // `numeric` past 76 digits — has `NaN` and nothing else. Only the
        // typed arm carries a scale to be finer than: a `p > 76` column is
        // held as text and compared by value through `NumericKey`, which
        // normalizes rather than rescaling.
        // The digit bounds are `numeric_in`'s storage format's (I63), which
        // a typed arm's scale and `i256` already fall inside.
        K::Decimal { scale, .. } => decimal_accepted_form(*scale),
        K::Numeric { infinities: false, .. } => format!("{NUMERIC_DIGITS}, or `NaN`"),
        K::Numeric { infinities: true, .. } => {
            format!("{NUMERIC_DIGITS}, or `NaN`, `Infinity` or `-Infinity`")
        }
        K::Enum { labels, .. } if !labels.is_empty() => enum_accepted_form(labels),
        K::Enum { .. } => "as one of the type's own declared labels".into(),
        // The range is half the sentence, as for an integer: a day the
        // calendar lacks, or one past it, is the refusal a well-formed
        // literal meets (I61).
        K::Date => "`YYYY-MM-DD`, optionally suffixed ` BC`, a day from `4714-11-24 BC` to \
                    `5874897-12-31`, or `infinity`/`-infinity`"
            .into(),
        K::Time => {
            "`HH:MM:SS`, optionally with a fractional second, no later than `24:00:00`".into()
        }
        K::Timestamp { with_tz: false } => {
            "`YYYY-MM-DD HH:MM:SS`, optionally with a fractional second and suffixed ` BC`, from \
             `4714-11-24 00:00:00 BC` to `294276-12-31 23:59:59.999999`, or \
             `infinity`/`-infinity`"
                .into()
        }
        K::Timestamp { with_tz: true } => {
            "`YYYY-MM-DD HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, \
             optionally with a fractional second and suffixed ` BC`, from `4714-11-24 00:00:00+00 \
             BC` to `294276-12-31 23:59:59.999999+00`, or `infinity`/`-infinity`"
                .into()
        }
        K::TimeTz => {
            "`HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, optionally \
             with a fractional second, no later than `24:00:00`"
                .into()
        }
        // `interval_out` under `IntervalStyle = postgres` (I4), which is the
        // only style a `pg_dump` connection writes; the bounds are I62's.
        K::Interval { .. } | K::IntervalFields { .. } => {
            "the way `interval` prints it — `1 year 2 mons 3 days`, `-01:00:00`, `00:00:00` for \
             zero — each unit once, its minutes under `60` and seconds at most `60`, or \
             `infinity`/`-infinity`"
                .into()
        }
        // `network_in`'s grammar (I67), which reads more than this says.
        K::Network { cidr: false } => {
            "as an IPv4 or IPv6 address, optionally followed by `/bits`, an IPv4 one omitting no \
             octet but those past the netmask's whole bytes"
                .into()
        }
        K::Network { cidr: true } => {
            "as an IPv4 or IPv6 network, optionally followed by `/bits`, with no bit set below \
             the netmask, an IPv4 one omitting any trailing octets, which are zero"
                .into()
        }
        K::MacAddr { octets: 8 } => "as eight colon-separated hex pairs".into(),
        K::MacAddr { .. } => "as six colon-separated hex pairs".into(),
        K::Uuid => {
            "as 32 hex digits, grouped `8-4-4-4-12`, a hyphen allowed only after a group of four \
             but the last, the whole optionally in braces"
                .into()
        }
        K::Bytea => {
            "as `\\x` followed by hex pairs, or as `bytea_output = escape` writes it".into()
        }
        K::Jsonb => "as a JSON document".into(),
        K::Text { .. } | K::PaddedText { .. } => "as any text".into(),
    }
}

/// How many of an enum's labels [`accepted_form`] names before it stops
/// counting them out.
///
/// A count cap rather than a length cap (pgdt-model's `SPAN_STORED_TEXT_MAX_BYTES` is the other
/// shape): every label a message prints is printed whole, where a length cap
/// would hand the user a spelling that is not a label. Nothing bounds how
/// many labels a type declares.
///
/// The overflow clause has somewhere to send the reader: `pgdt info --detail`
/// prints every label of an enum column *and* lists every user-defined type
/// with its labels, both uncapped (`docs/design/decisions.md`, "The CLI").
const ENUM_LABELS_SHOWN: usize = 12;

/// The enum clause: the declared labels themselves, which are the whole of
/// what a refused enum literal is missing.
///
/// Each label is single-quoted with any interior quote doubled — the spelling
/// the dump's own `CREATE TYPE … AS ENUM (…)` writes and the CLI's `dequote`
/// accepts — so a printed label pastes straight back into
/// `--filter "col=<label>"`. The CLI's own `label_list` renders the same way
/// and is not shared with it, sitting a layer above.
fn enum_accepted_form(labels: &[String]) -> String {
    let shown = labels.len().min(ENUM_LABELS_SHOWN);
    let list = labels[..shown]
        .iter()
        .map(|label| format!("'{}'", label.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    match labels.len() - shown {
        0 => format!("as one of the type's declared labels: {list}"),
        more => format!(
            "as one of the type's declared labels: {list}, and {more} more; see `info --detail`"
        ),
    }
}

/// A scale-free `numeric` literal's shape: `numeric_in`'s storage bounds
/// (I63), stated as the digits either side of the point.
const NUMERIC_DIGITS: &str =
    "as a number of at most 131072 digits before the point and 16383 after it";

/// The `numeric(p,s)` clause, which is about the **scale** because that is
/// what the refusal is about: `decode::decimal_unscaled_digits` drops a
/// trailing digit only when it is zero, so `--filter 'price>1.005'` on a
/// `numeric(10,2)` is refused.
///
/// Precision says nothing here, binding a field alone: a literal wider than
/// the column can hold still compares against every value in it, as the
/// server coerces it with no typmod (I63). A negative scale is legal from
/// PostgreSQL 15 and means the column stores multiples of a power of ten,
/// which the literal's reader enforces by refusing to drop a non-zero digit
/// off the integer part.
fn decimal_accepted_form(scale: i8) -> String {
    match scale {
        0 => "as a whole number, or `NaN`".into(),
        s if s > 0 => {
            let plural = if s == 1 { "" } else { "s" };
            format!("as a number with at most {s} decimal place{plural}, or `NaN`")
        }
        s => {
            let step = format!("1{}", "0".repeat(usize::from(s.unsigned_abs())));
            format!("as a whole number that is a multiple of {step}, or `NaN`")
        }
    }
}

#[cfg(test)]
mod tests;
