use super::*;

/// Walk one `List` level, appending its lengths to `dims` and its leaves
/// to `elements` in row-major order — the flattened shape
/// [`pgdt_codec::ArrayLiteral`] wants.
fn collect_array(
    column: &dyn Array,
    row: usize,
    child_plan: &NestedPlan,
    depth: usize,
    dims: &mut Vec<usize>,
    elements: &mut Vec<Option<std::borrow::Cow<'static, str>>>,
) -> Result<()> {
    let values = column.as_any().downcast_ref::<ListArray>().unwrap().value(row);
    if dims.len() == depth {
        dims.push(values.len());
    }
    for i in 0..values.len() {
        match child_plan {
            NestedPlan::Array(inner) => {
                collect_array(values.as_ref(), i, &inner.plan, depth + 1, dims, elements)?;
            }
            _ => elements
                .push(render_field(values.as_ref(), i, child_plan)?.map(std::borrow::Cow::Owned)),
        }
    }
    Ok(())
}

/// Rebuild an `array_out` literal from a `List` value.
pub(super) fn render_array(
    column: &dyn Array,
    row: usize,
    child_plan: &NestedPlan,
) -> Result<String> {
    let mut dims = Vec::new();
    let mut elements = Vec::new();
    collect_array(column, row, child_plan, 0, &mut dims, &mut elements)?;
    if elements.is_empty() {
        return Ok("{}".to_string());
    }
    let lower_bounds = vec![1; dims.len()];
    Ok(pgdt_codec::render_array(&pgdt_codec::ArrayLiteral { elements, dims, lower_bounds }))
}
