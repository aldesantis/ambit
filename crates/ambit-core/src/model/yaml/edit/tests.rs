//! The splice primitives, on documents of no particular format.

use super::*;
use crate::errors::ExitCode;

fn editor(text: &str) -> YamlEditor {
    YamlEditor::parse(text, "t.yml").unwrap()
}

fn lines(text: &str) -> Vec<String> {
    vec![text.to_owned()]
}

/// Every item of the root list `key`, kept, followed by `added`.
fn keep_all(editor: &YamlEditor, key: &str) -> Vec<PlannedItem> {
    let seq = editor.root_value(key).unwrap();

    editor
        .items(seq)
        .into_iter()
        .map(|item| PlannedItem::keep(item, lines("unused")))
        .collect()
}

#[test]
fn an_unchanged_plan_touches_nothing_even_under_an_anchor() {
    let text = "a: &x\n  - 1 # one\n  - 2\n";
    let mut editor = editor(text);
    let plan = keep_all(&editor, "a");

    editor.update_sequence("a", &plan).unwrap();

    assert_eq!(editor.finish().unwrap(), text);
}

#[test]
fn a_block_scalar_is_replaced_with_its_item() {
    let text = "a:\n  - k: |\n      long\n    j: 1\n";
    let mut editor = editor(text);
    let item = editor.items(editor.root_value("a").unwrap())[0];
    let value = editor.value(item, "k").unwrap();
    let plan = [PlannedItem::patch(
        item,
        vec![Patch::Scalar {
            node: value,
            value: "short".to_owned(),
        }],
        vec!["k: short".to_owned(), "j: 1".to_owned()],
    )];

    editor.update_sequence("a", &plan).unwrap();

    assert_eq!(editor.finish().unwrap(), "a:\n  - k: short\n    j: 1\n");
}

#[test]
fn inserts_a_key_into_the_last_item_of_a_document_without_a_final_newline() {
    let text = "a:\n  - k: 1\n    j: 'q' # c";
    let mut editor = editor(text);
    let item = editor.items(editor.root_value("a").unwrap())[0];
    let plan = [PlannedItem::patch(
        item,
        vec![Patch::InsertKey {
            key: "m".to_owned(),
            value: "true".to_owned(),
        }],
        Vec::new(),
    )];

    editor.update_sequence("a", &plan).unwrap();

    assert_eq!(
        editor.finish().unwrap(),
        "a:\n  - k: 1\n    j: 'q' # c\n    m: \"true\"\n"
    );
}

#[test]
fn removes_a_multi_line_value_with_its_key() {
    let text = "a:\n  - k: 1\n    j:\n      - x\n      - y\n    l: 2\n";
    let mut editor = editor(text);
    let item = editor.items(editor.root_value("a").unwrap())[0];
    let plan = [PlannedItem::patch(
        item,
        vec![Patch::RemoveKey {
            key: "j".to_owned(),
        }],
        Vec::new(),
    )];

    editor.update_sequence("a", &plan).unwrap();

    assert_eq!(editor.finish().unwrap(), "a:\n  - k: 1\n    l: 2\n");
}

#[test]
fn reordering_rerenders_the_list_from_the_planned_lines() {
    let text = "a:\n    - x\n    - y # last\nb: 1\n";
    let mut editor = editor(text);
    let items = editor.items(editor.root_value("a").unwrap());
    let plan = [
        PlannedItem::keep(items[1], lines("y")),
        PlannedItem::keep(items[0], lines("x")),
    ];

    editor.update_sequence("a", &plan).unwrap();

    assert_eq!(
        editor.finish().unwrap(),
        "a:\n    - y\n    - x # last\nb: 1\n"
    );
}

#[test]
fn refuses_an_anchored_item_it_would_remove() {
    let text = "a:\n  - &one x\n  - y\n";
    let mut editor = editor(text);
    let items = editor.items(editor.root_value("a").unwrap());
    let error = editor
        .update_sequence("a", &[PlannedItem::keep(items[1], lines("y"))])
        .unwrap_err();

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.message,
        "cannot edit `a`: it uses an anchor (t.yml line 2)"
    );
}

#[test]
fn overlapping_plans_are_a_bug() {
    let text = "a:\n  - x\n";
    let mut editor = editor(text);
    let item = editor.items(editor.root_value("a").unwrap())[0];

    editor
        .update_sequence("a", &[PlannedItem::replace(item, lines("y"))])
        .unwrap();
    editor
        .update_sequence("a", &[PlannedItem::replace(item, lines("z"))])
        .unwrap();

    assert_eq!(editor.finish().unwrap_err().code, ExitCode::Internal);
}

#[test]
fn renders_scalars_the_way_the_emitter_quotes_them() {
    assert_eq!(plain_scalar("main"), "main");
    assert_eq!(plain_scalar("1e5"), "\"1e5\"");
    assert_eq!(plain_scalar("a: b"), "\"a: b\"");
    assert_eq!(quoted_scalar("say \"hi\""), "\"say \\\"hi\\\"\"");
    assert_eq!(
        styled_scalar(ScalarStyle::SingleQuoted, "it's"),
        Some("'it''s'".to_owned())
    );
    assert_eq!(styled_scalar(ScalarStyle::Literal, "x"), None);
}
