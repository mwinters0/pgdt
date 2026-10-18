use pgdt_model::TableName;

use super::TableSelector;

fn name(database: Option<&str>, schema: Option<&str>, table: &str) -> TableName {
    TableName {
        database: database.map(str::to_owned),
        schema: schema.map(str::to_owned),
        table: table.to_owned(),
    }
}

/// A part left unstated selects whatever the table holds there; a stated one,
/// only what it states — `Some(None)` included, which a [`TableName`] states
/// for a name no schema qualifies or a database no `\connect` names.
#[test]
fn a_selector_matches_the_parts_it_states() {
    let qualified = name(Some("app"), Some("public"), "t");
    let bare = name(None, None, "t");
    let other = name(Some("app"), Some("public"), "u");

    let any = TableSelector::named("t");
    assert!(any.selects(&qualified) && any.selects(&bare) && !any.selects(&other));

    let in_schema = TableSelector::named("t").in_schema("public");
    assert!(in_schema.selects(&qualified) && !in_schema.selects(&bare));

    let in_database = TableSelector::named("t").in_database("app");
    assert!(in_database.selects(&qualified) && !in_database.selects(&bare));

    let exact = TableSelector::from(&bare);
    assert!(exact.selects(&bare) && !exact.selects(&qualified));
    let exact = TableSelector::from(qualified.clone());
    assert!(exact.selects(&qualified) && !exact.selects(&bare));
}

/// The name a refusal quotes: the schema where the selector states one.
#[test]
fn a_selector_displays_the_name_it_was_given() {
    assert_eq!(TableSelector::named("t").to_string(), "t");
    assert_eq!(TableSelector::named("t").in_schema("s").in_database("d").to_string(), "s.t");
    assert_eq!(TableSelector::from(name(Some("d"), None, "t")).to_string(), "t");
}
