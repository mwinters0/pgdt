//! **What a pass records of one `COPY` block's rows, in one place**: its
//! census and unrepresentable count, its observer and, for TimescaleDB's
//! catalog, its rows as text — each part fed every field of a row from one
//! split of a held row, or from the runs a streaming reader hands over — and
//! the block's record finished from them at its close
//! (`docs/design/decisions.md`, "D169").

use std::sync::Arc;

use pgdt_core::internal::StatisticsScope;
use pgdt_core::{CopyHeader, Result, split_fields};

use crate::block::{CensusFold, FieldCount, Reach};
use crate::statistics::{BlockObserver, FieldRefusalExt, RowObserver};
use crate::timescaledb::{RecordedRows, recorded_field};
use crate::{BlockGathered, BlockRecord, LongestLine};

/// **Records one `COPY` block's rows, or one piece's**, into every part the
/// pass asked of it: the census and unrepresentable count, the block's
/// observer and its rows as text — any of them, or none.
///
/// **A held row is split once** ([`Self::row`]), each field handed to every
/// part recording it, as a streamed row's runs are ([`Self::fold`]) — D28's
/// rule on the recording path. A pass holds the block's recorder,
/// `Recorder<dyn BlockObserver>`, from its `CopyStart` to its `CopyEnd`, and
/// [`Self::finish`]es it into the block's record; a leader splitting the block
/// hands each piece one [`Self::piece`] made, a `Recorder<dyn RowObserver>`,
/// and [`Self::absorb`]s them in file order. Which of the two a recorder is,
/// its type says: a piece's can neither finish nor split.
pub struct Recorder<O: ?Sized = dyn BlockObserver> {
    /// The block's first data byte, which its observer places a row against.
    data_offset: u64,
    /// The census and unrepresentable count, for a block at the data level
    /// (`docs/design/decisions.md`, "D35", "D96").
    census: Option<CensusFold>,
    /// The rows as text, for a block whose rows a pass records
    /// (`docs/design/decisions.md`, "D133"): never a piece's, the leader
    /// reading such a block itself.
    rows: Option<RecordedRows>,
    /// The statistics observer, where the pass gathers or checks the block.
    observer: Option<Box<O>>,
}

impl Recorder {
    /// A recorder of the block whose first data byte is at `data_offset`,
    /// recording nothing until told.
    pub fn new(data_offset: u64) -> Self {
        Self { data_offset, census: None, rows: None, observer: None }
    }

    /// Census the block's rows, `width` columns wide, and count their
    /// unrepresentable values against `counter`.
    pub fn with_census(mut self, width: usize, counter: Arc<dyn FieldCount>) -> Self {
        self.census = Some(CensusFold::new(width, counter));
        self
    }

    /// Hand every row to `observer`, whose close is the block's statistics.
    pub fn with_observer(mut self, observer: Box<dyn BlockObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Record the block's rows as text, which holds every row whole
    /// ([`Self::holds_rows`]).
    pub fn with_rows(mut self) -> Self {
        self.rows = Some(RecordedRows::default());
        self
    }

    /// A recorder for a piece of this block: a census as wide, counted
    /// alike, and a piece of its observer, recording no row as text — a
    /// block recording its rows is never split.
    pub fn piece(&self) -> Recorder<dyn RowObserver> {
        debug_assert!(self.rows.is_none(), "a block recording its rows is read whole (D133)");
        Recorder {
            data_offset: self.data_offset,
            census: self.census.as_ref().map(CensusFold::fresh),
            rows: None,
            observer: self.observer.as_ref().map(|observer| observer.piece()),
        }
    }

    /// Fold `later`, made by [`Self::piece`] and handed rows that all follow
    /// every row this recorder has been handed, into this one.
    pub fn absorb(&mut self, later: Recorder<dyn RowObserver>) {
        if let (Some(census), Some(later)) = (&mut self.census, &later.census) {
            census.absorb(later);
        }
        if let (Some(observer), Some(later)) = (&mut self.observer, later.observer) {
            observer.absorb(later);
        }
    }

    /// **Close the block into its record**: `row_count` rows, its terminator
    /// at file offset `terminator_offset`, and `longest_line` — both the
    /// scanner's, which sees every row whether or not anything records it.
    /// The observer is closed, inside a statistics scope as what it frees was
    /// allocated (`pgdt_core::instrument`).
    ///
    /// **A gathered block's groups hold its `row_count` rows**, asserted in a
    /// debug build: a row the scanner counted and no host handed here fails
    /// every host's tests (`docs/design/decisions.md`, "D169").
    ///
    /// **A block whose observer met a field PostgreSQL refuses fails**, with
    /// [`pgdt_core::Error::FieldRefused`] naming `header`'s table and column,
    /// so a pass pushes no span for it (`roadmap.md`, "A literal is
    /// guaranteed in `*_out`'s form and never read past `*_in`'s").
    pub fn finish(
        self,
        header: &CopyHeader,
        row_count: u64,
        terminator_offset: u64,
        longest_line: Option<LongestLine>,
    ) -> Result<BlockRecord> {
        let mut record = BlockRecord {
            row_count,
            census: self.census.map(|mut census| census.take()),
            recorded_rows: self.rows.map(Box::new),
            longest_line,
            ..BlockRecord::default()
        };
        if let Some(observer) = self.observer {
            // Shared (`docs/design/decisions.md`, "D34"); a block that
            // declined records the allowance instead (D85). The `Arc` is
            // sealed inside the scope too, its allocation being part of what
            // the account retains.
            let _attributed = StatisticsScope::enter();
            let observed = observer.close(terminator_offset - self.data_offset);
            match observed.gathered {
                BlockGathered::Gathered(statistics) => {
                    debug_assert_eq!(
                        statistics.groups.iter().map(|group| group.rows).sum::<u64>(),
                        row_count,
                        "{}: a gathered block's groups hold every row its scanner counted",
                        header.table
                    );
                    record.statistics = Some(Arc::new(statistics));
                }
                BlockGathered::Declined { allowance } => {
                    record.statistics_declined = Some(allowance);
                }
                BlockGathered::Refused(refusal) => {
                    return Err(refusal.into_error(header, self.data_offset));
                }
                BlockGathered::Checked => {}
            }
            record.ignored_refusals = observed.ignored_refusals.map(Box::new);
            record.checked_in_full = observed.checked_in_full;
        }
        Ok(record)
    }
}

impl<O: ?Sized + RowObserver> Recorder<O> {
    /// The block's first data byte.
    pub fn data_offset(&self) -> u64 {
        self.data_offset
    }

