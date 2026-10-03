//! The positioned tree built from saphyr-parser events, and the rules enforced on it.
//!
//! saphyr's own loader collapses duplicate keys, so the tree is built here from the event stream
//! instead: every node keeps where it started, and a mapping keeps its pairs in document order,
//! duplicates included, so the duplicate-key error can name both lines.
//!
//! Nodes also keep what the lossless editor (`edit.rs`) needs to splice the original text: the
//! scalar style, whether a collection is flow style, an anchor, and where a flow collection ends.
//!
//! Nothing outside `yaml/` depends on these shapes.

use std::ops::Range;

use saphyr_parser::{Event, Parser, ScalarStyle, ScanError, Span, Tag};

use super::schema::{self, CORE_PREFIX, CORE_TAGS, Scalar};
use crate::errors::{AmbitError, Result, at, config_error};
use crate::util::text::js_trim;

/// Index of a node in [`Document`]'s arena.
pub(super) type NodeId = usize;

#[derive(Debug)]
pub(super) enum NodeKind {
    /// Pairs in document order, duplicates and non-string keys included.
    Map(Vec<(NodeId, NodeId)>),
    Seq(Vec<NodeId>),
    Scalar(Scalar),
    /// A `*name` reference. Not resolved: an alias is no supported value, so an accessor reports it
    /// rather than following it.
    Alias,
}

#[derive(Debug)]
pub(super) struct Node {
    pub kind: NodeKind,
    /// The explicit tag, fully expanded (`tag:yaml.org,2002:str`, `!mine`).
    tag: Option<String>,
    /// Whether the node carries an `&anchor`.
    anchored: bool,
    /// How a scalar was written. Absent for collections and aliases.
    style: Option<ScalarStyle>,
    /// Whether a collection is written in flow style (`[a, b]`, `{a: 1}`).
    flow: bool,
    /// Character offsets into the parsed text, as saphyr reports them.
    ///
    /// A flow collection ends past its closing bracket. A block collection's end is its start
    /// event's, which says nothing about its extent: [`Document::byte_span`] derives that from its
    /// children.
    start: usize,
    end: usize,
    /// 1-based, within the parsed text.
    line: usize,
}

/// One parsed document: its text, and everything needed to position an error in it.
#[derive(Debug)]
pub(super) struct Document {
    /// How the document is named in error messages.
    pub file: String,
    text: String,
    /// Lines of the containing file above the parsed text: a frontmatter block's opening delimiter
    /// and any blank lines under it. Needed so a reported line number matches the whole file, not
    /// just the parsed slice.
    line_offset: usize,
    nodes: Vec<Node>,
    /// Absent for a document holding nothing but whitespace and comments.
    pub root: Option<NodeId>,
}

impl Document {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn kind(&self, id: NodeId) -> &NodeKind {
        &self.nodes[id].kind
    }

    pub fn scalar(&self, id: NodeId) -> Option<&Scalar> {
        match self.kind(id) {
            NodeKind::Scalar(scalar) => Some(scalar),
            _ => None,
        }
    }

