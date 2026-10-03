//! `expects`: what must be true of the world for an item to work.

use crate::errors::Result;
use crate::model::reference::Reference;
use crate::model::yaml::YamlMapping;
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
    let _ = mapping;
    todo!("port model/expectation.ts:parseExpectations")
}

/// The environment variables an `expects` list names, sorted and deduplicated.
///
/// The one projection with callers outside `doctor`: a harness config passes a server's variables
/// through to the process it spawns, and needs a list of names rather than of entries. Kept here,
/// rather than at each call site, so a second kind arriving cannot leak into an `env` map.
pub fn expected_env(expects: &[Expectation]) -> Vec<String> {
    let _ = expects;
    todo!("port model/expectation.ts:expectedEnv")
}

/// The union of several `expects` lists, grouped by kind and sorted within each.
pub fn union_expectations(lists: &[&[Expectation]]) -> ExpectationSet {
    let _ = lists;
    todo!("port model/expectation.ts:unionExpectations")
}
