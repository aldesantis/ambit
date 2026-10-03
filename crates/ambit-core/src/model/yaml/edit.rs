//! Lossless edits to a YAML document someone else wrote.
//!
//! An edit never re-emits the document. It collects byte ranges of the original text to replace,
//! so everything outside them (comments, blank lines, quoting, key order) stays byte-identical.
//! Only the root mapping's sequences are edited, which is every list `ambit.yml` has.
//!
//! - A block sequence is edited in place. A new item goes after the last one, at the existing dash
//!   indentation. A removed item takes its whole lines with it, a comment trailing it on the same
//!   line included; comments on lines of their own stay. A changed item swaps only the spans that
//!   changed, or, when that is not possible, the item's own span.
//! - A flow sequence is re-rendered whole in block style, comments inside it included, since
//!   splicing one item into it would need its own formatting rules. So is a block sequence whose
//!   items change order.
//! - A sequence left with no items becomes `[]`: a key with no value would read as null.
//! - A missing key is appended at the end of the document.
//! - A node the edit touches that carries an anchor or a tag is refused rather than rewritten:
//!   dropping either changes what the document means. Multi-document input is already refused by
//!   the parser, and a flow-style root has no line structure to splice.
//!
//! The editor knows nothing about any format. Its caller says what each item becomes, as rendered
//! block lines for the cases where it cannot be spliced, and checks the result by parsing it again.

use std::ops::Range;

use saphyr_parser::ScalarStyle;

use super::emit_yaml;
use super::tree::{self, Document, NodeId, NodeKind};
use crate::errors::{AmbitError, Result, at, config_error};
use crate::util::json::{JsonValue, stringify};

/// A node of the document being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct YamlNode(NodeId);

/// One change inside an item that is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Patch {
    /// Replaces a scalar's value, keeping its quoting style where the value allows it.
    Scalar { node: YamlNode, value: String },
    /// Adds `key: value` as the last key of the item's mapping.
    InsertKey { key: String, value: String },
    /// Removes `key` from the item's mapping.
    RemoveKey { key: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Change {
    Keep,
    Patch(Vec<Patch>),
    Replace,
}

/// One item of a sequence as it should read after the edit.
///
/// `lines` is the whole item rendered as a block at indentation zero, without its dash: what is
/// written when the item is new, when it has to be replaced whole, and when the sequence is
/// re-rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedItem {
    original: Option<YamlNode>,
    change: Change,
    lines: Vec<String>,
}

impl PlannedItem {
    /// An existing item, unchanged.
    pub fn keep(node: YamlNode, lines: Vec<String>) -> Self {
        Self {
            original: Some(node),
            change: Change::Keep,
            lines,
        }
    }

    /// An existing item, changed by `patches`. Replaced whole by `lines` when a patch cannot be
    /// spliced, such as removing the key that shares a line with the item's dash.
    pub fn patch(node: YamlNode, patches: Vec<Patch>, lines: Vec<String>) -> Self {
        Self {
            original: Some(node),
            change: Change::Patch(patches),
            lines,
        }
    }

    /// An existing item, replaced whole by `lines`.
    pub fn replace(node: YamlNode, lines: Vec<String>) -> Self {
        Self {
            original: Some(node),
            change: Change::Replace,
            lines,
        }
    }

    pub fn added(lines: Vec<String>) -> Self {
        Self {
            original: None,
            change: Change::Keep,
            lines,
        }
    }
}

struct Splice {
    range: Range<usize>,
    text: String,
}

/// A parsed document and the splices collected against it.
pub(crate) struct YamlEditor {
    document: Document,
    root: NodeId,
    /// The line ending new lines use: the document's own.
    newline: &'static str,
    splices: Vec<Splice>,
}

