//! **A term's key factor is its largest normalized position's**, and none
//! where no position is normalized.

use pgdt_model::{ColumnDef, TypeDef, TypeKind};

use super::*;
use crate::compare::{ComparisonPlan, comparison_for};

fn factor(declared: &str) -> Option<KeyFactor> {
    let types = [TypeDef {
        name: "public.both".into(),
        kind: TypeKind::Composite {
            fields: Some(vec![ColumnDef::new("a", "jsonb"), ColumnDef::new("b", "int4multirange")]),
        },
    }];
    match comparison_for(declared, None, &types, &[]) {
        ComparisonPlan::Nested(plan) => nested_key_factor(&plan),
        ComparisonPlan::Compared { kind, .. } => key_factor(&kind),
        other => panic!("{declared}: {other:?}"),
    }
}

#[test]
fn a_term_s_key_factor_is_its_largest_normalized_position_s() {
    let jsonb = KeyFactor { held: 4, per: 5 };
    let multirange = KeyFactor { held: 8, per: 4 };
    assert_eq!(factor("jsonb"), Some(jsonb));
    assert_eq!(factor("jsonb[]"), Some(jsonb));
    assert_eq!(factor("int4multirange"), Some(multirange));
    assert_eq!(factor("int4multirange[]"), Some(multirange));
    assert_eq!(factor("public.both"), Some(multirange));
    for none in ["text", "integer[]", "int4range", "bytea"] {
        assert_eq!(factor(none), None, "{none}");
    }
    assert_eq!(jsonb.of(5), 4);
    assert_eq!(jsonb.of(6), 5);
    assert_eq!(multirange.of(u64::MAX), u64::MAX);
}
