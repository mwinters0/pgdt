//! A declared type read against the catalog the preamble holds: its typmod
//! split off, a built-in's canonical name, the definition a reference names,
//! an array's element, a domain's terminal type and its `NOT NULL`, a base
//! type's delimiter, and the position a container holds a value at. Beneath
//! every module reading a declared type — the type mapping, the comparison
//! register, a strict parse's grammar and the datetime grammar — and reading
//! none of them.

use pgdt_core::internal::canonical_type_name;
use pgdt_model::{TypeDef, TypeKind};

/// **A position a container holds a value at, which a NULL may sit at** — an
/// array's element or a composite's field — and whether its type refuses one
/// there: a `NOT NULL` domain, or a domain over one ([`domain_not_null`]),
/// whose `domain_in` `array_in` and `record_in` hand a NULL element or field
/// (I76). A range's bound is no such position: `range_in` hands an infinite
/// bound to no input function, so its plan is bare. `plan` is a
/// [`NestedPlan`](crate::pgtype::NestedPlan) or a
/// [`NestedCompare`](crate::compare::NestedCompare).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Position<T> {
    pub plan: T,
    /// Whether a NULL here is refused, but under
    /// [`pgdt_model::PostgresInvalidValues::Ignore`].
    pub not_null: bool,
}

impl<T> Position<T> {
    /// A position a NULL may sit at.
    pub fn nullable(plan: T) -> Self {
        Self { plan, not_null: false }
    }

    /// The position of a value declared `declared`, refusing a NULL where
    /// that is a `NOT NULL` domain.
    pub(crate) fn of(plan: T, declared: &str, types: &[TypeDef]) -> Self {
        Self { plan, not_null: domain_not_null(declared, types) }
    }
}

/// A `numeric(p,s)`'s typmod, in `numerictypmodin`'s ranges: a precision of 1
/// to 1000 and, from PostgreSQL 15, a scale of -1000 to 1000 (I51).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumericTypmod {
    pub precision: u16,
    pub scale: i16,
}

/// An `interval` column's field qualifier, one of the thirteen `gram.y`'s
/// `opt_interval` writes and `intervaltypmodin` admits. It is part of the
/// column's typmod, which `interval_in` hands `DecodeInterval`: the unit a
/// number written with none takes, and whether `a:b` is minutes and seconds
/// (I84). A column with none reads its fields under the full range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalQualifier {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    YearToMonth,
    DayToHour,
    DayToMinute,
    DayToSecond,
    HourToMinute,
    HourToSecond,
    MinuteToSecond,
}

impl IntervalQualifier {
    /// Every qualifier, in `opt_interval`'s order.
    pub const ALL: [Self; 13] = [
        Self::Year,
        Self::Month,
        Self::Day,
        Self::Hour,
        Self::Minute,
        Self::Second,
        Self::YearToMonth,
        Self::DayToHour,
        Self::DayToMinute,
        Self::DayToSecond,
        Self::HourToMinute,
        Self::HourToSecond,
        Self::MinuteToSecond,
    ];

    /// The qualifier `words` spell — folded to lower case, single-spaced, and
    /// without `second`'s precision — as `opt_interval` reads them.
    fn parse(words: &str) -> Option<Self> {
        Some(match words {
            "year" => Self::Year,
            "month" => Self::Month,
            "day" => Self::Day,
            "hour" => Self::Hour,
            "minute" => Self::Minute,
            "second" => Self::Second,
            "year to month" => Self::YearToMonth,
            "day to hour" => Self::DayToHour,
            "day to minute" => Self::DayToMinute,
            "day to second" => Self::DayToSecond,
            "hour to minute" => Self::HourToMinute,
            "hour to second" => Self::HourToSecond,
            "minute to second" => Self::MinuteToSecond,
            _ => return None,
        })
    }

