//! Commented YAML files as `ambit init` writes them.

use crate::util::json::JsonObject;

/// One commented block of a scaffolded file: prose, then at most one of the two YAML forms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScaffoldBlock {
    /// Prose, one entry per emitted line. An empty entry is a bare `#` separator.
    pub comment: Vec<String>,
    /// Keys this block sets.
    pub values: Option<JsonObject>,
    /// Keys shown commented out, for something only the reader can supply.
    pub example: Option<JsonObject>,
}

/// A scaffolded file, as bytes: each block separated by a blank line, with a trailing newline.
///
/// Pure and byte-stable: the output is a function of the blocks alone, so two runs on two machines
/// scaffold the same file.
pub fn render_scaffold(blocks: &[ScaffoldBlock]) -> String {
    let _ = blocks;
    todo!("port model/scaffold.ts:renderScaffold")
}
