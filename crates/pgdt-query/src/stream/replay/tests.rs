use std::sync::Arc;

use pgdt_core::{CopyHeader, Error};
use pgdt_dump::cache::{CacheMode, SourceWatch, StrictIdentity};
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_model::CopyBlock;

use super::activate;
use super::plan::{ReplayPlan, resolve_for_query};
use crate::options::{QueryOptions, ScanExtent};
use crate::predicate::{Expr, PredicateOp};
use crate::stream::mapping::map_for_query;
use crate::stream::tests::two_blocks;

/// **The plan resolves every block before any is read, and activation
/// takes that resolution rather than making its own.** A block's filter
/// or projection refusal refuses the plan.
#[tokio::test]
async fn the_filter_is_resolved_once_per_block_at_plan_time() {
    use crate::predicate::Predicate;
    use pgdt_io::LocalFileSource;

    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
    let not_null = |column: &str| {
        Expr::all([Predicate { column: column.into(), op: PredicateOp::IsNotNull, value: None }])
    };
    let query_options = QueryOptions {
        filter: not_null("a"),
        projection: Some(vec!["a".into()]),
        // Past the first block, which would otherwise settle the table.
        scan_extent: ScanExtent::Full,
        ..QueryOptions::default()
    };
    let mapped = map_for_query(
        &source,
        "public.t",
        &ScanOptions::default(),
        &query_options,
        &CacheMode::DISABLED,
        &SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap(),
    )
    .await
    .unwrap();
    let [first, second] = &mapped.matches[..] else { panic!("two blocks: {:?}", mapped.matches) };
    let plan_for = |query_options: &QueryOptions| {
        ReplayPlan::new(
            ReadOptions::default(),
            query_options.clone(),
            mapped.database.clone(),
            &mapped.matches,
            mapped.metadata.clone(),
        )
    };

    // A filter no block can answer refuses the plan, naming the first.
    let refusing = QueryOptions { filter: not_null("c"), ..query_options.clone() };
    assert!(matches!(
        plan_for(&refusing).map(|_| ()),
        Err(Error::UnknownPredicateColumn { header_offset, .. })
            if header_offset == first.header_offset
    ));

    // So does a projection naming a column no block carries.
    let refusing = QueryOptions { projection: Some(vec!["c".into()]), ..query_options.clone() };
    assert!(matches!(
        plan_for(&refusing).map(|_| ()),
        Err(Error::UnknownProjectionColumn { header_offset, .. })
            if header_offset == first.header_offset
    ));

    let plan = plan_for(&query_options).unwrap();
    assert_eq!(
        plan.blocks.keys().copied().collect::<Vec<_>>(),
        [first.header_offset, second.header_offset]
    );

    // The planned entry is what resolving the block where it is reached
    // would have produced.
    let live = |block: &CopyBlock| {
        resolve_for_query(
            &block.header,
            block.header_offset,
            mapped.database.as_deref(),
            &query_options,
            mapped.metadata.as_ref(),
            &plan.table,
            block.unrepresentable(),
        )
    };
    let planned = &plan.blocks[&first.header_offset];
    assert_eq!(format!("{planned:?}"), format!("{:?}", live(first).unwrap()));

    // The second block's `a` is its second field, and it is `a` that
    // feeds the projection.
    assert_eq!(plan.blocks[&second.header_offset].field_targets, [None, Some(0)]);

    // Activating the planned block hands out the plan's own tree.
    let open = |header: &CopyHeader, block: &CopyBlock| {
        activate(header.clone(), block.header_offset, mapped.database.clone(), &plan)
    };
    let ((_, _, _, filter, _), resolved, notes) = open(&first.header, first).unwrap();
    assert!(Arc::ptr_eq(&filter, &planned.filter));
    assert_eq!((resolved, notes), (planned.resolved.clone(), planned.notes.clone()));

    // An activation the entry does not describe resolves for itself.
    let ((_, _, _, other, _), _, _) = open(&second.header, first).unwrap_or_else(|e| panic!("{e}"));
    assert!(!Arc::ptr_eq(&other, &planned.filter));
}
