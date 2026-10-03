//! Loading a document with saphyr's loader, and the rules the loader does not enforce.
//!
//! saphyr's loader builds the positioned tree and types plain scalars by the YAML 1.2 core schema.
//! It tolerates things ambit must not: a duplicate key (the loader keeps the last value), a custom
//! tag, a non-string key. A [`Checker`] stands between the parser and the loader and sees every
//! event first, so those are caught with their lines before the loader merges anything away.
//!
//! Nothing outside `yaml/` depends on these shapes.

use std::borrow::Cow;
use std::collections::HashSet;

use saphyr::{MarkedYamlOwned, ScalarOwned, YamlDataOwned, YamlLoader};
use saphyr_parser::{Event, Parser, ScalarStyle, ScanError, Span, SpannedEventReceiver, Tag};

use crate::errors::{AmbitError, Result, at, config_error};
use crate::util::text::js_trim;

pub(super) type Node = MarkedYamlOwned;

/// The prefix the `!!` handle expands to.
const CORE_PREFIX: &str = "tag:yaml.org,2002:";

/// The tags the YAML 1.2 core schema resolves. A node carrying anything else is using a custom
/// tag, and the document is rejected: arbitrary type resolution is how `!!python/object`
/// constructs get in.
const CORE_TAGS: &[&str] = &[
    "tag:yaml.org,2002:bool",
    "tag:yaml.org,2002:float",
    "tag:yaml.org,2002:int",
    "tag:yaml.org,2002:map",
    "tag:yaml.org,2002:null",
    "tag:yaml.org,2002:seq",
    "tag:yaml.org,2002:str",
];

/// One parsed document: its tree, and everything needed to position an error in it.
#[derive(Debug)]
pub(super) struct Document {
    /// How the document is named in error messages.
    pub file: String,
    text: String,
    /// Lines of the containing file above the parsed text: a frontmatter block's opening delimiter
    /// and any blank lines under it. Needed so a reported line number matches the whole file, not
    /// just the parsed slice.
    line_offset: usize,
    /// Character offsets of every alias. The loader replaces an alias with a copy of the node it
    /// names, but an alias is no supported value, so an accessor that meets one reports it rather
    /// than reading the copy.
    aliases: HashSet<usize>,
    /// Absent for a document holding nothing but whitespace and comments.
    pub root: Option<Node>,
}

impl Document {
    fn is_alias(&self, node: &Node) -> bool {
        self.aliases.contains(&node.span.start.index())
    }

    pub fn scalar<'n>(&self, node: &'n Node) -> Option<&'n ScalarOwned> {
        match &node.data {
            YamlDataOwned::Value(scalar) if !self.is_alias(node) => Some(scalar),
            _ => None,
        }
    }

    /// The string value of a string scalar.
    pub fn string<'n>(&self, node: &'n Node) -> Option<&'n str> {
        match self.scalar(node) {
            Some(ScalarOwned::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn is_null(&self, node: &Node) -> bool {
        matches!(self.scalar(node), Some(ScalarOwned::Null))
    }

    pub fn is_map(&self, node: &Node) -> bool {
        !self.is_alias(node) && matches!(node.data, YamlDataOwned::Mapping(_))
    }

    pub fn items<'n>(&self, node: &'n Node) -> Option<&'n [Node]> {
        match &node.data {
            YamlDataOwned::Sequence(items) if !self.is_alias(node) => Some(items),
            _ => None,
        }
    }

    /// The pairs of a mapping, in document order.
    pub fn pairs<'n>(&self, node: &'n Node) -> Vec<(&'n Node, &'n Node)> {
        match &node.data {
            YamlDataOwned::Mapping(pairs) if !self.is_alias(node) => pairs.iter().collect(),
            _ => Vec::new(),
        }
    }

    /// The 1-based line `node` starts on, counted in the containing file.
    pub fn line_of(&self, node: &Node) -> usize {
        node.span.start.line() + self.line_offset
    }

    /// The source text `node` was parsed from. Messages quote this rather than the parsed value,
    /// so the fix for `ref: 1e5` reads `ref: "1e5"` and not `ref: "100000"`.
    pub fn text_of(&self, node: &Node) -> Option<&str> {
        let start = self.byte_offset(node.span.start.index());
        let end = self.byte_offset(node.span.end.index());
        let slice = js_trim(self.text.get(start..end)?);

        (!slice.is_empty()).then_some(slice)
    }

    /// saphyr's markers count characters, not bytes.
    fn byte_offset(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(byte, _)| byte)
    }

    /// How a value is described in a type-mismatch message.
    pub fn describe(&self, node: &Node) -> &'static str {
        if self.is_alias(node) {
            return UNSUPPORTED;
        }

        match &node.data {
            YamlDataOwned::Mapping(_) => "a mapping",
            YamlDataOwned::Sequence(_) => "a sequence",
            YamlDataOwned::Value(scalar) => describe_scalar(scalar),
            _ => UNSUPPORTED,
        }
    }
}