    /// `INTERVAL_RANGE(typmod)`: the `INTERVAL_MASK` bits of each field the
    /// qualifier spans, `MONTH`, `YEAR` and `DAY` being bits 1 to 3 and
    /// `HOUR`, `MINUTE` and `SECOND` 10 to 12 (`datetime.h`).
    pub(crate) fn range(self) -> i32 {
        let (year, month, day, hour, minute, second) =
            (1 << 2, 1 << 1, 1 << 3, 1 << 10, 1 << 11, 1 << 12);
        match self {
            Self::Year => year,
            Self::Month => month,
            Self::Day => day,
            Self::Hour => hour,
            Self::Minute => minute,
            Self::Second => second,
            Self::YearToMonth => year | month,
            Self::DayToHour => day | hour,
            Self::DayToMinute => day | hour | minute,
            Self::DayToSecond => day | hour | minute | second,
            Self::HourToMinute => hour | minute,
            Self::HourToSecond => hour | minute | second,
            Self::MinuteToSecond => minute | second,
        }
    }
}

/// Split `declared` into its base type name and typmod contents, if any
/// (`numeric(38,10)` -> `("numeric", Some("38,10"))`), where the typmod ends
/// the string. It says whether a name is schema-qualified; a built-in's is read
/// by [`builtin_name`]. Only `numeric` and `float` read the typmod's *value*
/// for their Arrow type, every other mapping below being
/// `Microsecond`-precision or otherwise typmod-independent; a `character`'s
/// and a `bit`'s length and an `interval`'s qualifier are read for its
/// comparison and grammar. A `(` inside a quoted name is the name's (I29).
pub(crate) fn split_typmod(s: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    let open = s.bytes().position(|b| {
        quoted ^= b == b'"';
        b == b'(' && !quoted
    });
    match open {
        Some(i) if s.ends_with(')') => (s[..i].trim_end(), Some(&s[i + 1..s.len() - 1])),
        _ => (s, None),
    }
}

/// A built-in's declared spelling — no `.` in it (I8) — as the one name
/// [`builtin_scalar`](crate::pgtype::builtin_scalar) and
/// [`builtin_range_subtype`](crate::pgtype::builtin_range_subtype) match on,
/// its typmod and an `interval`'s field qualifier, read the way PostgreSQL's
/// grammar reads them (`docs/design/roadmap.md`, "The input contract is valid
/// PostgreSQL").
///
/// - **A typmod may sit mid-name**: `format_type` writes
///   `timestamp(3) with time zone`, `time(2) without time zone` and
///   `interval day to second(3)`, so the one parenthesized group is cut out
///   wherever it falls.
/// - **`interval`'s field qualifier is the rest of its typmod**, so
///   `interval year to month` is `interval`, its qualifier
///   [`IntervalQualifier::YearToMonth`]; only `opt_interval`'s thirteen
///   spellings are one. Quoted, the typmod's first integer is the qualifier's
///   `INTERVAL_MASK` bits, as `intervaltypmodin` reads `"interval"(4)`.
/// - **An unquoted name is folded to lower case and its whitespace to one
///   space**, then read through the grammar's own keywords (`int`, `char`,
///   `varchar`, `dec`, `float(p)`, bare `timestamp`, …) and then as a catalog
///   name (`int4`, `bpchar`, `timestamptz`, …). `format_type` writes one of the
///   latter itself: `bpchar`, for a `character` column with no typmod.
/// - **A quoted name is a catalog name alone**, case and all, as the grammar
///   takes it: `"char"` is the one-byte internal type, not `character`, and
///   reaches no arm; nor does `"integer"` or any spelling only the grammar
///   knows, which names a type `pg_catalog` does not hold and so answers
///   quoted.
pub(crate) fn builtin_name(
    declared: &str,
) -> (std::borrow::Cow<'static, str>, Option<&str>, Option<IntervalQualifier>) {
    let (words, typmod) = match (declared.find('('), declared.rfind(')')) {
        (Some(open), Some(close)) if open < close => (
            format!("{} {}", &declared[..open], &declared[close + 1..]),
            Some(&declared[open + 1..close]),
        ),
        _ => (declared.to_owned(), None),
    };
    let words = words.trim();
    if let Some(quoted) = words.strip_prefix('"').and_then(|w| w.strip_suffix('"')) {
        let name = match catalog_name(quoted) {
            Some(sql) => sql.to_owned(),
            None if CATALOG_NAMES.iter().any(|&(_, sql)| sql == quoted) => words.to_owned(),
            None => quoted.to_owned(),
        };
        let qualifier = (name == "interval").then_some(typmod).flatten().and_then(|typmod| {
            let range = typmod.split(',').next()?.trim().parse::<i32>().ok()?;
            IntervalQualifier::ALL.into_iter().find(|q| q.range() == range)
        });
        return (name.into(), typmod, qualifier);
    }
    let words = words.split_ascii_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    let keyword = match words.as_str() {
        "int" => "integer",
        "dec" | "decimal" => "numeric",
        // `float(p)` is `real` through 24 bits of precision and `double
        // precision` past them; the precision is spent choosing, and the type
        // chosen takes no typmod.
        "float" => {
            let real = typmod.and_then(|p| p.trim().parse::<u8>().ok()).is_some_and(|p| p <= 24);
            return ((if real { "real" } else { "double precision" }).into(), None, None);
        }
        // The SQL word with no length is `character(1)`, as the grammar
        // gives it one; `bpchar`, the catalog name, is the type with none,
        // which is how `format_type` writes such a column (I8).
        "character" | "char" | "nchar" | "national char" | "national character" => {
            return ("character".into(), typmod.or(Some("1")), None);
        }
        "char varying"
        | "nchar varying"
        | "national char varying"
        | "national character varying" => "character varying",
        "timestamp" => "timestamp without time zone",
        "time" => "time without time zone",
        w => {
            let qualifier = w.strip_prefix("interval ").and_then(IntervalQualifier::parse);
            if qualifier.is_some() {
                return ("interval".into(), typmod, qualifier);
            }
            let name = catalog_name(w).map_or_else(|| words.clone(), str::to_owned);
            return (name.into(), typmod, None);
        }
    };
    (keyword.into(), typmod, None)
}

