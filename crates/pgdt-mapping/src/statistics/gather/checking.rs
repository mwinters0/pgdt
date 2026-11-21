//! The strict checker: a block's observer that gathers nothing and checks
//! every field as a strict parse does (`docs/design/decisions.md`, "D103").

use std::any::Any;
use std::sync::Arc;

use pgdt_core::internal::StatisticsScope;
use pgdt_model::BlockGathered;
use pgdt_model::internal::{BlockObserved, BlockObserver};

use super::read::{Absorbed, Reader};
use super::strict::FieldCheck;
use crate::statistics::account::{Charge, Term};

/// **An observer gathering nothing that checks every field of its block**,
/// what [`checker`](super::checker()) builds; its pieces are checkers too,
/// recovered as one by its `absorb`. It answers
/// [`BlockGathered::Checked`], or [`BlockGathered::Refused`] at the first
/// field refused, and never declines.
pub(super) struct Checker {
    reader: Reader,
    /// **Charges nothing**: it counts this checker among the observers the
    /// account may be short of
    /// ([`StatisticsAccount`](crate::statistics::account::StatisticsAccount)),
    /// what it holds to check being charged to no account
    /// (`docs/design/decisions.md`, "D81"). Last, as every observer's is.
    charge: Charge,
}

impl Checker {
    /// A checker of every field by `checks`, counted by `charge`.
    pub(super) fn new(checks: Arc<[Option<FieldCheck>]>, charge: Charge) -> Self {
        Self { reader: Reader::new(Some(checks), &[]), charge }
    }
}

/// Under the instrument, what a checker allocated in a statistics scope is
/// freed in one (`pgdt_core::instrument`).
#[cfg(feature = "introspect")]
impl Drop for Checker {
    fn drop(&mut self) {
        self.reader.free_attributed();
    }
}

impl BlockObserver for Checker {
    fn holds(&self, column: usize) -> bool {
        self.reader.holds(column)
    }

    fn begin_row(&mut self, offset: u64) {
        self.reader.begin_row(offset);
    }

    fn fold(&mut self, column: usize, bytes: &[u8], ends: bool) {
        self.reader.fold(column, bytes, ends, &mut ());
    }

    fn end_row(&mut self, _fields: usize) {
        self.reader.end_row();
    }

    fn close(mut self: Box<Self>, _end: u64) -> BlockObserved {
        let _attributed = StatisticsScope::enter();
        let gathered = match self.reader.refused.take() {
            Some(refusal) => BlockGathered::Refused(refusal),
            None => BlockGathered::Checked,
        };
        let ignored_refusals = self.reader.take_ignored();
        BlockObserved { gathered, ignored_refusals, checked_in_full: true }
    }

    fn piece(&self) -> Box<dyn BlockObserver> {
        let _attributed = StatisticsScope::enter();
        let charge = Charge::new(Arc::clone(self.charge.account()), Term::Pieces);
        Box::new(Self { reader: self.reader.piece(), charge })
    }

    /// A refusal outranks everything, and the earlier row's a later one's
    /// ([`Reader::absorb`]).
    fn absorb(&mut self, later: Box<dyn BlockObserver>) {
        let _attributed = StatisticsScope::enter();
        let mut later = later.into_any().downcast::<Self>().expect("a piece of this checker");
        if let Absorbed::Refused(refusal) = self.reader.absorb(&mut later.reader) {
            drop(later);
            self.reader.refuse(refusal, &mut ());
        }
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}
