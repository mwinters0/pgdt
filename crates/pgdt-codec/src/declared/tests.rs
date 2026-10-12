use super::*;

fn ty(name: &str, kind: TypeKind) -> TypeDef {
    TypeDef { name: name.to_string(), kind }
}

/// **A domain's `NOT NULL` binds every domain over it, and no array of
/// it** (I76), found in any spelling of its name; a cycle ends.
#[test]
fn a_domain_not_null_binds_the_domains_over_it() {
    let not_null = |base: &str| TypeKind::Domain {
        base_type: base.into(),
        collation: None,
        not_null: true,
        check: false,
    };
    let types = [
        ty("public.nn", not_null("integer")),
        ty("public.over", TypeKind::domain("PUBLIC.nn")),
        ty("public.plain", TypeKind::domain("integer")),
        ty("public.a", TypeKind::domain("public.b")),
        ty("public.b", TypeKind::domain("public.a")),
    ];
    assert!(domain_not_null("public.nn", &types));
    assert!(domain_not_null("public.over", &types));
    assert!(domain_not_null(r#""public"."over""#, &types));
    assert!(!domain_not_null("public.nn[]", &types));
    assert!(!domain_not_null("public.plain", &types));
    assert!(!domain_not_null("integer", &types));
    assert!(!domain_not_null("public.a", &types));
}
