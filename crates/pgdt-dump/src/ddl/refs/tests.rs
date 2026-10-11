use std::collections::BTreeSet;

use super::*;

/// [`extract_statement_cross_refs`]'s five recognized shapes, in lines
/// shaped as `fixtures/16/objects/default.sql`'s.
fn refs_of(stmt: &str) -> (Vec<String>, Vec<String>) {
    let mut roles = BTreeSet::new();
    let mut tablespaces = BTreeSet::new();
    extract_statement_cross_refs(stmt, &mut roles, &mut tablespaces);
    (roles.into_iter().collect(), tablespaces.into_iter().collect())
}

#[test]
fn owner_to_is_recognized_across_every_ownable_object_kind() {
    assert_eq!(
        refs_of("ALTER FUNCTION objects.rgb_to_cmyk(objects.color_rgb) OWNER TO postgres;").0,
        vec!["postgres".to_string()]
    );
    assert_eq!(refs_of("ALTER LARGE OBJECT 16490 OWNER TO postgres;").0, vec!["postgres"]);
}

#[test]
fn a_grant_records_its_grantee_but_not_public() {
    assert_eq!(
        refs_of("GRANT SELECT ON TABLE objects.events TO fixture_reader;").0,
        vec!["fixture_reader".to_string()]
    );
    assert!(refs_of("GRANT SELECT ON TABLE objects.widgets TO PUBLIC;").0.is_empty());
}

#[test]
fn a_revoke_records_its_grantee_from_from() {
    assert_eq!(
        refs_of("REVOKE ALL ON TABLE objects.events FROM fixture_reader;").0,
        vec!["fixture_reader".to_string()]
    );
}

/// `ALTER DEFAULT PRIVILEGES` is one statement carrying two role
/// references — the role the default applies to, and the grant's own
/// grantee — both recorded from the single combined line
/// `buildDefaultACLCommands` writes.
#[test]
fn alter_default_privileges_records_both_its_for_role_and_its_grantee() {
    let (roles, _) = refs_of(
        "ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA objects GRANT SELECT ON TABLES \
             TO fixture_reader;",
    );
    assert_eq!(roles, vec!["fixture_reader".to_string(), "postgres".to_string()]);
}

#[test]
fn set_default_tablespace_records_a_real_tablespace_but_not_a_reset() {
    assert_eq!(refs_of("SET default_tablespace = fastspace;").1, vec!["fastspace".to_string()]);
    assert!(refs_of("SET default_tablespace = '';").1.is_empty());
}

#[test]
fn pg_default_is_never_recorded_even_if_named_explicitly() {
    assert!(refs_of("SET default_tablespace = pg_default;").1.is_empty());
}

/// `fmtId` quotes a name holding a capital, so a role `PUBLIC` and a
/// tablespace `PG_DEFAULT` are written quoted and are kept; `public`,
/// quoted or not, is the pseudo-role to the server (I85), and is not.
#[test]
fn a_quoted_public_role_and_pg_default_tablespace_are_kept() {
    assert_eq!(
        refs_of("GRANT SELECT ON TABLE objects.widgets TO \"PUBLIC\";").0,
        vec!["PUBLIC".to_string()]
    );
    assert_eq!(refs_of("ALTER TABLE objects.t OWNER TO \"PUBLIC\";").0, vec!["PUBLIC"]);
    assert!(refs_of("REVOKE ALL ON TABLE objects.t FROM \"public\";").0.is_empty());
    assert_eq!(
        refs_of("SET default_tablespace = \"PG_DEFAULT\";").1,
        vec!["PG_DEFAULT".to_string()]
    );
}

/// `fmtId()`'s quoted form (a role/tablespace name that needs quoting,
/// with `""` doubling any literal `"`) is parsed the same way an
/// unquoted one is.
#[test]
fn a_quoted_role_name_is_unquoted() {
    assert_eq!(
        refs_of("ALTER SCHEMA public OWNER TO \"has a \"\"quote\"\" and space\";").0,
        vec!["has a \"quote\" and space".to_string()]
    );
}

#[test]
fn a_plain_ddl_statement_records_nothing() {
    let (roles, tablespaces) = refs_of("CREATE TABLE public.t (id integer);");
    assert!(roles.is_empty());
    assert!(tablespaces.is_empty());
}
