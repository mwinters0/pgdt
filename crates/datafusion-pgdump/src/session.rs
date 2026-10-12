//! What a session needs before a dump's table is planned in it.

use std::sync::Arc;

use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::SessionContext;

use crate::budget::{ScanBudget, session_budget};
use crate::settings::PgDumpSettings;
use crate::unrepresentable::PgDumpUnrepresentable;

/// **Give `ctx` what every pgdump table planned in it reads**, returning the
/// session's budget: the [`PgDumpSettings`] `SET` moves, at their defaults
/// unless it carries them; a [`ScanBudget`] discovered from the process's
/// allowance unless it carries one, which every scan of the session draws on;
/// and `pgdump_unrepresentable` ([`PgDumpUnrepresentable`]), the session's
/// planning left as its embedder built it — a use DataFusion would evaluate
/// refused when it is evaluated, or at planning in a session built
/// [`with_unrepresentable_guard`](crate::with_unrepresentable_guard).
///
/// [`crate::register_dump`] and [`crate::register_table_factory`] call it;
/// an embedder registering a [`crate::PgDumpTable`] by hand calls it, without
/// which `SET pgdump.…` is refused, the function is unknown, and the table's
/// scans draw on a budget every such session shares.
pub fn install(ctx: &SessionContext) -> Arc<ScanBudget> {
    PgDumpSettings::install(ctx);
    ctx.register_udf(ScalarUDF::new_from_impl(PgDumpUnrepresentable::default()));
    session_budget(ctx)
}
