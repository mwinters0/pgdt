//! What a session needs before a dump's table is planned in it.

use std::sync::Arc;

use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::{SessionConfig, SessionContext};

use crate::budget::{ScanBudget, session_budget};
use crate::settings::PgDumpSettings;
use crate::unrepresentable::{PgDumpUnrepresentable, with_guard};

/// **`builder`, readied for every pgdump table planned in the session it
/// builds**: the [`PgDumpSettings`] `SET` moves, at their defaults unless its
/// config carries them; a [`ScanBudget`] discovered from the process's
/// allowance unless its config carries one, which every scan of the session
/// draws on; `pgdump_unrepresentable` and the physical optimizer rule refusing
/// at planning a plan in which DataFusion would evaluate it
/// (`docs/design/decisions.md`, "D101"); and
/// `datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown` off,
/// DataFusion 55.1's ungrouped-aggregate filter losing a column's bounds, so
/// that a scan pruning under what is left answers a `MIN` or `MAX` wrongly
/// (`KD56`).
///
/// **Called last before [`SessionStateBuilder::build`]**, after the config:
/// a config set after it replaces what it readied. The aggregate's filter is
/// turned off whatever the config already says, DataFusion 55.1's config
/// keeping a value and not whether it was stated (`docs/status/upstream.md`,
/// "UF1"); a caller wanting it on all the same sets it after this call, as
/// `pgdt sql` does where its environment states the option.
///
/// Offered on a builder alone, so nothing rebuilds a session its embedder
/// owns: one holding a live [`SessionContext`] rebuilds it through
/// [`SessionStateBuilder::new_from_existing`], which keeps no prepared
/// statement. A session never readied still plans a registered dump's tables:
/// [`crate::register_dump`] and [`crate::register_table_factory`] give it the
/// settings, the budget and the function, the function then refused only when
/// DataFusion evaluates it, and the aggregate's filter left as the session has
/// it.
pub fn ready_session(builder: SessionStateBuilder) -> SessionStateBuilder {
    let mut builder = with_guard(builder);
    let config = builder.config().get_or_insert_with(SessionConfig::new);
    let options = config.options_mut();
    if options.extensions.get::<PgDumpSettings>().is_none() {
        options.extensions.insert(PgDumpSettings::default());
    }
    // upstream: UF1
    options.optimizer.enable_aggregate_dynamic_filter_pushdown = false;
    if config.get_extension::<ScanBudget>().is_none() {
        config.set_extension(Arc::new(ScanBudget::discover()));
    }
    builder
}

/// **Give `ctx` what every pgdump table planned in it reads**, returning the
/// session's budget: the settings and the budget [`ready_session`] gives a
/// builder, each unless the session carries one, and `pgdump_unrepresentable`,
/// the session's planning left as its embedder built it. Registering a dump
/// calls it, so a session never readied still plans its tables.
pub(crate) fn install(ctx: &SessionContext) -> Arc<ScanBudget> {
    PgDumpSettings::install(ctx);
    ctx.register_udf(ScalarUDF::new_from_impl(PgDumpUnrepresentable::default()));
    session_budget(ctx)
}