    /// Whether any part folds a row's fields — the census or the observer —
    /// so a reader streaming a row hands its runs here.
    pub fn folds(&self) -> bool {
        self.census.is_some() || self.observer.is_some()
    }

    /// Whether every row must reach [`Self::row`] whole: a block recording
    /// its rows as text, which no run of a field can be.
    pub fn holds_rows(&self) -> bool {
        self.rows.is_some()
    }

    /// Whether column `column`'s field is wanted whole by any part folding
    /// it: its reader then holds it and hands it over once, as its last
    /// bytes.
    pub fn holds(&self, column: usize) -> bool {
        self.census.as_ref().is_some_and(|census| census.holds(column))
            || self.observer.as_ref().is_some_and(|observer| observer.holds(column))
    }

    /// **One row held whole**, its first byte at file offset `offset` and
    /// `raw` the still-escaped line without its terminator: split once, each
    /// field handed to every part.
    ///
    /// **The census reads a row holding no array only through its last
    /// tested column, and only those columns' tiers** (`CensusFold::reach`),
    /// whatever else reads the row; where nothing else does, the row is
    /// split only that far — not at all where no column is tested.
    pub fn row(&mut self, offset: u64, raw: &[u8]) {
        let census = self.census.as_ref().map(|census| census.reach(raw));
        let reach = match census {
            _ if self.observer.is_some() || self.rows.is_some() => usize::MAX,
            Some(Reach::Shaped) => usize::MAX,
            Some(Reach::Tested(fields)) => fields,
            None => return,
        };
        if reach == 0 {
            return;
        }
        if let Some(observer) = &mut self.observer {
            observer.begin_row(offset - self.data_offset);
        }
        let mut recorded = self.rows.as_ref().map(|_| Vec::new());
        let mut fields = 0;
        for (column, field) in split_fields(raw).take(reach).enumerate() {
            match (&mut self.census, census) {
                (Some(fold), Some(Reach::Shaped)) => fold.fold(column, field, true),
                (Some(fold), Some(Reach::Tested(fields))) if column < fields => {
                    fold.test(column, field);
                }
                _ => {}
            }
            if let Some(observer) = &mut self.observer {
                observer.fold(column, field, true);
            }
            if let Some(recorded) = &mut recorded {
                recorded.push(recorded_field(field));
            }
            fields = column + 1;
        }
        self.end_row(fields);
        if let (Some(rows), Some(recorded)) = (&mut self.rows, recorded) {
            rows.rows.push(recorded);
        }
    }

    /// A streamed row begins, its first byte at file offset `offset`, ahead
    /// of any of its fields' runs.
    pub fn begin_row(&mut self, offset: u64) {
        debug_assert!(self.rows.is_none(), "a recorded row is held whole");
        if let Some(observer) = &mut self.observer {
            observer.begin_row(offset - self.data_offset);
        }
    }

    /// The next `bytes` of field `column` of the row begun, `ends` where
    /// they are its last, to every part folding it.
    pub fn fold(&mut self, column: usize, bytes: &[u8], ends: bool) {
        if let Some(census) = &mut self.census {
            census.fold(column, bytes, ends);
        }
        if let Some(observer) = &mut self.observer {
            observer.fold(column, bytes, ends);
        }
    }

    /// The row ends, `fields` wide, its last field's end folded already.
    pub fn end_row(&mut self, fields: usize) {
        if let Some(census) = &mut self.census {
            census.end_row(fields);
        }
        if let Some(observer) = &mut self.observer {
            observer.end_row(fields);
        }
    }
}

#[cfg(test)]
mod tests;