    /// The string value of a string scalar.
    pub fn string(&self, id: NodeId) -> Option<&str> {
        match self.scalar(id) {
            Some(Scalar::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn is_map(&self, id: NodeId) -> bool {
        matches!(self.kind(id), NodeKind::Map(_))
    }

    pub fn pairs(&self, id: NodeId) -> &[(NodeId, NodeId)] {
        match self.kind(id) {
            NodeKind::Map(pairs) => pairs,
            _ => &[],
        }
    }

    /// The 1-based line `id` starts on, counted in the containing file.
    pub fn line_of(&self, id: NodeId) -> usize {
        self.nodes[id].line + self.line_offset
    }

    /// The source text `id` was parsed from. Messages quote this rather than the parsed value, so
    /// the fix for `ref: 1e5` reads `ref: "1e5"` and not `ref: "100000"`.
    pub fn text_of(&self, id: NodeId) -> Option<&str> {
        let node = &self.nodes[id];
        let start = self.byte_offset(node.start);
        let end = self.byte_offset(node.end);
        let slice = js_trim(self.text.get(start..end)?);

        (!slice.is_empty()).then_some(slice)
    }

    /// The parsed text.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn items(&self, id: NodeId) -> &[NodeId] {
        match self.kind(id) {
            NodeKind::Seq(items) => items,
            _ => &[],
        }
    }

    pub fn is_flow(&self, id: NodeId) -> bool {
        self.nodes[id].flow
    }

    pub fn is_anchored(&self, id: NodeId) -> bool {
        self.nodes[id].anchored
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        self.nodes[id].tag.as_deref()
    }

    pub fn scalar_style(&self, id: NodeId) -> Option<ScalarStyle> {
        self.nodes[id].style
    }

    /// The byte range `id` occupies in [`Document::text`].
    ///
    /// A scalar's range stops at its last character: saphyr's span for a quoted scalar runs on over
    /// the whitespace after the closing quote. A block collection runs from its first item or key
    /// to the end of its last descendant, so a comment trailing it is outside the range.
    pub fn byte_span(&self, id: NodeId) -> Range<usize> {
        let node = &self.nodes[id];
        let start = self.byte_offset(node.start);

        let end = match &node.kind {
            NodeKind::Map(pairs) if !node.flow => pairs
                .iter()
                .map(|&(_, value)| self.byte_span(value).end)
                .max()
                .unwrap_or(start),
            NodeKind::Seq(items) if !node.flow => items
                .iter()
                .map(|&item| self.byte_span(item).end)
                .max()
                .unwrap_or(start),
            NodeKind::Scalar(_) if node.style == Some(ScalarStyle::DoubleQuoted) => {
                start + closing_quote(&self.text[start..], '"')
            }
            NodeKind::Scalar(_) if node.style == Some(ScalarStyle::SingleQuoted) => {
                start + closing_quote(&self.text[start..], '\'')
            }
            _ => {
                let end = self.byte_offset(node.end);
                let slice = self.text.get(start..end).unwrap_or_default();

                start + slice.trim_end().len()
            }
        };

        start..end.max(start)
    }

    fn byte_offset(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(byte, _)| byte)
    }

    /// How a value is described in a type-mismatch message.
    pub fn describe(&self, id: NodeId) -> &'static str {
        match self.kind(id) {
            NodeKind::Map(_) => "a mapping",
            NodeKind::Seq(_) => "a sequence",
            NodeKind::Scalar(Scalar::Null) => "null",
            NodeKind::Scalar(Scalar::Bool(_)) => "a boolean",
            NodeKind::Scalar(Scalar::Number(n)) if is_integer(*n) => "an integer",
            NodeKind::Scalar(Scalar::Number(_)) => "a number",
            NodeKind::Scalar(Scalar::String(_)) => "a string",
            NodeKind::Alias => "an unsupported value",
        }
    }
}

/// The length of the quoted scalar `text` starts with, through its closing quote: saphyr's span for
/// one can run on over the comment after it.
///
/// Inside double quotes a backslash escapes the next character; inside single quotes a doubled
/// quote stands for one.
fn closing_quote(text: &str, quote: char) -> usize {
    let mut chars = text.char_indices().skip(1).peekable();

    while let Some((index, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            chars.next();
        } else if c == quote {
            if quote == '\'' && chars.peek().is_some_and(|&(_, next)| next == '\'') {
                chars.next();
            } else {
                return index + c.len_utf8();
            }
        }
    }

    text.len()
}

/// `Number.isInteger`.
pub(super) fn is_integer(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0
}

/// What a collection being built still expects.
enum Frame {
    Seq(NodeId),
    Map { id: NodeId, key: Option<NodeId> },
}

struct Builder {
    file: String,
    text: String,
    line_offset: usize,
    nodes: Vec<Node>,
    stack: Vec<Frame>,
    root: Option<NodeId>,
}

