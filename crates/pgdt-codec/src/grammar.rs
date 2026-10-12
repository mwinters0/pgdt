//! **Which grammar reads a column's text, and whether PostgreSQL refuses a
//! field** — the codec's part of a strict parse
//! (`docs/design/decisions.md`, "D103"). The namespace declaring the strict
//! parse's three API types: [`TextGrammar`] and [`Geometric`], the input
//! grammar of a type the register orders by nothing, which the comparison
//! register's plans carry; and [`Unchecked`], why a strict parse checks
//! nothing of a value, which its listing of a column carries.
//!
//! `input` reads a column's grammar off its declaration, and `check` holds the
//! refusal checks a field is put to and what a strict parse checks nothing
//! of; `internal` re-exports what a crate above calls. A field with an order
//! is checked through its key's reader ([`crate::key`]).

pub(crate) mod check;
pub(crate) mod input;

pub use self::check::Unchecked;
pub use self::input::{Geometric, TextGrammar};
