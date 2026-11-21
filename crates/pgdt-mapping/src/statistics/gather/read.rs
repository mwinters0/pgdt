//! What every block observer reads a row's fields by, gathering or not: a
//! strict parse's check, a field handed over a run at a time, the rows
//! counted and the first refusal (`docs/design/decisions.md`, "D103",
//! "D138").

use std::sync::Arc;

use pgdt_codec::internal::FieldWalk;
use pgdt_core::internal::{PRINTOUT_ESCAPED_BYTES, StatisticsScope, printout_head};
use pgdt_model::internal::IgnoredRefusalsExt;
use pgdt_model::{FieldRefusal, IgnoredRefusals};

use super::bounds::Refused;
use super::clip::{LONG_FIELD_HEAD, LongRead};
use super::column::ColumnGatherer;
use super::strict::FieldCheck;

/// **What gathers a field beside its check**: a block's or a piece's
/// columns ([`Groups`](super::gatherer::Groups)), or nothing, for the
/// strict checker (`()`). Each answer is `None` for a column it does not
/// gather, which is every column once it has stopped.
pub(super) trait Gather {
    /// What reads column `column`'s field past its head, as its runs go by.
    fn long(&self, column: usize) -> Option<LongRead>;

    /// Gather column `column`'s field held whole: whether it is one
    /// PostgreSQL refuses and was gone past, or a refusal.
    fn observe(&mut self, column: usize, field: &[u8]) -> Option<Result<bool, Refused>>;

    /// [`Self::observe`] of a field `escaped` bytes long read past its head
    /// by `long`.
    fn observe_long(
        &mut self,
        column: usize,
        long: LongRead,
        escaped: usize,
    ) -> Option<Result<bool, Refused>>;

    /// The type column `column` is declared as, which a refusal names.
    fn declared_type(&self, column: usize) -> String;

    /// Free everything gathered, a field PostgreSQL refuses stopping the
    /// block.
    fn free(&mut self);
}

/// The strict checker gathers nothing.
impl Gather for () {
    fn long(&self, _: usize) -> Option<LongRead> {
        None
    }

    fn observe(&mut self, _: usize, _: &[u8]) -> Option<Result<bool, Refused>> {
        None
    }

    fn observe_long(&mut self, _: usize, _: LongRead, _: usize) -> Option<Result<bool, Refused>> {
        None
    }

    fn declared_type(&self, _: usize) -> String {
        String::new()
    }

    fn free(&mut self) {}
}

/// **A block's rows as one observer reads them**, whatever it gathers: each
/// field checked before it is gathered, where a strict parse checks them —
/// and after a decline too — the field handed over a run at a time held as
/// far as its head, the rows counted, and the first field PostgreSQL
/// refuses, after which no field is read.
pub(super) struct Reader {
    /// What a strict parse checks every field by, one per column, where this
    /// observer checks them
    /// ([`PostgresInvalidValues::Strict`](pgdt_model::PostgresInvalidValues::Strict)).
    checks: Option<Arc<[Option<FieldCheck>]>>,
    /// Each walking column's walk ([`FieldCheck::walk`]), built at its first
    /// field and readied as each ends — outside every statistics scope, a
    /// check being no statistic, so built and freed alike unattributed.
    walks: Vec<Option<FieldWalk>>,
    /// Whether each column's field is handed over whole
    /// ([`BlockObserver::holds`](pgdt_model::internal::BlockObserver::holds)),
    /// fixed for the block whatever it stops, and shared with its pieces.
    holds: Arc<[bool]>,
    /// Where the row being folded starts, relative to the block's first data
    /// byte.
    row_offset: u64,
    /// The field being handed over a run at a time, between its first run
    /// and its last.
    run: Option<RunField>,
    /// The first field this observer met that PostgreSQL refuses, by its
    /// type's `*_in` or its column's `NOT NULL`: what it gathered is freed
    /// and no further field is read. A piece carries it into the block it
    /// is absorbed into.
    pub(super) refused: Option<FieldRefusal>,
    /// The fields PostgreSQL refuses, by type or `NOT NULL`, that this
    /// observer keyed and went past, told to ignore them. A piece's is
    /// folded into its block's, its first numbered past the block's rows, as
    /// a refusal is.
    ignored: Option<IgnoredRefusals>,
    /// The rows this observer has been handed, and, in a block, those of
    /// every piece it has absorbed, **counted whether or not it has
    /// stopped**: a piece made before a decline can still refuse, and its
    /// line is numbered past every row before it ([`FieldRefusal::line`]).
    rows: u64,
}

