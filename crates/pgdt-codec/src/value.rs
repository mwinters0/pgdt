//! The type families: everything PostgreSQL says about one type — its
//! reader, its renderer, its refusal classifier and its key type — in one
//! module a family, which the `decode`, `nested` and `key` namespaces declare
//! from. A family reads no `crate::key`: `OrderKey` holds the families' key
//! types, so building one is the key's dispatch over them.

pub(crate) mod bit;
pub(crate) mod bool;
pub(crate) mod bytea;
pub(crate) mod datetime;
pub(crate) mod digits;
pub(crate) mod float;
pub(crate) mod geometric;
pub(crate) mod int;
pub(crate) mod json;
pub(crate) mod libc;
pub(crate) mod nested;
pub(crate) mod network;
pub(crate) mod numeric;
pub(crate) mod text;
pub(crate) mod uuid;

use std::cmp::Ordering;

/// The first non-`Equal` answer, or `Equal` when there is none — the
/// member-wise half of `json::Jsonb`'s order and the nested keys' alike.
pub(crate) fn first_difference(mut answers: impl Iterator<Item = Ordering>) -> Ordering {
    answers.find(|answer| answer.is_ne()).unwrap_or(Ordering::Equal)
}