/// Each catalog type name whose SQL spelling is another, beside that
/// spelling. The right-hand names are the grammar's alone: `pg_catalog` holds
/// no type called `integer`.
const CATALOG_NAMES: [(&str, &str); 13] = [
    ("int2", "smallint"),
    ("int4", "integer"),
    ("int8", "bigint"),
    ("float4", "real"),
    ("float8", "double precision"),
    ("bool", "boolean"),
    ("bpchar", "character"),
    ("varchar", "character varying"),
    ("timestamp", "timestamp without time zone"),
    ("timestamptz", "timestamp with time zone"),
    ("time", "time without time zone"),
    ("timetz", "time with time zone"),
    ("varbit", "bit varying"),
];

/// A catalog type name whose SQL spelling is another, as that spelling —
/// `None` where the two agree or the name is not a built-in's.
fn catalog_name(name: &str) -> Option<&'static str> {
    CATALOG_NAMES.iter().find(|&&(catalog, _)| catalog == name).map(|&(_, sql)| sql)
}

/// The definition `reference` names — a declared type, a domain's base —
/// found by comparing both sides in their canonical spelling, so a quoted and
/// an unquoted spelling of one name agree and the server's case folding is
/// applied to each (I29). `None` for a reference that is not one type name.
pub(crate) fn find_type<'a>(reference: &str, types: &'a [TypeDef]) -> Option<&'a TypeDef> {
    let key = canonical_type_name(reference)?;
    types.iter().find(|t| canonical_type_name(&t.name).is_some_and(|name| name == key))
}

/// The range whose `multirange_type_name` is `reference`, compared as
/// [`find_type`] compares — the only trace a multirange companion has (I10).
pub(crate) fn companion_range<'a>(reference: &str, types: &'a [TypeDef]) -> Option<&'a TypeDef> {
    let key = canonical_type_name(reference)?;
    types.iter().find(|t| {
        matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. }
            if canonical_type_name(n).is_some_and(|name| name == key))
    })
}