const UNSUPPORTED: &str = "an unsupported value";

fn describe_scalar(scalar: &ScalarOwned) -> &'static str {
    match scalar {
        ScalarOwned::Null => "null",
        ScalarOwned::Boolean(_) => "a boolean",
        ScalarOwned::Integer(_) => "an integer",
        ScalarOwned::FloatingPoint(n) if is_integer(n.0) => "an integer",
        ScalarOwned::FloatingPoint(_) => "a number",
        ScalarOwned::String(_) => "a string",
    }
}

/// `Number.isInteger`.
pub(super) fn is_integer(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0
}

/// How the loader types a scalar. An empty plain scalar (`name:` with nothing after it) is null,
/// as the core schema has it; saphyr's typing alone would read it as an empty string.
fn resolve(
    value: Cow<'_, str>,
    style: ScalarStyle,
    tag: Option<&Cow<'_, Tag>>,
) -> Option<ScalarOwned> {
    if value.is_empty() && style == ScalarStyle::Plain && tag.is_none() {
        return Some(ScalarOwned::Null);
    }

    ScalarOwned::parse_from_cow_and_metadata(value, style, tag)
}

/// What a collection being loaded still expects.
enum Frame {
    Seq,
    /// The keys seen so far with their lines, and whether the next node is a value rather than a
    /// key.
    Map {
        keys: Vec<(String, usize)>,
        awaiting_value: bool,
    },
}

/// Inspects each event for the rules the loader does not enforce, then hands it on.
struct Checker<'input> {
    loader: YamlLoader<'input, Node>,
    file: String,
    line_offset: usize,
    stack: Vec<Frame>,
    aliases: HashSet<usize>,
    /// The first violation, in document order.
    problem: Option<AmbitError>,
}

impl Checker<'_> {
    fn line(&self, span: Span) -> usize {
        span.start.line() + self.line_offset
    }

    fn report(&mut self, problem: AmbitError) {
        self.problem.get_or_insert(problem);
    }

    fn check_tag(&mut self, tag: Option<&Cow<'_, Tag>>, span: Span) {
        let Some(tag) = tag else {
            return;
        };
        let expanded = format!("{}{}", tag.handle, tag.suffix);

        if CORE_TAGS.contains(&expanded.as_str()) {
            return;
        }

        let shorthand = match expanded.strip_prefix(CORE_PREFIX) {
            Some(suffix) => format!("!!{suffix}"),
            None => expanded,
        };

        self.report(config_error(
            format!(
                "custom YAML tag `{shorthand}` is not permitted {}",
                at(&self.file, Some(self.line(span)))
            ),
            [
                "ambit parses YAML 1.2 with the core schema only".to_owned(),
                format!("remove `{shorthand}`"),
            ],
        ));
    }

    /// Records a node starting. When it is a mapping key, checks it is a string and not a
    /// duplicate.
    ///
    /// `key` is the node's string value when it is a string scalar, otherwise how it is
    /// described.
    fn node(&mut self, key: std::result::Result<String, &'static str>, span: Span) {
        let line = self.line(span);
        let Some(Frame::Map {
            keys,
            awaiting_value,
        }) = self.stack.last_mut()
        else {
            return;
        };

        *awaiting_value = !*awaiting_value;

        if !*awaiting_value {
            return;
        }

        let name = match key {
            Ok(name) => name,
            Err(found) => {
                let problem = config_error(
                    format!(
                        "mapping keys must be strings {}",
                        at(&self.file, Some(line))
                    ),
                    [
                        format!("found {found} as a key"),
                        "quote the key".to_owned(),
                    ],
                );

                return self.report(problem);
            }
        };

        let Some(&(_, first)) = keys.iter().find(|(seen, _)| *seen == name) else {
            keys.push((name, line));
            return;
        };

        let problem = config_error(
            format!("duplicate key \"{name}\" {}", at(&self.file, Some(line))),
            [
                format!("first defined on line {first}"),
                "remove one of the two definitions".to_owned(),
            ],
        );

        self.report(problem);
    }
}