/// What [`Reader::absorb`] found of a piece.
pub(super) enum Absorbed {
    /// The block had refused already: the piece's rows all follow that one.
    Dropped,
    /// The piece refused, its line numbered past the block's rows, and the
    /// block had not: the block refuses with it.
    Refused(FieldRefusal),
    /// Neither refused: the piece's rows are counted, and what it gathered
    /// is the block's to fold in.
    Rows,
}

impl Reader {
    /// A block's reader, checking every field by `checks` where it is
    /// given, and holding whole each field `columns` or `checks` read
    /// whole ([`ColumnGatherer::reads_whole`], [`FieldCheck::holds`]).
    pub(super) fn new(
        checks: Option<Arc<[Option<FieldCheck>]>>,
        columns: &[Option<ColumnGatherer>],
    ) -> Self {
        let checked = checks.as_deref().unwrap_or_default();
        let width = columns.len().max(checked.len());
        let holds = (0..width)
            .map(|at| {
                let gathered = columns.get(at).and_then(Option::as_ref);
                let checked = checked.get(at).and_then(Option::as_ref);
                gathered.is_some_and(ColumnGatherer::reads_whole)
                    || checked.is_some_and(FieldCheck::holds)
            })
            .collect();
        Self::with(checks, holds, None)
    }

    fn with(
        checks: Option<Arc<[Option<FieldCheck>]>>,
        holds: Arc<[bool]>,
        refused: Option<FieldRefusal>,
    ) -> Self {
        Self {
            checks,
            walks: Vec::new(),
            holds,
            row_offset: 0,
            run: None,
            refused,
            ignored: None,
            rows: 0,
        }
    }

    /// The reader of a piece of this reader's block: the same checks and
    /// the same fields held whole, carrying a refusal already met.
    pub(super) fn piece(&self) -> Self {
        let checks = self.checks.clone();
        Self::with(checks, Arc::clone(&self.holds), self.refused.clone())
    }

    pub(super) fn holds(&self, column: usize) -> bool {
        self.holds.get(column).copied().unwrap_or(false)
    }

    /// Whether this reader checks every field as a strict parse does.
    pub(super) fn checks_every_field(&self) -> bool {
        self.checks.is_some()
    }

    /// The fields gone past, told to ignore them, taken as the block closes.
    pub(super) fn take_ignored(&mut self) -> Option<IgnoredRefusals> {
        self.ignored.take()
    }

    /// A row begins at `offset`: a field a row the file's end cut short left
    /// being handed over is dropped, and the row is counted.
    pub(super) fn begin_row(&mut self, offset: u64) {
        self.abandon_run();
        self.rows += 1;
        self.row_offset = offset;
    }

    /// A row ends, its last field folded already.
    pub(super) fn end_row(&self) {
        debug_assert!(self.run.is_none(), "a row ends with its last field");
    }

    /// The next `bytes` of field `column`, `ends` where they are its last.
    pub(super) fn fold(
        &mut self,
        column: usize,
        bytes: &[u8],
        ends: bool,
        gather: &mut impl Gather,
    ) {
        if self.refused.is_some() {
            return;
        }
        if ends && self.run.is_none() {
            self.field(column, bytes, gather);
        } else {
            self.field_run(column, bytes, ends, gather);
        }
    }

    /// **Stop at a field PostgreSQL refuses**: the pass fails on it, so what
    /// was gathered is freed rather than finished, and no later field is read
    /// (`roadmap.md`, "A literal is guaranteed in `*_out`'s form and never
    /// read past `*_in`'s").
    pub(super) fn refuse(&mut self, refusal: FieldRefusal, gather: &mut impl Gather) {
        self.refused = Some(refusal);
        gather.free();
    }

