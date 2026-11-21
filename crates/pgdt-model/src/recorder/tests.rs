use std::any::Any;
use std::sync::Arc;

use pgdt_core::{CopyHeader, split_fields};

use super::Recorder;
use crate::block::{CensusFold, FieldCount, Kept};
use crate::internal::BlockCensusExt;
use crate::statistics::{BlockObserved, BlockObserver, RowObserver};
use crate::{BlockCensus, BlockGathered, UnrepresentableTier};

/// Counts a first column's `inf` past the format, from a head as long,
/// and a second's fields holding `inf` anywhere past the engine, whole.
struct Infs;

impl FieldCount for Infs {
    fn tested(&self) -> &[(usize, Kept)] {
        &[(0, Kept::Head(3)), (1, Kept::Whole)]
    }

    fn tier(&self, column: usize, field: &[u8]) -> Option<UnrepresentableTier> {
        match column {
            0 => (field == b"inf").then_some(UnrepresentableTier::Format),
            1 => field.windows(3).any(|w| w == b"inf").then_some(UnrepresentableTier::Engine),
            _ => panic!("column {column} is not tested"),
        }
    }
}

/// An observer keeping each row it is handed as its fields' bytes, a run
/// at a time joined, and its offset.
#[derive(Default)]
struct Rows {
    rows: Vec<(u64, Vec<Vec<u8>>)>,
}

impl RowObserver for Rows {
    fn holds(&self, _column: usize) -> bool {
        false
    }

    fn begin_row(&mut self, offset: u64) {
        self.rows.push((offset, vec![Vec::new()]));
    }

    fn fold(&mut self, column: usize, bytes: &[u8], _ends: bool) {
        let (_, fields) = self.rows.last_mut().expect("a row begun");
        if fields.len() <= column {
            fields.resize(column + 1, Vec::new());
        }
        fields[column].extend_from_slice(bytes);
    }

    fn end_row(&mut self, fields: usize) {
        let (_, held) = self.rows.last_mut().expect("a row begun");
        assert_eq!(held.len(), fields, "a row ends as wide as its fields");
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl BlockObserver for Rows {
    fn close(self: Box<Self>, _end: u64) -> BlockObserved {
        BlockObserved {
            gathered: BlockGathered::Checked,
            ignored_refusals: None,
            checked_in_full: false,
        }
    }

    fn piece(&self) -> Box<dyn RowObserver> {
        Box::new(Rows::default())
    }

    fn absorb(&mut self, later: Box<dyn RowObserver>) {
        let later = later.into_any().downcast::<Rows>().expect("a piece of these rows");
        self.rows.extend(later.rows);
    }
}

fn header() -> CopyHeader {
    CopyHeader {
        schema: Some("public".to_owned()),
        table: "t".to_owned(),
        columns: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
    }
}

/// What a recorder finishes with for its census.
fn census_of(recorder: Recorder) -> BlockCensus {
    let end = recorder.data_offset();
    let record = recorder.finish(&header(), 0, end, None).expect("nothing refuses");
    record.census.expect("a censused block")
}

const ROWS: [&[u8]; 9] = [
    b"inf\tinf\t{{1,2}}",
    b"infinity\txinfx\t[0:1][1:2]={{1,2},{3,4}}\textra\t{x}",
    b"{}\t{}\t[1:2]=x",
    b"[1:0]={}\t\\N\t{\"{a}\"}",
    b"in\tf\t\t",
    b"plain\trow\tof\ttext",
    b"{{{{{{{\t[\t[a=b]",
    b"",
    b"inf",
];

/// **A row held whole is censused as one folded a run at a time**: every
/// row below, held by a recorder censusing alone — split only as far as the
/// census reads — and by one observing it too — split whole — answers the
/// census and count its fields fed in runs of each length to 12 answer: a
/// shape split inside its prefix or its brace run, a head outgrown, a row
/// wider than its header, a row holding no array.
#[test]
fn a_row_held_whole_is_censused_as_one_folded_in_runs() {
    let mut alone = Recorder::new(0).with_census(3, Arc::new(Infs));
    let mut observed =
        Recorder::new(0).with_census(3, Arc::new(Infs)).with_observer(Box::new(Rows::default()));
    for raw in ROWS {
        alone.row(0, raw);
        observed.row(0, raw);
    }
    let (alone, observed) = (census_of(alone), census_of(observed));
    assert_eq!(alone, observed, "a held row's census does not hang on what else reads it");
    for run in 1..=12 {
        let mut fold = CensusFold::new(3, Arc::new(Infs));
        for raw in ROWS {
            let fields: Vec<&[u8]> = split_fields(raw).collect();
            for (column, field) in fields.iter().enumerate() {
                if fold.holds(column) {
                    fold.fold(column, field, true);
                    continue;
                }
                let mut pieces = field.chunks(run).peekable();
                if pieces.peek().is_none() {
                    fold.fold(column, &[], true);
                }
                while let Some(piece) = pieces.next() {
                    fold.fold(column, piece, pieces.peek().is_none());
                }
            }
            fold.end_row(fields.len());
        }
        assert_eq!(fold.take(), alone, "runs of {run}");
        assert_eq!(fold.take(), BlockCensus::new(3), "a take leaves nothing folded");
    }
}

/// **A block read in pieces records what it would have read whole**: its
/// rows dealt out to pieces in file order and absorbed back answer the
/// census and the rows, each placed against the block's first data byte,
/// that one recorder handed every row does.
#[test]
fn pieces_absorbed_in_file_order_record_the_whole_block() {
    let data_offset = 100;
    let whole = || {
        Recorder::new(data_offset)
            .with_census(3, Arc::new(Infs))
            .with_observer(Box::new(Rows::default()))
    };
    let mut serial = whole();
    let mut split = whole();
    let mut pieces = vec![split.piece(), split.piece(), split.piece()];
    let mut offset = data_offset;
    for (i, raw) in ROWS.into_iter().enumerate() {
        serial.row(offset, raw);
        let piece = i * pieces.len() / ROWS.len();
        pieces[piece].row(offset, raw);
        offset += raw.len() as u64 + 1;
    }
    for piece in pieces {
        split.absorb(piece);
    }
    let (serial_rows, split_rows) = (observed_rows(&mut serial), observed_rows(&mut split));
    assert_eq!(split_rows, serial_rows);
    assert_eq!(serial_rows.first().map(|(offset, _)| *offset), Some(0), "placed in the block");
    assert_eq!(census_of(split), census_of(serial));
}

/// The rows `recorder`'s observer was handed, taken out of it.
fn observed_rows(recorder: &mut Recorder) -> Vec<(u64, Vec<Vec<u8>>)> {
    let observer = recorder.observer.take().expect("an observer");
    let rows = observer.into_any().downcast::<Rows>().expect("these rows");
    let taken = rows.rows.clone();
    recorder.observer = Some(rows as Box<dyn BlockObserver>);
    taken
}

/// A recorded row is each field's text, its `COPY` escapes undone and a
/// NULL `None`.
#[test]
fn a_recorded_row_undoes_copy_escapes() {
    let mut recorder = Recorder::new(0).with_rows();
    recorder.row(0, b"a\\tb\t\\N\tc\\\\d");
    let record = recorder.finish(&header(), 1, 0, None).expect("nothing refuses");
    let rows = record.recorded_rows.expect("rows recorded").rows;
    assert_eq!(rows, [vec![Some("a\tb".to_owned()), None, Some("c\\d".to_owned())]]);
}
