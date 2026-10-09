use indexmap::IndexSet;

use crate::errors::{AmbitError, Result, at, config_error};
use crate::model::reference::Reference;
use crate::model::yaml::{YamlEntry, YamlMapping};
use crate::util::cmp::js_cmp;
use crate::util::string_enum;

string_enum! {
    pub enum ExpectationKind {
        Env => "env",
    }
}

pub const EXPECTATION_KINDS: &[ExpectationKind] = ExpectationKind::ALL;

pub type Expectation = Reference<ExpectationKind>;

pub fn expectation_noun(kind: ExpectationKind) -> &'static str {
    match kind {
        ExpectationKind::Env => "an environment variable",
    }
}

const EXPECTS_KEY: &str = "expects";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExpectationSet {
    pub env: Vec<String>,
}

impl ExpectationSet {
    pub fn get(&self, kind: ExpectationKind) -> &[String] {
        match kind {
            ExpectationKind::Env => &self.env,
        }
    }
}

fn kind_list() -> String {
    EXPECTATION_KINDS
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn expectation_yaml(kind: ExpectationKind, name: &str) -> String {
    format!("- {kind}: {name}")
}

fn bare_entry(mapping: &YamlMapping, value: &str) -> AmbitError {
    let kind = EXPECTATION_KINDS[0];
    let kinds = EXPECTATION_KINDS
        .iter()
        .map(|one| format!("`{one}:`"))
        .collect::<Vec<_>>()
        .join(", ");

    mapping.key_error(
        EXPECTS_KEY,
        &format!("`{EXPECTS_KEY}` entry \"{value}\" names no precondition"),
        vec![
            format!("an entry says what kind of precondition it is: {kinds}"),
            format!(
                "for {} named \"{value}\", that is `{}`",
                expectation_noun(kind),
                expectation_yaml(kind, value)
            ),
        ],
    )
}

fn ambiguous_entry(entry: &YamlMapping, keys: &[String]) -> AmbitError {
    let advice = vec![
        format!("an entry is one key: {}", kind_list()),
        "give it exactly one of them".to_owned(),
    ];

    match keys.first() {
        None => config_error(
            format!(
                "an `{EXPECTS_KEY}` entry names no precondition {}",
                at(entry.file(), entry.line())
            ),
            advice,
        ),
        Some(first) => {
            let mut sorted = keys.to_vec();

            sorted.sort_by(|a, b| js_cmp(a, b));

            entry.key_error(
                first,
                &format!(
                    "an `{EXPECTS_KEY}` entry names {} preconditions: {}",
                    keys.len(),
                    sorted.join(", ")
                ),
                advice,
            )
        }
    }
}

pub fn parse_expectations(mapping: &YamlMapping) -> Result<Vec<Expectation>> {
    let Some(entries) = mapping.optional_entry_list(EXPECTS_KEY)? else {
        return Ok(Vec::new());
    };

    entries
        .into_iter()
        .map(|entry| {
            let entry = match entry {
                YamlEntry::String(positioned) => {
                    return Err(bare_entry(mapping, &positioned.value));
                }
                YamlEntry::Mapping(entry) => entry,
            };

            let keys = entry.keys();

            if keys.len() != 1 {
                return Err(ambiguous_entry(&entry, &keys));
            }

            let kind_text = &keys[0];

            let Some(kind) = ExpectationKind::parse(kind_text) else {
                return Err(entry.key_error(
                    kind_text,
                    &format!("unknown precondition \"{kind_text}\" in an `{EXPECTS_KEY}` entry"),
                    vec![
                        format!("an entry is one key: {}", kind_list()),
                        format!("replace `{kind_text}` with one of them"),
                    ],
                ));
            };

            Ok(Reference {
                kind,
                name: entry.require_string(kind_text)?,
            })
        })
        .collect()
}

fn names_of<'e>(
    expects: impl Iterator<Item = &'e Expectation>,
    kind: ExpectationKind,
) -> Vec<String> {
    let mut names: Vec<String> = expects
        .filter(|item| item.kind == kind)
        .map(|item| item.name.clone())
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();

    names.sort_by(|a, b| js_cmp(a, b));
    names
}

pub fn expected_env(expects: &[Expectation]) -> Vec<String> {
    names_of(expects.iter(), ExpectationKind::Env)
}

pub fn union_expectations(lists: &[&[Expectation]]) -> ExpectationSet {
    ExpectationSet {
        env: names_of(
            lists.iter().flat_map(|list| list.iter()),
            ExpectationKind::Env,
        ),
    }
}
