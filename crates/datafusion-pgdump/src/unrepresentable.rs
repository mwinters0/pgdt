//! `pgdump_unrepresentable(<column>)`: the library's `IS UNREPRESENTABLE`
//! spelled as a DataFusion scalar function, and the guard that refuses it at
//! planning where only a scan can answer it.
//!
//! **Only a scan can answer it**: under the null mode an unrepresentable
//! value reaches DataFusion as a NULL, which no longer says where it came
//! from, so the function is answered on the field's text, by the library, as
//! a filter pushed `Exact` into a pgdump scan ([`crate::pushdown`]). Wherever
//! DataFusion would have to evaluate it instead — projected, under an
//! expression no scan answers, over another source — evaluating it refuses,
//! and a session its embedder built [`with_guard`] refuses the plan before it
//! runs (`docs/design/decisions.md`, "D101").

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
use datafusion::common::{Result, exec_err, plan_datafusion_err};
use datafusion::config::ConfigOptions;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::logical_expr::{
    ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    Volatility,
};
use datafusion::physical_expr::ScalarFunctionExpr;
use datafusion::physical_optimizer::PhysicalOptimizerRule;
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

    /// Reached only in a session built without [`with_guard`], where
    /// DataFusion holds the value, not its text. Sound though it refuses at
    /// execution: DataFusion skips it only where the answer does not depend
    /// on it — an `AND`'s or an `OR`'s short circuit, an untaken `CASE`
    /// branch, no rows.
    fn invoke_with_args(&self, _: ScalarFunctionArgs) -> Result<ColumnarValue> {
        exec_err!(
            "{}; a session built with `datafusion_pgdump::with_unrepresentable_guard` refuses \
             such a query at planning, before a row is read",
            evaluated_by_datafusion(&format!("{UNREPRESENTABLE_FUNCTION}(…)"))
        )
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

/// **Register the function in `ctx`**, leaving the session's planning as its
/// embedder built it: a use DataFusion would evaluate refuses when it is
/// evaluated, unless the session was built [`with_guard`]. [`crate::register_dump`]
/// and [`crate::register_table_factory`] call it, and an embedder registering
/// a [`crate::PgDumpTable`] by hand calls it itself.
pub fn register(ctx: &SessionContext) {
    ctx.register_udf(ScalarUDF::new_from_impl(PgDumpUnrepresentable::default()));
}

/// **`builder` with the function and its guard**: a physical optimizer rule,
/// appended after every other, refusing at planning a plan in which any node
/// holds the function among the expressions DataFusion evaluates — a scan's
/// pushed filters being the library's, no node but a scan answering it
/// (`docs/design/runtime-invariants.md`, "RT22"). Offered on a builder alone,
/// so nothing rebuilds a session its embedder owns: one holding a live
/// [`SessionContext`] rebuilds it through
/// [`SessionStateBuilder::new_from_existing`], which keeps no prepared
/// statement.
pub fn with_guard(mut builder: SessionStateBuilder) -> SessionStateBuilder {
    builder
        .scalar_functions()
        .get_or_insert_with(Vec::new)
        .push(Arc::new(ScalarUDF::new_from_impl(PgDumpUnrepresentable::default())));
    builder.with_physical_optimizer_rule(Arc::new(Guard))
}

/// The rule [`with_guard`] appends.
#[derive(Debug)]
struct Guard;

impl PhysicalOptimizerRule for Guard {
    fn optimize(
        &self,
        plan: Arc<dyn ExecutionPlan>,
        _: &ConfigOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        refuse_evaluation(&plan)?;
        Ok(plan)
    }

    fn name(&self) -> &str {
        "pgdump_unrepresentable_guard"
    }

    fn schema_check(&self) -> bool {
        true
    }
}

/// Refuse `plan` wherever a node holds the function among the expressions
/// DataFusion evaluates, a scalar subquery's plan included, it being a
/// child: a pgdump scan reports only its dynamic filters, the filters its
/// provider answered being the library's. A function a plan's statistics
/// answered is gone by then, and runs unevaluated: over an empty table, a
/// subquery's `MAX(x) … WHERE pgdump_unrepresentable(x)` plans as `NULL`,
/// which is why the refusal test's table holds a row.
fn refuse_evaluation(plan: &Arc<dyn ExecutionPlan>) -> Result<()> {
    let mut found = None;
    plan.apply(|node| {
        node.apply_expressions(&mut |expr| {
            expr.apply(|inner| {
                if let Some(call) = inner.downcast_ref::<ScalarFunctionExpr>()
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
