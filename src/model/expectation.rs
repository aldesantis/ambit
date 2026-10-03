//! What an `expects` entry names: a fact about the world that has to be true for the thing to work.
//!
//! `expects` differs from `requires`: a requirement is **resolved**. It selects catalog items by
//! pattern, and an entry matching nothing fails install at exit 3. An expectation is **checked**:
//! nothing provides it, no two catalogs can offer competing copies of it, it cannot expect anything
//! back, and it cannot cycle. `doctor` asks the world about it; if the world says no, the install is
//! left alone and `doctor` fails at exit 6.
//!
//! ```yaml
//! ambit:
//!   requires: # resolved into the bundle; exit 3 if it matches nothing
//!     - name: close
//!       capabilities: [mcps]
//!   expects: # checked by `doctor`; exit 6 if unsatisfied
//!     - env: CLOSE_API_KEY
//! ```
//!
//! `env:` is the only kind today. It is a list rather than a bare `env:` key because a skill that
//! shells out to `docker` or `gh` has a precondition (the binary being on `PATH`) that does not fit
//! `env:`. Adding it later means one more entry in [`EXPECTATION_KINDS`] and one more case in
//! `doctor`, not a second top-level list.
//!
//! `expects` is the one annotation every kind of catalog entity carries: a skill reads variables at
//! runtime, a server reads its own credentials, and a hook's command reads whatever the shell the
//! harness spawns hands it.
//!
//! This is also the last list still written as one-key `<kind>: <name>` mappings. Its reader lives
//! here, not in `reference.rs`: a document list and `ambit why`'s command-line subject share no
//! reader at all.
//!
//! An entry declares its kind rather than encoding it in the value, the same one-key discriminator
//! an MCP entity's `transport` uses. Nothing guesses: a bare `expects: [CLOSE_API_KEY]` is refused
//! rather than read as an environment variable, since "`env` is the only kind today" is a fact about
//! today, not a shorthand that stays correct once a second kind is added.

use indexmap::IndexSet;

use crate::errors::{AmbitError, Result, at, config_error};
use crate::model::reference::Reference;
use crate::model::yaml::{YamlEntry, YamlMapping};
use crate::util::cmp::js_cmp;
use crate::util::string_enum;

string_enum! {
    /// Which kind of precondition an entry states.
    ///
    /// One today. This is the extension point: `bin:` for a program that must be on the `PATH`, a
    /// minimum harness version, a required file. Each lands here, in `doctor`, and nowhere else,
    /// since every surface that reads an entry reads [`EXPECTATION_KINDS`] to know what one may say.
    pub enum ExpectationKind {
        Env => "env",
    }
}

/// The kinds of precondition an `expects` entry can name, in the order every report lists them.
pub const EXPECTATION_KINDS: &[ExpectationKind] = ExpectationKind::ALL;

/// One precondition: which kind, and what it names inside that kind.
pub type Expectation = Reference<ExpectationKind>;

/// What a precondition of each kind is called in a message about one.
pub fn expectation_noun(kind: ExpectationKind) -> &'static str {
    match kind {
        ExpectationKind::Env => "an environment variable",
    }
}

/// The key the list is written under, wherever one is written.
const EXPECTS_KEY: &str = "expects";

/// Every expectation a set of declarers states, grouped by kind: the shape a bundle reports.
///
/// Every kind is present even when empty, so the keys a consumer reads are fixed by
/// [`EXPECTATION_KINDS`] rather than by what a particular project happens to declare.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExpectationSet {
    pub env: Vec<String>,
}

impl ExpectationSet {
    /// The names stated for one kind.
    pub fn get(&self, kind: ExpectationKind) -> &[String] {
        match kind {
            ExpectationKind::Env => &self.env,
        }
    }
}

/// The kinds as a refusal lists them. One today, but the join stays correct once more are added.
fn kind_list() -> String {
    EXPECTATION_KINDS
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// How an expectation is written in a document, for a message telling someone to write one.
fn expectation_yaml(kind: ExpectationKind, name: &str) -> String {
    format!("- {kind}: {name}")
}

/// The error for an entry written as a bare string: the shape a list of plain names has.
///
/// Names the spelling rather than assuming it: `expects: [CLOSE_API_KEY]` is probably an
/// environment variable today, since that is the only kind, but treating it as one is the shorthand
/// this format refuses. A `bin:` kind arriving later would otherwise silently reinterpret a file
/// nobody edited.
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

/// The error for an entry mapping that names no kind of precondition, or more than one.
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

/// Parses an `expects` list: a sequence of one-key mappings, each naming a kind and a value in it.
///
/// Returned in the order it was written; nothing here sorts it. `mapping` is the block the key sits
/// in: a skill's `ambit:`, or an entity's whole document.
///
/// # Errors
///
/// Exit 2 for an entry that is not a mapping, one that names no kind or several, or one whose value
/// is not a string.
pub fn parse_expectations(mapping: &YamlMapping) -> Result<Vec<Expectation>> {
    let Some(entries) = mapping.optional_entry_list(EXPECTS_KEY)? else {
        return Ok(Vec::new());
    };

    entries
        .into_iter()
        .map(|entry| {
            // A string is a plain name; everything else the sequence could hold was already refused
            // by `optional_entry_list`.
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

/// The distinct names of one kind, sorted.
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

/// The environment variables an `expects` list names, sorted and deduplicated.
///
/// The one projection with callers outside `doctor`: a harness config passes a server's variables
/// through to the process it spawns, and needs a list of names rather than of entries. Kept here,
/// rather than at each call site, so a second kind arriving cannot leak into an `env` map.
pub fn expected_env(expects: &[Expectation]) -> Vec<String> {
    names_of(expects.iter(), ExpectationKind::Env)
}

/// The union of several `expects` lists, grouped by kind and sorted within each.
pub fn union_expectations(lists: &[&[Expectation]]) -> ExpectationSet {
    ExpectationSet {
        env: names_of(
            lists.iter().flat_map(|list| list.iter()),
            ExpectationKind::Env,
        ),
    }
}
