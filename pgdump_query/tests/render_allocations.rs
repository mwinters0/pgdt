//! What rendering one row costs in heap allocations.
//!
//! `render_field_into` exists so that printing a row builds it in one buffer
//! rather than a `String` per field, and the only honest statement of what
//! that bought is a count. Counting it here rather than by inspection is what
//! makes it a fact that stays true: a renderer that quietly starts allocating
//! again — a `format!` reintroduced, an owned intermediate on a path that had
//! none — moves this number and fails, where a paragraph in a notes doc would
//! not.
//!
//! The shape is the measured control's: the sixteen columns
//! `scripts/generate_perf_data.py` writes, in its order, which is what
//! [`docs/design/measurements.md`]'s `query` figures time. Beside it sits the
//! array pair the same generator writes under `--arrays`, at both of its
//! element counts, because what an array arm costs is the count that has to
//! *not* grow with the elements.
//!
//! **This binary installs a counting global allocator**, which is why it is a
//! test file of its own rather than a case in `tests/batch.rs`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, FixedSizeBinaryArray,
    Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, ListArray, StringViewArray,
    Time64MicrosecondArray, TimestampMicrosecondArray,
};
use arrow::buffer::OffsetBuffer;
use arrow::datatypes::{DataType, Field};
use pgdump_query::{NestedPlan, render_field_into};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    // A grow counts too: it is a second trip through the allocator, and
    // removing exactly that trip is half of what pre-sizing a buffer buys.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// One row of each of the control's sixteen columns, in its column order.
fn control_row() -> Vec<ArrayRef> {
    vec![
        Arc::new(Int32Array::from(vec![1_234_567])),
        Arc::new(Int16Array::from(vec![-12_345i16])),
        Arc::new(Int64Array::from(vec![-1_234_567_890_123_456i64])),
        Arc::new(Float32Array::from(vec![-123_456.75f32])),
        Arc::new(Float64Array::from(vec![-123_456_789_012.5f64])),
        Arc::new(
            Decimal128Array::from(vec![-123_456_789_012_345_678i128])
                .with_precision_and_scale(20, 6)
                .unwrap(),
        ),
        Arc::new(Date32Array::from(vec![19_889])),
        Arc::new(Time64MicrosecondArray::from(vec![49_530_123_456i64])),
        Arc::new(TimestampMicrosecondArray::from(vec![1_718_000_000_123_456i64])),
        Arc::new(
            TimestampMicrosecondArray::from(vec![1_718_000_000_123_456i64]).with_timezone("UTC"),
        ),
        Arc::new(
            FixedSizeBinaryArray::try_from_iter(std::iter::once([0xa0u8; 16])).expect("16 bytes"),
        ),
        Arc::new(BinaryArray::from(vec![[0x5au8; 64].as_slice()])),
        Arc::new(BooleanArray::from(vec![true])),
        Arc::new(StringViewArray::from(vec!["lorem ipsum dolor sit amet"])),
        Arc::new(StringViewArray::from(vec![
            "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor",
        ])),
        Arc::new(StringViewArray::from(vec!["a\tb\nc\\d"])),
    ]
}

/// One `integer[]` column of `len` elements, and one `text[]` of `len`
/// elements every one of which `array_out` would quote. The text elements are
/// all the same width, so the scratch buffer that quoting needs is grown by
/// the first of them and reused by the rest — which is the point being
/// counted.
fn array_row(len: usize) -> (ArrayRef, ArrayRef) {
    let ints: ArrayRef = Arc::new(Int32Array::from((0..len as i32).collect::<Vec<_>>()));
    let texts: ArrayRef = Arc::new(StringViewArray::from(
        (0..len).map(|i| format!("has space,{i:03}")).collect::<Vec<_>>(),
    ));
    let wrap = |values: ArrayRef, item: DataType| -> ArrayRef {
        Arc::new(ListArray::new(
            Arc::new(Field::new("item", item, true)),
            OffsetBuffer::from_lengths([len]),
            values,
            None,
        ))
    };
    (wrap(ints, DataType::Int32), wrap(texts, DataType::Utf8View))
}