impl Builder {
    fn add(&mut self, kind: NodeKind, attributes: Attributes, span: Span) -> NodeId {
        let id = self.nodes.len();
        let flow = matches!(kind, NodeKind::Map(_) | NodeKind::Seq(_))
            && matches!(self.text.chars().nth(span.start.index()), Some('[' | '{'));

        self.nodes.push(Node {
            kind,
            tag: attributes.tag,
            anchored: attributes.anchor != 0,
            style: attributes.style,
            flow,
            start: span.start.index(),
            end: span.end.index(),
            line: span.start.line(),
        });

        match self.stack.last_mut() {
            None => self.root = Some(id),
            Some(Frame::Seq(seq)) => {
                let seq = *seq;
                if let NodeKind::Seq(items) = &mut self.nodes[seq].kind {
                    items.push(id);
                }
            }
            Some(Frame::Map { id: map, key }) => match key.take() {
                None => *key = Some(id),
                Some(key_id) => {
                    let map = *map;
                    if let NodeKind::Map(pairs) = &mut self.nodes[map].kind {
                        pairs.push((key_id, id));
                    }
                }
            },
        }

        id
    }

    /// Closes the innermost collection. A flow collection's end event starts at its closing
    /// bracket, so that is where the collection is recorded to end.
    fn close(&mut self, span: Span) {
        let Some(Frame::Seq(id) | Frame::Map { id, .. }) = self.stack.pop() else {
            return;
        };

        if self.nodes[id].flow {
            self.nodes[id].end = span.start.index() + 1;
        }
    }

    fn line(&self, span: Span) -> usize {
        span.start.line() + self.line_offset
    }

    fn finish(self) -> Document {
        Document {
            file: self.file,
            text: self.text,
            line_offset: self.line_offset,
            nodes: self.nodes,
            root: self.root,
        }
    }
}

/// What an event says about a node besides its kind.
struct Attributes {
    tag: Option<String>,
    /// saphyr's anchor id: 0 for none.
    anchor: usize,
    style: Option<ScalarStyle>,
}

impl Attributes {
    fn new(anchor: usize, tag: Option<&Tag>) -> Self {
        Self {
            tag: tag.map(|tag| format!("{}{}", tag.handle, tag.suffix)),
            anchor,
            style: None,
        }
    }
}

/// Parses `text` into a positioned tree, rejecting a syntax error or a second document.
///
/// `line_offset` is the number of lines of the containing file above `text`, for a frontmatter
/// block.
///
/// # Errors
///
/// Exit 2 for a syntax error, a tab used as indentation, or more than one document.
pub(super) fn parse(text: &str, file: &str, line_offset: usize) -> Result<Document> {
    let mut builder = Builder {
        file: file.to_owned(),
        text: text.to_owned(),
        line_offset,
        nodes: Vec::new(),
        stack: Vec::new(),
        root: None,
    };
    let mut documents = 0;

    for event in Parser::new_from_str(text) {
        let (event, span) = event.map_err(|error| syntax_error(&builder, &error))?;

        match event {
            Event::DocumentStart(_) => {
                documents += 1;

                if documents > 1 {
                    return Err(config_error(
                        format!("invalid YAML {}", at(file, Some(builder.line(span)))),
                        ["Source contains multiple documents", "fix the syntax error"],
                    ));
                }
            }
            Event::Scalar(value, style, anchor, tag) => {
                let mut attributes = Attributes::new(anchor, tag.as_deref());
                let plain = matches!(style, ScalarStyle::Plain);
                let scalar = schema::resolve_scalar(&value, plain, attributes.tag.as_deref());

                attributes.style = Some(style);
                builder.add(NodeKind::Scalar(scalar), attributes, span);
            }
            Event::SequenceStart(anchor, tag) => {
                let attributes = Attributes::new(anchor, tag.as_deref());
                let id = builder.add(NodeKind::Seq(Vec::new()), attributes, span);

                builder.stack.push(Frame::Seq(id));
            }
            Event::MappingStart(anchor, tag) => {
                let attributes = Attributes::new(anchor, tag.as_deref());
                let id = builder.add(NodeKind::Map(Vec::new()), attributes, span);

                builder.stack.push(Frame::Map { id, key: None });
            }
            Event::SequenceEnd | Event::MappingEnd => builder.close(span),
            Event::Alias(_) => {
                builder.add(NodeKind::Alias, Attributes::new(0, None), span);
            }
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
        }
    }

    Ok(builder.finish())
}

