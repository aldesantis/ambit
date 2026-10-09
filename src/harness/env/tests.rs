use indexmap::IndexMap;

use super::*;

fn map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
    pairs
        .iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|&name| name.to_owned()).collect()
}

fn keys(env: Option<IndexMap<String, String>>) -> Vec<String> {
    env.unwrap_or_default().into_keys().collect()
}

#[test]
fn spells_one_variable_the_way_each_family_of_harnesses_does() {
    assert_eq!(shell_ref("TOKEN"), "${TOKEN}");
    assert_eq!(namespaced_ref("TOKEN"), "${env:TOKEN}");
    assert_eq!(braced_ref("TOKEN"), "{env:TOKEN}");
}

#[test]
fn rewrites_every_reference_in_a_larger_string_leaving_the_rest_of_it_alone() {
    assert_eq!(
        translate_refs("Bearer ${TOKEN}", namespaced_ref),
        "Bearer ${env:TOKEN}"
    );
    assert_eq!(
        translate_refs("${USER}:${PASSWORD}@host", braced_ref),
        "{env:USER}:{env:PASSWORD}@host"
    );
}

#[test]
fn is_a_no_op_for_a_harness_whose_syntax_already_is_the_catalogs() {
    assert_eq!(
        translate_refs("Bearer ${TOKEN}", shell_ref),
        "Bearer ${TOKEN}"
    );
}

#[test]
fn never_resolves_a_variable_whatever_the_environment_holds() {
    assert_eq!(
        translate_refs("Bearer ${AMBIT_TEST_TOKEN}", shell_ref),
        "Bearer ${AMBIT_TEST_TOKEN}"
    );
}

#[test]
fn leaves_a_placeholder_that_is_not_a_variable_name_where_it_found_it() {
    for value in ["${1}", "${a-b}", "${ TOKEN }", "${}", "${TOKEN", "$TOKEN"] {
        assert_eq!(translate_refs(value, namespaced_ref), value);
    }
}

#[test]
fn passes_a_value_holding_no_reference_through_unchanged() {
    assert_eq!(
        translate_refs("https://mcp.invalid/fixture", braced_ref),
        "https://mcp.invalid/fixture"
    );
}

#[test]
fn lists_them_in_first_appearance_order_repeats_included() {
    assert_eq!(referenced_names("${B} ${A} ${B}"), ["B", "A", "B"]);
}

#[test]
fn lists_none_for_a_value_with_no_reference() {
    assert_eq!(referenced_names("Bearer token"), Vec::<String>::new());
    assert_eq!(referenced_names("${ TOKEN }"), Vec::<String>::new());
}

#[test]
fn names_the_variable() {
    assert_eq!(sole_reference("${TOKEN}").as_deref(), Some("TOKEN"));
    assert_eq!(
        sole_reference("${_private_1}").as_deref(),
        Some("_private_1")
    );
}

#[test]
fn names_nothing_when_the_reference_is_only_part_of_the_value() {
    assert_eq!(sole_reference("Bearer ${TOKEN}"), None);
    assert_eq!(sole_reference("${TOKEN} "), None);
    assert_eq!(sole_reference("${A}${B}"), None);
    assert_eq!(sole_reference("token"), None);
}

#[test]
fn maps_every_expected_name_to_a_reference_the_harness_expands() {
    assert_eq!(
        stdio_env(&names(&["TOKEN", "API_KEY"]), &IndexMap::new(), shell_ref),
        Some(map(&[("API_KEY", "${API_KEY}"), ("TOKEN", "${TOKEN}")]))
    );
}

#[test]
fn sorts_by_name_so_the_file_does_not_churn_when_the_catalog_reorders_its_list() {
    assert_eq!(
        keys(stdio_env(
            &names(&["TOKEN", "API_KEY", "BASE_URL"]),
            &IndexMap::new(),
            shell_ref
        )),
        ["API_KEY", "BASE_URL", "TOKEN"]
    );
}

#[test]
fn does_not_mutate_the_list_it_was_handed() {
    let expected = names(&["TOKEN", "API_KEY"]);

    stdio_env(&expected, &IndexMap::new(), shell_ref);

    assert_eq!(expected, ["TOKEN", "API_KEY"]);
}

#[test]
fn returns_nothing_for_an_empty_list_so_the_caller_can_omit_the_key_entirely() {
    assert_eq!(stdio_env(&[], &IndexMap::new(), shell_ref), None);
}

#[test]
fn uses_the_harnesss_own_spelling() {
    assert_eq!(
        stdio_env(&names(&["TOKEN"]), &IndexMap::new(), namespaced_ref),
        Some(map(&[("TOKEN", "${env:TOKEN}")]))
    );
    assert_eq!(
        stdio_env(&names(&["TOKEN"]), &IndexMap::new(), braced_ref),
        Some(map(&[("TOKEN", "{env:TOKEN}")]))
    );
}

#[test]
fn gives_the_process_the_name_it_reads_from_the_variable_that_supplies_it() {
    assert_eq!(
        stdio_env(
            &names(&["ACME_PLANNER_TOKEN"]),
            &map(&[("PLANNER_TOKEN", "${ACME_PLANNER_TOKEN}")]),
            shell_ref
        ),
        Some(map(&[("PLANNER_TOKEN", "${ACME_PLANNER_TOKEN}")]))
    );
}

#[test]
fn still_passes_through_an_expected_variable_no_entry_references() {
    assert_eq!(
        stdio_env(
            &names(&["ACME_PLANNER_TOKEN", "PLANNER_WORKSPACE"]),
            &map(&[("PLANNER_TOKEN", "${ACME_PLANNER_TOKEN}")]),
            shell_ref
        ),
        Some(map(&[
            ("PLANNER_TOKEN", "${ACME_PLANNER_TOKEN}"),
            ("PLANNER_WORKSPACE", "${PLANNER_WORKSPACE}"),
        ]))
    );
}

#[test]
fn lets_an_entry_override_the_reference_an_expected_name_would_have_got() {
    assert_eq!(
        stdio_env(
            &names(&["TOKEN"]),
            &map(&[("TOKEN", "${ACME_TOKEN}")]),
            shell_ref
        ),
        Some(map(&[("TOKEN", "${ACME_TOKEN}")]))
    );
}

#[test]
fn translates_the_value_like_any_other_embedded_reference_and_all() {
    assert_eq!(
        stdio_env(
            &[],
            &map(&[("PLANNER_AUTH", "Bearer ${ACME_PLANNER_TOKEN}")]),
            namespaced_ref
        ),
        Some(map(&[("PLANNER_AUTH", "Bearer ${env:ACME_PLANNER_TOKEN}")]))
    );
    assert_eq!(
        stdio_env(
            &[],
            &map(&[("PLANNER_URL", "https://planner.invalid")]),
            braced_ref
        ),
        Some(map(&[("PLANNER_URL", "https://planner.invalid")]))
    );
}

#[test]
fn sorts_the_two_sources_together_so_a_renamed_key_is_not_written_last() {
    assert_eq!(
        keys(stdio_env(
            &names(&["PLANNER_WORKSPACE", "ACME_TOKEN"]),
            &map(&[("PLANNER_TOKEN", "${ACME_TOKEN}")]),
            shell_ref
        )),
        ["PLANNER_TOKEN", "PLANNER_WORKSPACE"]
    );
}
