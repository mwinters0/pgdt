//! `pgdump_unrepresentable(<column>)`: the library's `IS UNREPRESENTABLE`
//! spelled as a DataFusion scalar function, and the planner that keeps it
//! where only a scan can answer it.
//!
//! **Only a scan can answer it**: under the null mode an unrepresentable
//! value reaches DataFusion as a NULL, which no longer says where it came
//! from, so the function is answered on the field's text, by the library, as
//! a filter pushed `Exact` into a pgdump scan ([`crate::pushdown`]). Wherever
//! DataFusion would have to evaluate it instead — projected, under an
//! expression no scan answers, over another source — the plan is refused
//! before it runs ([`install`]; `docs/design/decisions.md`, "D101").

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, FieldRef};
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
use datafusion::common::{Result, plan_datafusion_err, plan_err};
use datafusion::execution::context::QueryPlanner;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::logical_expr::{
    ColumnarValue, Expr, LogicalPlan, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF,
    ScalarUDFImpl, Signature, Volatility,
};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::SessionContext;

/// The function's name, as SQL calls it.
pub const UNREPRESENTABLE_FUNCTION: &str = "pgdump_unrepresentable";

/// **`pgdump_unrepresentable(<column>)`**: whether the column's value is one
/// PostgreSQL accepts for its declared type and DataFusion cannot hold — past
/// Arrow's format spec, or past the calendar `arrow-cast` displays through —
/// exactly the values the null mode reads as NULL, so it tells those from the
/// NULLs the dump holds. Two-valued, `NOT` above it its negation, and
/// answered in every mode but by a scan alone; refused under the strings
/// schema mode, which reads no declared type.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct PgDumpUnrepresentable {
    signature: Signature,
}

impl Default for PgDumpUnrepresentable {
    fn default() -> Self {
        Self { signature: Signature::any(1, Volatility::Immutable) }
    }
}

impl ScalarUDFImpl for PgDumpUnrepresentable {
    fn name(&self) -> &str {
        UNREPRESENTABLE_FUNCTION
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    /// Never NULL: a NULL field is not an unrepresentable value.
    fn return_field_from_args(&self, _: ReturnFieldArgs) -> Result<FieldRef> {
        Ok(Arc::new(Field::new(self.name(), DataType::Boolean, false)))
    }

    /// A NULL argument answers `false`, not NULL.
    fn is_strict(&self) -> bool {
        false
    }

    /// Reached only where the planner's check was not installed, or by a
    /// literal argument folded while the plan is optimized: either way
    /// DataFusion holds the value, not its text.
    fn invoke_with_args(&self, _: ScalarFunctionArgs) -> Result<ColumnarValue> {
        plan_err!("{}", evaluated_by_datafusion(&format!("{UNREPRESENTABLE_FUNCTION}(…)")))
    }
}

/// The refusal for `call`, a use of the function DataFusion would evaluate.
fn evaluated_by_datafusion(call: &str) -> String {
    format!(
        "`{call}` is answered on a dump's text by a pgdump table's scan, as a filter on one of \
         its columns, and here DataFusion would have to evaluate it over a value it holds, where \
         a NULL no longer says whether it was one — use it only in a WHERE clause over a pgdump \
         table's column, alone, under NOT, AND and OR, or beside terms the scan answers"
    )
}

/// **Register the function in `ctx`, and the planner check that refuses a
/// plan in which DataFusion would evaluate it**: wrapped around the session's
/// own planner, the optimized plan is walked before it is planned
/// physically, and every use of the function outside a scan's pushed filters
/// refuses it (`docs/design/runtime-invariants.md`, "RT22"). Once per session; [`crate::register_dump`] and
/// [`crate::register_table_factory`] call it, and an embedder registering a
/// [`crate::PgDumpTable`] by hand calls it itself.
pub fn install(ctx: &SessionContext) {
    let state = ctx.state_ref();
    let mut state = state.write();
    if state.config().get_extension::<Installed>().is_some() {
        return;
    }
    let planner = Arc::new(Guarded(Arc::clone(state.query_planner())));
    let session_id = state.session_id().to_string();
    let mut built = SessionStateBuilder::new_from_existing(state.clone())
        .with_session_id(session_id)
        .with_query_planner(planner)
        .build();
    built.config_mut().set_extension(Arc::new(Installed));
    *state = built;
    drop(state);
    ctx.register_udf(ScalarUDF::new_from_impl(PgDumpUnrepresentable::default()));
}

/// The mark [`install`] leaves on a session it has installed into.
#[derive(Debug)]
struct Installed;

/// The session's own planner, behind [`refuse_evaluation`].
#[derive(Debug)]
struct Guarded(Arc<dyn QueryPlanner + Send + Sync>);

#[async_trait]
impl QueryPlanner for Guarded {
    async fn create_physical_plan(
        &self,
        logical_plan: &LogicalPlan,
        session: &dyn Session,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        refuse_evaluation(logical_plan)?;
        self.0.create_physical_plan(logical_plan, session).await
    }
}

/// Refuse `plan`, optimized, wherever it holds the function outside a scan's
/// filters, subqueries included: a scan holds only the filters its provider
/// answered, and every other node's expressions are DataFusion's to evaluate.
fn refuse_evaluation(plan: &LogicalPlan) -> Result<()> {
    let mut found = None;
    plan.apply_with_subqueries(|node| {
        if matches!(node, LogicalPlan::TableScan(_)) {
            return Ok(TreeNodeRecursion::Continue);
        }
        node.apply_expressions(|expr| {
            expr.apply(|inner| {
                if let Expr::ScalarFunction(call) = inner
                    && call.name() == UNREPRESENTABLE_FUNCTION
                {
                    found = Some(inner.to_string());
                    return Ok(TreeNodeRecursion::Stop);
                }
                Ok(TreeNodeRecursion::Continue)
            })
        })
    })?;
    match found {
        Some(call) => Err(plan_datafusion_err!("{}", evaluated_by_datafusion(&call))),
        None => Ok(()),
    }
}