    /// **Take in `later`'s refusal, its fields gone past and its rows**,
    /// `later` reading a piece whose rows all follow this reader's. A
    /// refusal outranks everything a piece gathered, and the earlier row's
    /// outranks a later one's: a reader that refused takes nothing, and one
    /// that has not takes a piece's, its line numbered past this reader's
    /// rows.
    pub(super) fn absorb(&mut self, later: &mut Reader) -> Absorbed {
        if self.refused.is_some() {
            return Absorbed::Dropped;
        }
        if let Some(mut refusal) = later.refused.take() {
            refusal.line += self.rows;
            return Absorbed::Refused(refusal);
        }
        let mut ignored = later.ignored.take();
        for column in ignored.iter_mut().flat_map(|ignored| &mut ignored.columns) {
            column.first.line += self.rows;
        }
        IgnoredRefusals::fold(&mut self.ignored, ignored);
        self.rows += later.rows;
        Absorbed::Rows
    }

    /// One field of the row begun, held whole: checked, then gathered.
    fn field(&mut self, column: usize, field: &[u8], gather: &mut impl Gather) {
        let check = self.checks.as_deref().and_then(|checks| checks.get(column)?.as_ref());
        if let Some(check) = check {
            let walk = walk_of(&mut self.walks, check, column);
            if let Some(value) = check.refused(walk, field) {
                let refusal = check.refusal(self.row_offset, self.rows, column, value);
                let _attributed = StatisticsScope::enter();
                return self.refuse(refusal, gather);
            }
        }
        let _attributed = StatisticsScope::enter();
        let Some(observed) = gather.observe(column, field) else { return };
        self.observed(column, observed, gather, || printout_head(field));
    }

    /// The next `bytes` of a field handed over a run at a time, `ends` where
    /// they are its last: held while they fit its head, and past it read
    /// as they go by — by its column's statistics and its check — and never
    /// held (`docs/design/decisions.md`, "D138").
    fn field_run(&mut self, column: usize, bytes: &[u8], ends: bool, gather: &mut impl Gather) {
        let mut run = self.run.take().unwrap_or_else(|| RunField::new(column));
        debug_assert_eq!(run.column, column, "a field's runs are handed over in turn");
        run.escaped += bytes.len();
        if !run.outgrown {
            if run.head.len() + bytes.len() <= RUN_HEAD {
                run.head.extend_from_slice(bytes);
                if ends {
                    self.field(column, &run.head, gather);
                } else {
                    self.run = Some(run);
                }
                return;
            }
            // Outgrown: what it held is read first, and kept as its head.
            run.outgrown = true;
            {
                let _attributed = StatisticsScope::enter();
                run.long = gather.long(column);
            }
            self.read_run(&mut run, None);
            let kept = RUN_HEAD - run.head.len();
            run.head.extend_from_slice(&bytes[..kept]);
        }
        self.read_run(&mut run, Some(bytes));
        if ends {
            self.end_run(run, gather);
        } else {
            self.run = Some(run);
        }
    }

    /// Feed `bytes` — or, where `None`, `run`'s head — to what reads a field
    /// past its head: its column's statistics and its check.
    fn read_run(&mut self, run: &mut RunField, bytes: Option<&[u8]>) {
        let bytes = bytes.unwrap_or(&run.head);
        if let Some(long) = &mut run.long {
            let _attributed = StatisticsScope::enter();
            long.run(bytes);
        }
        let check = self.checks.as_deref().and_then(|checks| checks.get(run.column)?.as_ref());
        if let Some(check) = check
            && let Some(walk) = walk_of(&mut self.walks, check, run.column)
        {
            walk.run(bytes);
        }
    }