/// **One test, deliberately.** The counter is process-wide, so a second test
/// running on another thread would land in this one's window; a single test
/// function is the cheapest way to have exactly one thread allocating while
/// the flag is on. And it is a **debug** build, so what it counts is the
/// allocations the *source* asks for, not what an optimizer folds away — which
/// is the property worth pinning, since a reintroduced `format!` is a source
/// change.
///
/// **Most of the control row allocates nothing**: the three integers, all
/// four date/time columns, the boolean and the three texts write straight
/// into the caller's buffer, and so does a SQL NULL, which writes nothing at
/// all.
///
/// **What is left is three renderers that own eighteen of the twenty**:
/// `render_f32` 5, `render_f64` 7 and `render_decimal` 6. The other two are
/// `render_uuid` and `render_bytea`, one pre-sized `String` each.
#[test]
fn the_render_path_allocation_budget_per_row() {
    let columns = control_row();
    let mut line = String::with_capacity(4096);
    // Warm the buffer, and keep the first pass out of the count: an empty
    // line grows, which is a cost of a batch's first row and not of a row.
    for column in &columns {
        render_field_into(column.as_ref(), 0, &NestedPlan::Scalar, &mut line).unwrap();
    }
    let warm = line.clone();

    line.clear();
    let mut per_column = Vec::new();
    for column in &columns {
        per_column.push(counting(|| {
            render_field_into(column.as_ref(), 0, &NestedPlan::Scalar, &mut line).unwrap();
        }));
    }
    assert_eq!(line, warm, "the counted pass rendered the same text");
    assert_eq!(
        per_column,
        // id  i16 i64 f32 f64 num date time ts tstz uuid bytea bool text*3
        vec![0, 0, 0, 5, 7, 6, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0],
        "allocations per column of the control row"
    );
    assert_eq!(per_column.iter().sum::<usize>(), 20, "allocations to render one control row");

    // The buffer is what makes that a per-row number: a hundred rows cost a
    // hundred times the same five, with nothing accumulating.
    let counted = counting(|| {
        for _ in 0..100 {
            line.clear();
            for column in &columns {
                render_field_into(column.as_ref(), 0, &NestedPlan::Scalar, &mut line).unwrap();
            }
        }
    });
    assert_eq!(counted, 2_000, "allocations to render one hundred control rows");

    // A SQL NULL costs nothing: the sink writes nothing and reports `false`,
    // which is what lets the caller write its own `\N` marker.
    let null: ArrayRef = Arc::new(Int32Array::from(vec![None::<i32>]));
    let mut wrote = true;
    let counted = counting(|| {
        line.clear();
        wrote = render_field_into(null.as_ref(), 0, &NestedPlan::Scalar, &mut line).unwrap();
    });
    assert!(!wrote);
    assert!(line.is_empty());
    assert_eq!(counted, 0, "allocations to render a SQL NULL");

    // **An array column costs the same at fifty elements as at five**, which
    // is the property the direct walk exists for: the elements are written
    // where they will be read, so nothing per element is allocated. What is
    // left is `ListArray::value`'s slice, one `Arc` per array value, plus one
    // scratch `String` for a column whose elements need quoting — one for the
    // whole value, not one per element.
    let array_plan = NestedPlan::Array(Box::new(NestedPlan::Scalar));
    let mut counts = Vec::new();
    for len in [5usize, 50] {
        let (ints, texts) = array_row(len);
        for column in [&ints, &texts] {
            // Warm the scratch's growth out of the count for the same reason
            // the control's line buffer is warmed: it is not a per-row cost.
            line.clear();
            render_field_into(column.as_ref(), 0, &array_plan, &mut line).unwrap();
            let warm = line.clone();
            line.clear();
            counts.push(counting(|| {
                render_field_into(column.as_ref(), 0, &array_plan, &mut line).unwrap();
            }));
            assert_eq!(line, warm, "the counted pass rendered the same array");
        }
    }
    assert_eq!(
        counts,
        //   int[5] text[5] int[50] text[50]
        vec![1, 2, 1, 2],
        "allocations per array value, at five elements and at fifty",
    );
}

/// Run `body` with the counter on, and return how many times the allocator
/// was entered.
fn counting(body: impl FnOnce()) -> usize {
    COUNTING.store(true, Ordering::Relaxed);
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    body();
    let counted = ALLOCATIONS.load(Ordering::Relaxed) - before;
    COUNTING.store(false, Ordering::Relaxed);
    counted
}