impl<'input> SpannedEventReceiver<'input> for Checker<'input> {
    fn on_event(&mut self, event: Event<'input>, span: Span) {
        let event = match event {
            Event::Scalar(value, style, anchor, tag) => {
                self.check_tag(tag.as_ref(), span);

                let key = match resolve(value.clone(), style, tag.as_ref()) {
                    Some(ScalarOwned::String(text)) => Ok(text),
                    Some(scalar) => Err(describe_scalar(&scalar)),
                    None => Err(UNSUPPORTED),
                };

                self.node(key, span);

                // The loader would type an empty plain scalar as an empty string.
                if value.is_empty() && style == ScalarStyle::Plain && tag.is_none() {
                    Event::Scalar("~".into(), style, anchor, tag)
                } else {
                    Event::Scalar(value, style, anchor, tag)
                }
            }
            Event::SequenceStart(anchor, tag) => {
                self.check_tag(tag.as_ref(), span);
                self.node(Err("a sequence"), span);
                self.stack.push(Frame::Seq);

                Event::SequenceStart(anchor, tag)
            }
            Event::MappingStart(anchor, tag) => {
                self.check_tag(tag.as_ref(), span);
                self.node(Err("a mapping"), span);
                self.stack.push(Frame::Map {
                    keys: Vec::new(),
                    awaiting_value: false,
                });

                Event::MappingStart(anchor, tag)
            }
            Event::Alias(id) => {
                self.aliases.insert(span.start.index());
                self.node(Err(UNSUPPORTED), span);

                Event::Alias(id)
            }
            Event::SequenceEnd | Event::MappingEnd => {
                self.stack.pop();

                event
            }
            other => other,
        };

        self.loader.on_event(event, span);
    }
}

/// Parses `text` into a positioned tree, rejecting a syntax error, a second document, or a node
/// the loader would accept and ambit does not.
///
/// `line_offset` is the number of lines of the containing file above `text`, for a frontmatter
/// block.
///
/// # Errors
///
/// Exit 2 for a syntax error, a tab used as indentation, more than one document, a custom tag, a
/// non-string key, or a duplicate key.
pub(super) fn parse(text: &str, file: &str, line_offset: usize) -> Result<Document> {
    let mut parser = Parser::new_from_str(text);
    let mut checker = Checker {
        loader: YamlLoader::default(),
        file: file.to_owned(),
        line_offset,
        stack: Vec::new(),
        aliases: HashSet::new(),
        problem: None,
    };
    let syntax = |error: &ScanError| syntax_error(text, file, line_offset, error);

    parser.load(&mut checker, false).map_err(|e| syntax(&e))?;

    // Only the first document was loaded. Anything after it but the end of the stream is either a
    // second document or a syntax error.
    match parser.next() {
        Some(Err(error)) => return Err(syntax(&error)),
        Some(Ok((Event::DocumentStart(_), span))) => {
            return Err(config_error(
                format!(
                    "invalid YAML {}",
                    at(file, Some(span.start.line() + line_offset))
                ),
                ["Source contains multiple documents", "fix the syntax error"],
            ));
        }
        _ => {}
    }

    if let Some(problem) = checker.problem {
        return Err(problem);
    }

    let root = checker
        .loader
        .into_documents()
        .into_iter()
        .next()
        .filter(|root| !matches!(root.data, YamlDataOwned::BadValue));

    Ok(Document {
        file: file.to_owned(),
        text: text.to_owned(),
        line_offset,
        aliases: checker.aliases,
        root,
    })
}

/// Rewrites a parser error into ambit's message shape, keeping the position.
fn syntax_error(text: &str, file: &str, line_offset: usize, error: &ScanError) -> AmbitError {
    let line = error_line(text, error);
    let where_ = at(file, Some(line + line_offset));

    if is_tab_as_indent(text, error) {
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

/// The line an error is reported on, moved to where the problem is in the cases where saphyr
/// notices it later.
///
/// - A source that ends mid-construct (an unterminated quote, an unclosed flow collection) is
///   reported on its last line, not past it: saphyr's marker can sit one line beyond a source with
///   no final newline, and points at the opening quote of an unterminated scalar.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn root(text: &str) -> (Document, Node) {
        let mut document = parse(text, "t.yml", 0).unwrap();
        let root = document.root.take().unwrap();

        (document, root)
    }

    #[test]
    fn positions_nodes_by_character_and_one_based_line() {
        // saphyr's marker index counts characters, not bytes; the slice must still land on "ab".
        let (document, root) = root("é: ab\nx: 'q'\n");
        let pairs = document.pairs(&root);

        assert_eq!(document.text_of(pairs[0].1), Some("ab"));
        assert_eq!(document.line_of(pairs[0].1), 1);
        assert_eq!(document.text_of(pairs[1].1), Some("'q'"));
        assert_eq!(document.line_of(pairs[1].0), 2);
    }

    #[test]
    fn positions_a_nested_block_mapping_at_its_first_key() {
        let (document, root) = root("a:\n  b: 1\n");

        assert_eq!(document.line_of(&root), 1);
        assert_eq!(document.line_of(document.pairs(&root)[0].1), 2);
    }
}