impl YamlEditor {
    /// Parses `text` under the loader's rules. `file` is how it is named in errors.
    ///
    /// # Errors
    ///
    /// Exit 2 when the text does not parse, its root is not a mapping, or the root is written in
    /// flow style.
    pub fn parse(text: &str, file: &str) -> Result<Self> {
        let document = tree::parse(text, file, 0)?;

        if let Some(problem) = tree::structural_problem(&document) {
            return Err(problem);
        }

        let Some(root) = document.root.filter(|&root| document.is_map(root)) else {
            return Err(config_error(
                format!("{file} is not a YAML mapping"),
                ["write the document as `key: value` pairs"],
            ));
        };

        if document.is_flow(root) {
            return Err(config_error(
                format!(
                    "cannot edit {file}: its root is a flow mapping {}",
                    at(file, Some(document.line_of(root)))
                ),
                [
                    "ambit edits a config line by line, and a `{ ... }` document has no lines to edit",
                    "write the document as block `key: value` pairs, or make this change by hand",
                ],
            ));
        }

        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };

        Ok(Self {
            document,
            root,
            newline,
            splices: Vec::new(),
        })
    }

    /// The value of a root key.
    pub fn root_value(&self, key: &str) -> Option<YamlNode> {
        self.pair(self.root, key).map(|(_, value)| YamlNode(value))
    }

    /// The value of `key` in a mapping node.
    pub fn value(&self, map: YamlNode, key: &str) -> Option<YamlNode> {
        self.pair(map.0, key).map(|(_, value)| YamlNode(value))
    }

    /// The string keys of a mapping node, in document order.
    pub fn keys(&self, map: YamlNode) -> Vec<&str> {
        self.document
            .pairs(map.0)
            .iter()
            .filter_map(|&(key, _)| self.document.string(key))
            .collect()
    }

    /// The items of a sequence node. Empty for anything else.
    pub fn items(&self, seq: YamlNode) -> Vec<YamlNode> {
        self.document
            .items(seq.0)
            .iter()
            .map(|&item| YamlNode(item))
            .collect()
    }

    /// The value of a string scalar.
    pub fn string(&self, node: YamlNode) -> Option<&str> {
        self.document.string(node.0)
    }

    /// Rewrites the sequence under the root key `key` to hold `plan`, in that order.
    ///
    /// Every existing item absent from `plan` is removed. Nothing is touched when the plan keeps
    /// every item unchanged and in order. An absent key is appended, as `key: []` when `plan` is
    /// empty. Splices are collected, not applied: see [`YamlEditor::finish`].
    ///
    /// # Errors
    ///
    /// Exit 2 when a node the edit touches carries an anchor or a tag, when a sequence item's
    /// dash does not start its line, or when a key must be appended to a document with an explicit
    /// end marker.
    pub fn update_sequence(&mut self, key: &str, plan: &[PlannedItem]) -> Result<()> {
        let Some((key_node, seq)) = self.pair(self.root, key) else {
            return self.append_key(key, plan);
        };

        let originals = self.document.items(seq).to_vec();

        if is_unchanged(plan, &originals) {
            return Ok(());
        }

        self.check_node(key, key_node)?;
        self.check_node(key, seq)?;

        if self.document.is_flow(seq)
            || !matches!(self.document.kind(seq), NodeKind::Seq(_))
            || !keeps_order(plan, &originals)
        {
            return self.rerender(key, key_node, seq, plan);
        }

        let indent = self.dash_column(key, originals[0])?;

        for &item in &originals {
            if plan
                .iter()
                .any(|planned| planned.original == Some(YamlNode(item)))
            {
                continue;
            }

            self.check_subtree(key, item)?;
            let range = self.item_lines(key, item)?;
            self.splice(range, String::new());
        }

        if plan.is_empty() {
            let colon = self.colon_after(key_node);
            self.splice(colon + 1..colon + 1, " []".to_owned());

            return Ok(());
        }

        let mut added = Vec::new();

        for planned in plan {
            let Some(YamlNode(item)) = planned.original else {
                added.push(self.block_item(&planned.lines, indent));
                continue;
            };

            match &planned.change {
                Change::Keep => {}
                Change::Patch(patches) => self.patch_item(key, item, patches, &planned.lines)?,
                Change::Replace => self.replace_item(key, item, &planned.lines)?,
            }
        }

        if !added.is_empty() {
            let last = originals[originals.len() - 1];
            let at = self.line_end(self.document.byte_span(last).end);
            let lead = self.lead_for(at);
            let mut text = lead.to_owned();

            for item in &added {
                text.push_str(item);
                text.push_str(self.newline);
            }

            self.splice(at..at, text);
        }

        Ok(())
    }

    /// The edited text.
    ///
    /// # Errors
    ///
    /// Exit 1 when two collected splices overlap, which is a bug in the caller's plan.
    pub fn finish(mut self) -> Result<String> {
        self.splices.sort_by_key(|splice| splice.range.start);

        let text = self.document.text();
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;

        for splice in &self.splices {
            if splice.range.start < cursor {
                return Err(AmbitError::unexpected(format!(
                    "overlapping edits at byte {} of {}",
                    splice.range.start, self.document.file
                )));
            }

            out.push_str(&text[cursor..splice.range.start]);
            out.push_str(&splice.text);
            cursor = splice.range.end;
        }

        out.push_str(&text[cursor..]);
        Ok(out)
    }

    fn pair(&self, map: NodeId, key: &str) -> Option<(NodeId, NodeId)> {
        self.document
            .pairs(map)
            .iter()
            .copied()
            .find(|&(name, _)| self.document.string(name) == Some(key))
    }

    fn splice(&mut self, range: Range<usize>, text: String) {
        self.splices.push(Splice { range, text });
    }

    /// Refuses an edit touching a node that carries an anchor or a tag. `subject` is the root key
    /// being edited, for the message.
    fn check_node(&self, subject: &str, id: NodeId) -> Result<()> {
        let what = if self.document.is_anchored(id) {
            "an anchor"
        } else if self.document.tag(id).is_some() {
            "a tag"
        } else {
            return Ok(());
        };
        let file = &self.document.file;

        Err(config_error(
            format!(
                "cannot edit `{subject}`: it uses {what} {}",
                at(file, Some(self.document.line_of(id)))
            ),
            [
                format!(
                    "ambit edits a config without re-emitting it, and rewriting {what} could change what other parts of the document mean"
                ),
                format!("remove {what}, or make this change by hand"),
            ],
        ))
    }

    fn check_subtree(&self, subject: &str, id: NodeId) -> Result<()> {
        self.check_node(subject, id)?;

        match self.document.kind(id) {
            NodeKind::Map(pairs) => {
                for &(key, value) in pairs {
                    self.check_subtree(subject, key)?;
                    self.check_subtree(subject, value)?;
                }
            }
            NodeKind::Seq(items) => {
                for &item in items {
                    self.check_subtree(subject, item)?;
                }
            }
            NodeKind::Scalar(_) | NodeKind::Alias => {}
        }

        Ok(())
    }

    fn line_start(&self, byte: usize) -> usize {
        self.document.text()[..byte]
            .rfind('\n')
            .map_or(0, |newline| newline + 1)
    }

    /// Where the line holding `byte` ends, past its line break.
    fn line_end(&self, byte: usize) -> usize {
        let text = self.document.text();

        text[byte..]
            .find('\n')
            .map_or(text.len(), |newline| byte + newline + 1)
    }

    /// What new lines inserted at `at` need in front of them: a line break when `at` is the end of
    /// a document whose last line has none.
    fn lead_for(&self, at: usize) -> &'static str {
        let text = self.document.text();

        if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
            self.newline
        } else {
            ""
        }
    }

    /// The byte offset of the `:` after a mapping key.
    fn colon_after(&self, key: NodeId) -> usize {
        let end = self.document.byte_span(key).end;

        self.document.text()[end..]
            .find(':')
            .map_or(end, |colon| end + colon)
    }

    /// The column of the dash introducing a block sequence item.
    fn dash_column(&self, subject: &str, item: NodeId) -> Result<usize> {
        let dash = self.dash_of(subject, item)?;

        Ok(dash - self.line_start(dash))
    }

    /// The byte offset of the dash introducing a block sequence item, refusing a dash that does not
    /// start its line (`- - a`), which no line-based edit can take apart.
    fn dash_of(&self, subject: &str, item: NodeId) -> Result<usize> {
        let text = self.document.text();
        let mut before = text[..self.document.byte_span(item).start].trim_end();
        // An item's span starts after its anchor and tag, which sit between it and the dash.
        let mut properties = usize::from(self.document.is_anchored(item))
            + usize::from(self.document.tag(item).is_some());

        loop {
            if before.ends_with('-') {
                let dash = before.len() - 1;

                if text[self.line_start(dash)..dash].chars().all(|c| c == ' ') {
                    return Ok(dash);
                }
            }

            if properties == 0 {
                break;
            }

            properties -= 1;
            before = before
                .trim_end_matches(|c: char| !c.is_whitespace())
                .trim_end();
        }

        Err(config_error(
            format!(
                "cannot edit `{subject}`: an item does not start its own line {}",
                at(&self.document.file, Some(self.document.line_of(item)))
            ),
            [
                "ambit edits a list line by line, one `- ` item per line",
                "put the item on a line of its own, or make this change by hand",
            ],
        ))
    }

    /// The whole lines a block sequence item occupies, from its dash to the end of its last line.
    fn item_lines(&self, subject: &str, item: NodeId) -> Result<Range<usize>> {
        let dash = self.dash_of(subject, item)?;

        Ok(self.line_start(dash)..self.line_end(self.document.byte_span(item).end))
    }

    /// An item as written with its dash at column `indent`, without a trailing line break.
    fn block_item(&self, lines: &[String], indent: usize) -> String {
        format!("{}- {}", " ".repeat(indent), self.item_body(lines, indent))
    }

    /// An item as written after its dash at column `indent`.
    fn item_body(&self, lines: &[String], indent: usize) -> String {
        let continuation = format!("{}{}", self.newline, " ".repeat(indent + 2));

        lines.join(&continuation)
    }

    /// Replaces an item's own span, after its dash, with `lines`.
    fn replace_item(&mut self, subject: &str, item: NodeId, lines: &[String]) -> Result<()> {
        self.check_subtree(subject, item)?;

        let indent = self.dash_column(subject, item)?;
        let body = self.item_body(lines, indent);

        self.splice(self.document.byte_span(item), body);
        Ok(())
    }

    /// Applies `patches` to a kept item, falling back to replacing it whole when one cannot be
    /// spliced.
    fn patch_item(
        &mut self,
        subject: &str,
        item: NodeId,
        patches: &[Patch],
        lines: &[String],
    ) -> Result<()> {
        self.check_node(subject, item)?;

        match self.patch_splices(subject, item, patches)? {
            Some(splices) => {
                self.splices.extend(splices);
                Ok(())
            }
            None => self.replace_item(subject, item, lines),
        }
    }

    /// The splices `patches` make to `item`, or `None` when one of them cannot be spliced.
    fn patch_splices(
        &self,
        subject: &str,
        item: NodeId,
        patches: &[Patch],
    ) -> Result<Option<Vec<Splice>>> {
        let is_block_map = self.document.is_map(item) && !self.document.is_flow(item);
        let mut splices = Vec::new();

        for patch in patches {
            match patch {
                Patch::Scalar { node, value } => {
                    self.check_node(subject, node.0)?;

                    let Some(rendered) = self
                        .document
                        .scalar_style(node.0)
                        .and_then(|style| styled_scalar(style, value))
                    else {
                        return Ok(None);
                    };

                    splices.push(Splice {
                        range: self.document.byte_span(node.0),
                        text: rendered,
                    });
                }
                Patch::InsertKey { key, value } => {
                    if !is_block_map {
                        return Ok(None);
                    }

                    let span = self.document.byte_span(item);
                    let column = span.start - self.line_start(span.start);
                    let at = self.line_end(span.end);

                    splices.push(Splice {
                        range: at..at,
                        text: format!(
                            "{}{}{key}: {}{}",
                            self.lead_for(at),
                            " ".repeat(column),
                            plain_scalar(value),
                            self.newline
                        ),
                    });
                }
                Patch::RemoveKey { key } => {
                    let Some((key_node, value)) = self.pair(item, key) else {
                        continue;
                    };

                    let first = self.document.pairs(item).first().map(|&(first, _)| first);
                    let key_start = self.document.byte_span(key_node).start;
                    let starts_line = self.document.text()[self.line_start(key_start)..key_start]
                        .chars()
                        .all(|c| c == ' ');

                    // The first key shares its line with the item's dash.
                    if !is_block_map || first == Some(key_node) || !starts_line {
                        return Ok(None);
                    }

                    self.check_subtree(subject, value)?;
                    splices.push(Splice {
                        range: self.line_start(key_start)
                            ..self.line_end(self.document.byte_span(value).end),
                        text: String::new(),
                    });
                }
            }
        }

        Ok(Some(splices))
    }

    /// Replaces a sequence's whole value with a block rendering of `plan`, or `[]` when it is
    /// empty.
    fn rerender(
        &mut self,
        subject: &str,
        key: NodeId,
        seq: NodeId,
        plan: &[PlannedItem],
    ) -> Result<()> {
        self.check_subtree(subject, seq)?;

        let colon = self.colon_after(key);
        let end = self.document.byte_span(seq).end.max(colon + 1);

        if plan.is_empty() {
            self.splice(colon + 1..end, " []".to_owned());

            return Ok(());
        }

        let indent = match self.document.items(seq).first() {
            Some(&first) if !self.document.is_flow(seq) => self.dash_column(subject, first)?,
            _ => {
                let start = self.document.byte_span(key).start;

                start - self.line_start(start) + 2
            }
        };
        let mut rendered = String::new();

        for planned in plan {
            rendered.push_str(self.newline);
            rendered.push_str(&self.block_item(&planned.lines, indent));
        }

        self.splice(colon + 1..end, rendered);
        Ok(())
    }

    /// Appends `key` with `plan` as its block sequence at the end of the document.
    fn append_key(&mut self, key: &str, plan: &[PlannedItem]) -> Result<()> {
        let text = self.document.text();

        if text
            .lines()
            .any(|line| line.trim_end() == "..." || line.starts_with("... "))
        {
            return Err(config_error(
                format!(
                    "cannot add `{key}` to {}: the document has an explicit end marker",
                    self.document.file
                ),
                [
                    "a key appended after `...` would start a second document",
                    "remove the `...` line, or make this change by hand",
                ],
            ));
        }

        let at = text.len();
        let mut out = format!("{}{key}:", self.lead_for(at));

        if plan.is_empty() {
            out.push_str(" []");
        }

        for planned in plan {
            out.push_str(self.newline);
            out.push_str(&self.block_item(&planned.lines, 2));
        }

        out.push_str(self.newline);
        self.splice(at..at, out);
        Ok(())
    }
}

