//! The positioned tree built from saphyr-parser events.
//!
//! Placeholder shapes: F1 owns this file and replaces the internals. Nothing outside `yaml/` may
//! depend on them.

/// Index of a node in [`Document`]'s arena.
pub(super) type NodeId = usize;

/// One parsed document: its text, and everything needed to position an error in it.
#[derive(Debug)]
pub(super) struct Document {
    /// How the document is named in error messages.
    pub file: String,
}