/// How many more `CREATE TYPE`/`CREATE DOMAIN` definitions one path of a
/// recursive type walk may visit —
/// [`resolve_declared_type`](crate::pgtype::resolve_declared_type)'s and
/// [`comparison_for`]'s, through domains, composites, ranges and array elements
/// alike.
///
/// PostgreSQL's type graph is acyclic (I24), so a path visits each definition
/// at most once and the list's length is always enough. A path that runs out
/// has visited one twice: a cycle a hand-edited file made, or one the
/// name-keyed list holds where the server's did not, a rename the preamble
/// does not follow being enough. That walk answers `Unknown` — and a
/// comparison `Refused` — rather than recursing until the stack is gone
/// (I24).
///
/// [`comparison_for`]: crate::compare::comparison_for
#[derive(Debug, Clone, Copy)]
pub(crate) struct Visits(usize);

impl Visits {
    /// The allowance for one walk over `types`.
    pub(crate) fn over(types: &[TypeDef]) -> Self {
        Self(types.len())
    }

    /// One definition visited: what is left, or `None` where nothing was.
    pub(crate) fn spend(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }
}

/// The element type of an array declaration, in any of the six spellings
/// PostgreSQL's `Typename` production accepts — or `None` when `declared` is
/// not an array declaration at all.
///
/// **All six are one type, one array level deep** (I28): the parser keeps
/// "array of the element" and discards both the bracket count and the bounds,
/// so `integer[]`, `integer[3]`, `integer[][]`, `integer[3][4]`,
/// `integer ARRAY` and `integer ARRAY[4]` all answer `Some("integer")`.
/// `pg_dump` writes only the first (I21); the rest reach us from hand-written
/// SQL or another producer, which is inside the input contract
/// (`roadmap.md`, "The input contract is valid PostgreSQL").
///
/// The grammar is followed rather than approximated, in both directions.
/// `opt_array_bounds` is left-recursive with no cap, so a bracket run of any
/// length is still one array level even though `MAXDIM` is 6. But the `ARRAY`
/// alternatives take *at most one* bound, so `integer ARRAY[4][5]` is not a
/// declaration and answers `None` — as does text a server would reject
/// outright (`integer[abc]`, `integer[`). Inventing an array type for input
/// PostgreSQL refuses is the same mistake as reading a spelling more
/// literally than PostgreSQL does, pointed the other way.
///
/// **A quoted type name is never misread as an array** (I29). A name may
/// legally contain `[`, `]`, a space or the `ARRAY` keyword, and `pg_dump`
/// writes it quoted wherever it appears — so a column of `s."x ARRAY"` ends in
/// a `"` and both strip helpers bail, while `s."x ARRAY"[]` sheds the bound
/// outside the quotes and answers the name with its quotes intact, which
/// [`find_type`] then finds. The quotes are what keep the production
/// unambiguous, and stripping a suffix without checking for a closing quote
/// first is exactly what would break that.
pub(crate) fn array_element(declared: &str) -> Option<&str> {
    let declared = declared.trim();
    // `SimpleTypename ARRAY '[' Iconst ']'` and `SimpleTypename ARRAY`: at
    // most one bound and only in the `[n]` form, since the production spells
    // out `Iconst`. Tried before the unbounded run below and never after it.
    let head = match strip_bound(declared) {
        Some((head, true)) => head,
        _ => declared,
    };
    if let Some(element) = strip_array_keyword(head) {
        // What precedes the keyword is a bare `SimpleTypename`: a bracket of
        // its own or a second `ARRAY` is a syntax error, not a deeper array.
        return (strip_bound(element).is_none() && strip_array_keyword(element).is_none())
            .then_some(element);
    }
    // `SimpleTypename opt_array_bounds`: one or more `[]`/`[n]` pairs.
    let (mut element, _) = strip_bound(declared)?;
    while let Some((head, _)) = strip_bound(element) {
        element = head;
    }
    // `integer ARRAY[4][5]` arrives here, having shed both bounds: the
    // keyword took a second one, which the grammar has no production for.
    strip_array_keyword(element).is_none().then_some(element)
}

/// Strip one trailing `[]` or `[n]`, answering the head and whether the bound
/// was an `Iconst`: `opt_array_bounds` takes either form, the `ARRAY`
/// alternatives only the second. A bound that is neither empty nor an
/// unsigned integer is not a bound at all, and is left in place.
fn strip_bound(declared: &str) -> Option<(&str, bool)> {
    let inner = declared.trim_end().strip_suffix(']')?;
    let open = inner.rfind('[')?;
    let bound = inner[open + 1..].trim();
    let iconst = !bound.is_empty() && bound.bytes().all(|b| b.is_ascii_digit());
    (iconst || bound.is_empty()).then(|| (inner[..open].trim_end(), iconst))
}