/// Whether `plan` keeps every item of `originals`, unchanged and in order, and adds nothing.
fn is_unchanged(plan: &[PlannedItem], originals: &[NodeId]) -> bool {
    plan.len() == originals.len()
        && plan.iter().zip(originals).all(|(planned, &original)| {
            planned.original == Some(YamlNode(original)) && planned.change == Change::Keep
        })
}

/// Whether `plan` can be spliced into the block sequence `originals`: the existing items it keeps
/// appear in their original order, and every new item comes after them.
fn keeps_order(plan: &[PlannedItem], originals: &[NodeId]) -> bool {
    let mut last = None;
    let mut adding = false;

    for planned in plan {
        let Some(YamlNode(item)) = planned.original else {
            adding = true;
            continue;
        };

        let Some(index) = originals.iter().position(|&original| original == item) else {
            return false;
        };

        if adding || last.is_some_and(|last| index <= last) {
            return false;
        }

        last = Some(index);
    }

    true
}

/// A string as a plain scalar where that reads back as the same string, double-quoted otherwise.
///
/// The emitter's rules, so a value written here is quoted exactly when ambit's own files would
/// quote it.
pub(crate) fn plain_scalar(value: &str) -> String {
    emit_yaml(&JsonValue::String(value.to_owned()))
        .trim_end_matches('\n')
        .to_owned()
}

/// A string as a double-quoted scalar. JSON's escaping is valid YAML.
pub(crate) fn quoted_scalar(value: &str) -> String {
    stringify(&JsonValue::String(value.to_owned()))
}

/// A string in the quoting style an existing scalar was written in, so replacing a value does not
/// change how it is quoted. `None` for a block scalar, which is replaced whole instead.
fn styled_scalar(style: ScalarStyle, value: &str) -> Option<String> {
    match style {
        ScalarStyle::Plain => Some(plain_scalar(value)),
        ScalarStyle::SingleQuoted => Some(format!("'{}'", value.replace('\'', "''"))),
        ScalarStyle::DoubleQuoted => Some(quoted_scalar(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