/// Rewrites a parser error into ambit's message shape, keeping the position.
fn syntax_error(builder: &Builder, error: &ScanError) -> AmbitError {
    let line = error_line(&builder.text, error);
    let where_ = at(&builder.file, Some(line + builder.line_offset));

    if is_tab_as_indent(&builder.text, error) {
        return config_error(
            format!("YAML does not permit tabs for indentation {where_}"),
            [
                "a tab is indistinguishable from indentation but is not indentation",
                "replace the tab with spaces",
            ],
        );
    }

    config_error(
        format!("invalid YAML {where_}"),
        [error.info(), "fix the syntax error"],
    )
}

/// The line an error is reported on, moved to where yaml reported it in the cases where saphyr
/// notices the problem later than yaml did.
///
/// - A source that ends mid-construct (an unterminated quote, an unclosed flow collection) is
///   reported on its last line, as yaml did, not past it: saphyr's marker can sit one line beyond
///   a source with no final newline, and points at the opening quote of an unterminated scalar.
/// - A plain line that is not a `key: value` pair (`tampered` appended to a file) is reported on
///   that line. saphyr only notices when it reaches the next token, which is on a later line or
///   past the end, so the line is the last non-blank one before its marker.
fn error_line(text: &str, error: &ScanError) -> usize {
    let last = text.matches('\n').count() + 1;
    let info = error.info();

    if info.contains("unexpected end of stream") {
        return last;
    }

    let line = error.marker().line();

    if info.starts_with("simple key expect") {
        let lines: Vec<&str> = text.split('\n').collect();

        if let Some(found) = (1..line.min(last + 1)).rev().find(|&candidate| {
            lines
                .get(candidate - 1)
                .is_some_and(|l| !js_trim(l).is_empty())
        }) {
            return found;
        }
    }

    line.min(last)
}

/// Whether a scan error is a tab used as indentation: saphyr says so in its message, and
/// otherwise a tab in the error line's indentation gives it away.
fn is_tab_as_indent(text: &str, error: &ScanError) -> bool {
    if error.info().starts_with("tabs disallowed") {
        return true;
    }

    let line = text
        .split('\n')
        .nth(error.marker().line().saturating_sub(1));

    line.is_some_and(|line| {
        line.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .any(|c| c == '\t')
    })
}

/// The first structural violation, in document order: a custom tag, a non-string key, or a
/// duplicate key. These are all things the parser tolerates and ambit must not.
///
/// The walk is yaml's `visit` order: a node, then for a mapping each key and then its value,
/// depth first. A mapping's keys are all checked when the mapping is visited, before anything
/// nested in it.
pub(super) fn structural_problem(document: &Document) -> Option<AmbitError> {
    let mut problems = Vec::new();

    if let Some(root) = document.root {
        visit(document, root, &mut problems);
    }

    problems.into_iter().next()
}

fn visit(document: &Document, id: NodeId, problems: &mut Vec<AmbitError>) {
    match document.kind(id) {
        NodeKind::Map(pairs) => {
            check_tag(document, id, problems);
            check_keys(document, pairs, problems);

            for &(key, value) in pairs {
                visit(document, key, problems);
                visit(document, value, problems);
            }
        }
        NodeKind::Seq(items) => {
            check_tag(document, id, problems);

            for &item in items {
                visit(document, item, problems);
            }
        }
        NodeKind::Scalar(_) => check_tag(document, id, problems),
        NodeKind::Alias => {}
    }
}