    /// A field read past its head has ended: checked, then gathered, as a
    /// field held whole is.
    fn end_run(&mut self, mut run: RunField, gather: &mut impl Gather) {
        let column = run.column;
        let check = self.checks.as_deref().and_then(|checks| checks.get(column)?.as_ref());
        if let Some(check) = check {
            let walk = walk_of(&mut self.walks, check, column);
            if let Some(value) = check.refused_read(walk, run.escaped, &run.head) {
                let refusal = check.refusal(self.row_offset, self.rows, column, value);
                let _attributed = StatisticsScope::enter();
                drop(run.long.take());
                return self.refuse(refusal, gather);
            }
        }
        let _attributed = StatisticsScope::enter();
        let Some(long) = run.long.take() else { return };
        let Some(observed) = gather.observe_long(column, long, run.escaped) else { return };
        self.observed(column, observed, gather, || printout_head(&run.head));
    }

    /// Drop a field a row the file's end cut short left being handed over,
    /// its walk readied for the next.
    fn abandon_run(&mut self) {
        let Some(mut run) = self.run.take() else { return };
        if let Some(Some(walk)) = self.walks.get_mut(run.column) {
            walk.start();
        }
        let _attributed = StatisticsScope::enter();
        drop(run.long.take());
    }

    /// What gathering column `column`'s field answered: a value PostgreSQL
    /// refuses, by type or `NOT NULL`, gone past or refused, quoted as
    /// `value` quotes the field.
    fn observed(
        &mut self,
        column: usize,
        observed: Result<bool, Refused>,
        gather: &mut impl Gather,
        value: impl FnOnce() -> Option<String>,
    ) {
        let ignored = match observed {
            Ok(false) => return,
            Ok(true) => true,
            Err(Refused) => false,
        };
        let refusal = FieldRefusal {
            offset: self.row_offset,
            line: self.rows,
            column,
            declared_type: gather.declared_type(column),
            value: value(),
        };
        if ignored {
            IgnoredRefusals::add(&mut self.ignored, refusal);
        } else {
            self.refuse(refusal, gather);
        }
    }

    /// Under the instrument, what this reader allocated in a statistics
    /// scope is freed in one, as it was allocated (`pgdt_core::instrument`).
    #[cfg(feature = "introspect")]
    pub(super) fn free_attributed(&mut self) {
        let _attributed = StatisticsScope::enter();
        drop(std::mem::take(&mut self.holds));
        if let Some(mut run) = self.run.take() {
            drop(run.long.take());
        }
    }
}

/// The most of a field handed over in runs that is held: all of it while it
/// is no longer, read then as a field handed over whole is, and past it its
/// head — as much as a long text's reading keeps, and as a refusal is quoted
/// by.
const RUN_HEAD: usize =
    if LONG_FIELD_HEAD > PRINTOUT_ESCAPED_BYTES { LONG_FIELD_HEAD } else { PRINTOUT_ESCAPED_BYTES };

/// **A field handed over a run at a time**
/// ([`BlockObserver::fold`](pgdt_model::internal::BlockObserver::fold)): its
/// head as `COPY` wrote it, and once it outgrows that, what its column's
/// statistics read of it as its runs go by, its check's walk being the
/// column's own.
struct RunField {
    column: usize,
    /// Its first [`RUN_HEAD`] bytes: all of it while it is no longer.
    head: Vec<u8>,
    /// Its bytes so far.
    escaped: usize,
    /// Whether it outgrew its head, read from there as it goes by.
    outgrown: bool,
    /// Its text past its head, as its column's statistics read it — `None`
    /// for a column gathering nothing. Made, fed and freed in a statistics
    /// scope, as everything gathering holds is.
    long: Option<LongRead>,
}

impl RunField {
    fn new(column: usize) -> Self {
        Self { column, head: Vec::new(), escaped: 0, outgrown: false, long: None }
    }
}

/// Column `column`'s walk where `check` walks, built at its first field.
fn walk_of<'w>(
    walks: &'w mut Vec<Option<FieldWalk>>,
    check: &FieldCheck,
    column: usize,
) -> Option<&'w mut FieldWalk> {
    if walks.len() <= column {
        walks.resize_with(column + 1, || None);
    }
    let slot = &mut walks[column];
    if slot.is_none() {
        *slot = check.walk();
    }
    slot.as_mut()
}
