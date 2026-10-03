//! The one shape two vocabularies share: which kind a thing is, and the name inside that kind.
//!
//! Two parsers produce it, each with exactly one caller: `expects`'s document list of one-key
//! mappings, in `expectation.rs`, and `ambit why`'s `<kind>:<name>` command-line subject, in
//! `requirement.rs`. Those are two grammars, not one shared vocabulary: a list never writes a
//! separator and a subject is never a mapping, so each reader lives in the module that owns its
//! words.
//!
//! This shape survives because both of them parse *to* it: a bundle item is one item of one
//! namespace, spelled the same way either place. It is deliberately only the shape; a kind's legal
//! values, and every word said about one, belong to the vocabulary that has them.

/// One member of one kind: which kind, and the name inside it.
///
/// The shape an entry parses to, and, over the item kinds, the shape resolution identifies a
/// bundle item by, since a bundle item is exactly one item of one namespace.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reference<K> {
    pub kind: K,
    pub name: String,
}