/// Strip a trailing `ARRAY` keyword, case-insensitively — it is a keyword, so
/// `INTEGER ARRAY` and `Integer Array` are the same declaration (I28) — and
/// only where it stands as its own token, so the type name `myarray` is not
/// mistaken for an array of `my`.
fn strip_array_keyword(declared: &str) -> Option<&str> {
    let head = declared.trim_end();
    let split = head.len().checked_sub("ARRAY".len())?;
    let (element, keyword) = (head.get(..split)?, head.get(split..)?);
    if !keyword.eq_ignore_ascii_case("ARRAY") {
        return None;
    }
    let trimmed = element.trim_end();
    (trimmed.len() < split && !trimmed.is_empty()).then_some(trimmed)
}

/// Walk a chain of domains to the type name it bottoms out at — the declared
/// spelling of the first non-domain it reaches, or of `name` itself when that
/// is not a domain. The terminal is returned as the DDL spelled it, and a
/// reader asking whether it is a `box` tests it with [`is_box`].
///
/// `pgtype`'s `resolve_array` and the register's `array_comparison` test
/// this terminal rather than the declared spelling (I22, I26).
///
/// The loop is bounded by the type list's length — a domain chain visits each
/// `CREATE DOMAIN` at most once — which keeps a hand-edited file from
/// spinning rather than merely failing.
pub(crate) fn domain_terminal<'a>(name: &'a str, types: &'a [TypeDef]) -> &'a str {
    let mut name = name.trim();
    for _ in 0..=types.len() {
        match find_type(name, types).map(|t| &t.kind) {
            Some(TypeKind::Domain { base_type, .. }) => name = base_type.trim(),
            _ => break,
        }
    }
    name
}

/// Whether `declared` is a domain declaring `NOT NULL`, or a domain over one
/// — a chain [`domain_terminal`] walks — which `domain_in` refuses a NULL
/// under, `COPY` handing it each NULL field, and `array_in` and `record_in`
/// each NULL element and field (I76): asked of a column's declared type and
/// of each [`Position`]'s. An array of such a domain is not one: its column
/// holds a NULL array.
// pg-refuses: I76 — a domain's `NOT NULL`, which `domain_in` checks of a NULL.
pub(crate) fn domain_not_null(declared: &str, types: &[TypeDef]) -> bool {
    let mut name = declared.trim();
    for _ in 0..=types.len() {
        match find_type(name, types).map(|t| &t.kind) {
            Some(TypeKind::Domain { not_null: true, .. }) => return true,
            Some(TypeKind::Domain { base_type, .. }) => name = base_type.trim(),
            _ => return false,
        }
    }
    false
}

/// The `typdelim` of `declared`, through any chain of domains, a domain
/// copying its base type's (I22): `box`'s `;`, a base type's `DELIMITER`, and
/// otherwise `,` — `None` for a base type declaring one the preamble cannot
/// read, and for a shell, which no array is of.
pub(crate) fn typdelim(declared: &str, types: &[TypeDef]) -> Option<u8> {
    let terminal = domain_terminal(declared, types);
    if is_box(terminal) {
        return Some(b';');
    }
    match find_type(split_typmod(terminal).0, types).map(|t| &t.kind) {
        Some(TypeKind::Base { delimiter }) => *delimiter,
        Some(TypeKind::Shell) => None,
        _ => Some(b','),
    }
}

/// Whether `terminal` names the built-in `box`, read as [`builtin_name`]
/// reads every built-in: unqualified, and bare in any case or quoted in the
/// catalog's, as `--quote-all-identifiers` writes it (I8).
pub(crate) fn is_box(terminal: &str) -> bool {
    !split_typmod(terminal).0.contains('.') && builtin_name(terminal).0 == "box"
}

#[cfg(test)]
mod tests;