fn check_tag(document: &Document, id: NodeId, problems: &mut Vec<AmbitError>) {
    let Some(tag) = document.node(id).tag.as_deref() else {
        return;
    };

    if CORE_TAGS.contains(&tag) {
        return;
    }

    let shorthand = match tag.strip_prefix(CORE_PREFIX) {
        Some(suffix) => format!("!!{suffix}"),
        None => tag.to_owned(),
    };

    problems.push(config_error(
        format!(
            "custom YAML tag `{shorthand}` is not permitted {}",
            at(&document.file, Some(document.line_of(id)))
        ),
        [
            "ambit parses YAML 1.2 with the core schema only".to_owned(),
            format!("remove `{shorthand}`"),
        ],
    ));
}

fn check_keys(document: &Document, pairs: &[(NodeId, NodeId)], problems: &mut Vec<AmbitError>) {
    let mut seen: Vec<(&str, Option<usize>)> = Vec::new();

    for &(key, _) in pairs {
        let Some(name) = document.string(key) else {
            problems.push(config_error(
                format!(
                    "mapping keys must be strings {}",
                    at(&document.file, Some(document.line_of(key)))
                ),
                [
                    format!("found {} as a key", document.describe(key)),
                    "quote the key".to_owned(),
                ],
            ));
            continue;
        };

        let line = Some(document.line_of(key));

        if let Some((_, first)) = seen.iter().find(|(seen, _)| *seen == name) {
            problems.push(config_error(
                format!("duplicate key \"{name}\" {}", at(&document.file, line)),
                [
                    first.map_or_else(
                        || "already defined earlier".to_owned(),
                        |first| format!("first defined on line {first}"),
                    ),
                    "remove one of the two definitions".to_owned(),
                ],
            ));
            continue;
        }

        seen.push((name, line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_nodes_by_character_and_one_based_line() {
        // saphyr's marker index counts characters, not bytes; the slice must still land on "ab".
        let document = parse("é: ab\nx: 'q'\n", "t.yml", 0).unwrap();
        let root = document.root.unwrap();
        let pairs = document.pairs(root);

        assert_eq!(document.text_of(pairs[0].1), Some("ab"));
        assert_eq!(document.line_of(pairs[0].1), 1);
        assert_eq!(document.text_of(pairs[1].1), Some("'q'"));
        assert_eq!(document.line_of(pairs[1].0), 2);
    }

    #[test]
    fn positions_a_nested_block_mapping_at_its_first_key() {
        let document = parse("a:\n  b: 1\n", "t.yml", 0).unwrap();
        let root = document.root.unwrap();

        assert_eq!(document.line_of(root), 1);
        assert_eq!(document.line_of(document.pairs(root)[0].1), 2);
    }

    #[test]
    fn spans_stop_at_a_quoted_scalar_and_a_flow_bracket() {
        let document = parse("a: [x, 'y' ] # c\nb: \"z\\\"\"  # d\n", "t.yml", 0).unwrap();
        let root = document.root.unwrap();
        let pairs = document.pairs(root);
        let span = |id| &document.text()[document.byte_span(id)];

        assert!(document.is_flow(pairs[0].1));
        assert_eq!(span(pairs[0].1), "[x, 'y' ]");
        assert_eq!(span(document.items(pairs[0].1)[1]), "'y'");
        assert_eq!(span(pairs[1].1), "\"z\\\"\"");
        assert_eq!(
            document.scalar_style(pairs[1].1),
            Some(ScalarStyle::DoubleQuoted)
        );
    }

    #[test]
    fn spans_a_block_collection_to_its_last_descendant() {
        let document = parse("a:\n  - b: 1\n    c: é # x\n\nd: &k 2\n", "t.yml", 0).unwrap();
        let root = document.root.unwrap();
        let pairs = document.pairs(root);

        assert!(!document.is_flow(pairs[0].1));
        assert_eq!(
            &document.text()[document.byte_span(pairs[0].1)],
            "- b: 1\n    c: é"
        );
        assert!(document.is_anchored(pairs[1].1));
        assert!(!document.is_anchored(pairs[0].1));
    }

    #[test]
    fn rejects_a_second_document() {
        let error = parse("a: 1\n---\nb: 2\n", "t.yml", 0).unwrap_err();

        assert_eq!(error.message, "invalid YAML (t.yml line 2)");
    }
}
