use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use crate::errors::{Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::util::json::{JsonObject, JsonValue, format_number, js_key_order};
use crate::util::text::{js_trim, js_trim_start};

static BARE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]+$").expect("valid regex"));

static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(\[\[?)([^\]]*)(\]\]?)\s*(?:#[^\n\r\u{2028}\u{2029}]*)?$")
        .expect("valid regex")
});

static ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([^=\s#\[][^=]*?)\s*=").expect("valid regex"));

static BASIC_SEGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^"((?:[^"\\]|\\.)*)"\s*"#).expect("valid regex"));

static LITERAL_SEGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^'([^']*)'\s*").expect("valid regex"));

static BARE_SEGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_-]+)\s*").expect("valid regex"));

struct Header {
    line: usize,
    path: Vec<String>,
}

fn quote_key(key: &str) -> String {
    if BARE_KEY.is_match(key) {
        key.to_owned()
    } else {
        toml_string(key)
    }
}

fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\u{8}', "\\b")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\u{c}', "\\f")
        .replace('\r', "\\r");

    format!("\"{escaped}\"")
}

fn toml_value(value: &JsonValue, file: &str) -> Result<String> {
    match value {
        JsonValue::String(s) => Ok(toml_string(s)),
        JsonValue::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
        JsonValue::Number(n) => Ok(format_number(n)),
        JsonValue::Array(items) => {
            let rendered = items
                .iter()
                .map(|item| toml_value(item, file))
                .collect::<Result<Vec<_>>>()?;

            Ok(format!("[{}]", rendered.join(", ")))
        }
        JsonValue::Null | JsonValue::Object(_) => Err(config_error(
            format!("cannot render a value for {file} as TOML"),
            [
                format!(
                    "unsupported value type: {}",
                    if value.is_null() { "null" } else { "object" }
                ),
                "this is a bug in ambit; please report it".to_owned(),
            ],
        )),
    }
}

fn render_table(path: &[String], table: &JsonObject, file: &str) -> Result<Vec<String>> {
    let header = format!(
        "[{}]",
        path.iter()
            .map(|segment| quote_key(segment))
            .collect::<Vec<_>>()
            .join(".")
    );
    let mut scalars = Vec::new();
    let mut nested = Vec::new();

    for key in js_key_order(table) {
        let value = &table[key.as_str()];

        if let JsonValue::Object(sub) = value {
            let mut sub_path = path.to_vec();
            sub_path.push(key.clone());
            nested.push(String::new());
            nested.extend(render_table(&sub_path, sub, file)?);
        } else {
            scalars.push(format!("{} = {}", quote_key(key), toml_value(value, file)?));
        }
    }

    let mut lines = vec![header];
    lines.extend(scalars);
    lines.extend(nested);

    Ok(lines)
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(next) = chars.next()
        {
            out.push(next);
            continue;
        }

        out.push(c);
    }

    out
}

fn split_header_path(inner: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut rest = js_trim(inner);

    while !rest.is_empty() {
        let consumed = if rest.starts_with('"') {
            let captures = BASIC_SEGMENT.captures(rest)?;
            segments.push(unescape(&captures[1]));
            captures[0].len()
        } else if rest.starts_with('\'') {
            let captures = LITERAL_SEGMENT.captures(rest)?;
            segments.push(captures[1].to_owned());
            captures[0].len()
        } else {
            let captures = BARE_SEGMENT.captures(rest)?;
            segments.push(captures[1].to_owned());
            captures[0].len()
        };

        rest = &rest[consumed..];

        if let Some(after) = rest.strip_prefix('.') {
            rest = js_trim_start(after);
        } else if !rest.is_empty() {
            return None;
        }
    }

    (!segments.is_empty()).then_some(segments)
}

fn headers(lines: &[String], section: &str, file: &str) -> Result<Vec<Header>> {
    let mut found = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let Some(captures) = HEADER.captures(line) else {
            continue;
        };

        let Some(path) = split_header_path(&captures[2]) else {
            continue;
        };

        if path[0] != section {
            continue;
        }

        if &captures[1] == "[[" {
            return Err(config_error(
                format!("cannot edit \"{section}\" in {file}"),
                [
                    format!(
                        "line {} declares `{section}` as an array of tables",
                        index + 1
                    ),
                    format!("rewrite it as `[{section}.<name>]` tables, or move the file aside"),
                ],
            ));
        }

        found.push(Header { line: index, path });
    }

    Ok(found)
}

fn assert_editable(lines: &[String], section: &str, file: &str) -> Result<()> {
    let mut context: Vec<String> = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if let Some(header) = HEADER.captures(line) {
            context = split_header_path(&header[2]).unwrap_or_else(|| vec![" ".to_owned()]);
            continue;
        }

        let Some(assignment) = ASSIGNMENT.captures(line) else {
            continue;
        };

        let Some(key) = split_header_path(&assignment[1]) else {
            continue;
        };

        let names: Vec<String> = if context.is_empty() {
            key
        } else if context.len() == 1 && context[0] == section {
            std::iter::once(section.to_owned()).chain(key).collect()
        } else {
            Vec::new()
        };

        if names.first().map(String::as_str) != Some(section) {
            continue;
        }

        return Err(config_error(
            format!("cannot edit \"{section}\" in {file}"),
            [
                format!(
                    "line {} sets `{}` outside a `[{section}.<name>]` table",
                    index + 1,
                    names.join(".")
                ),
                format!("rewrite it as a `[{section}.<name>]` table, or move the file aside"),
            ],
        ));
    }

    Ok(())
}

fn is_under(path: &[String], prefix: &[String]) -> bool {
    path.len() > prefix.len() && path.starts_with(prefix)
}

fn span_of(lines: &[String], all: &[Header], at: usize) -> (usize, usize) {
    let Some(server) = all.get(at) else {
        return (0, 0);
    };

    let prefix = &server.path[..server.path.len().min(2)];
    let mut end = lines.len();

    for other in &all[at + 1..] {
        if !is_under(&other.path, prefix) {
            end = other.line;
            break;
        }
    }

    for (index, line) in lines[server.line + 1..end].iter().enumerate() {
        let Some(captures) = HEADER.captures(line) else {
            continue;
        };

        let under = split_header_path(&captures[2]).is_some_and(|path| is_under(&path, prefix));

        if !under {
            end = server.line + 1 + index;
            break;
        }
    }

    while end > server.line + 1 && js_trim(&lines[end - 1]).is_empty() {
        end -= 1;
    }

    (server.line, end)
}

fn server_headers(lines: &[String], section: &str, file: &str) -> Result<IndexMap<String, usize>> {
    let mut found = IndexMap::new();

    for (index, header) in headers(lines, section, file)?.into_iter().enumerate() {
        if header.path.len() != 2 {
            continue;
        }

        let name = header
            .path
            .into_iter()
            .nth(1)
            .expect("the path has two segments");

        found.entry(name).or_insert(index);
    }

    Ok(found)
}

fn split(text: &str) -> (Vec<String>, &'static str) {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let pieces: Vec<&str> = text.split('\n').collect();
    let last = pieces.len() - 1;
    let mut lines: Vec<String> = pieces
        .into_iter()
        .enumerate()
        .map(|(index, piece)| {
            if index < last {
                piece.strip_suffix('\r').unwrap_or(piece).to_owned()
            } else {
                piece.to_owned()
            }
        })
        .collect();

    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    (lines, newline)
}

fn join(lines: &[String], newline: &str) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}{newline}", lines.join(newline))
    }
}

fn is_blank(line: Option<&String>) -> bool {
    js_trim(line.map_or("", String::as_str)).is_empty()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TomlDriver;

impl DocumentDriver for TomlDriver {
    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        let Some(text) = text else {
            return Ok(IndexSet::new());
        };

        let (lines, _) = split(text);

        Ok(server_headers(&lines, section, file)?.into_keys().collect())
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let (lines, newline) = split(text.unwrap_or(""));

        assert_editable(&lines, section, file)?;

        let mut current = lines;

        for entry in entries {
            let JsonValue::Object(table) = &entry.value else {
                return Err(config_error(
                    format!(
                        "cannot render \"{section}.{}\" for {file} as TOML",
                        entry.key
                    ),
                    [
                        "a managed entry must be a table of keys",
                        "this is a bug in ambit; please report it",
                    ],
                ));
            };

            let rendered = render_table(&[section.to_owned(), entry.key.clone()], table, file)?;
            let present = server_headers(&current, section, file)?;

            if let Some(&at) = present.get(&entry.key) {
                let (start, end) = span_of(&current, &headers(&current, section, file)?, at);

                current.splice(start..end, rendered);
            } else {
                if !current.is_empty() && !is_blank(current.last()) {
                    current.push(String::new());
                }

                current.extend(rendered);
            }
        }

        Ok(join(&current, newline))
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let Some(text) = text else {
            return Ok(false);
        };

        Ok(self.merge_section(Some(text), section, std::slice::from_ref(entry), file)? == text)
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let Some(text) = text else {
            return Ok(None);
        };

        let (mut current, newline) = split(text);
        let mut removed = false;

        for key in keys {
            let present = server_headers(&current, section, file)?;

            let Some(&at) = present.get(key) else {
                continue;
            };

            let (start, end) = span_of(&current, &headers(&current, section, file)?, at);
            let from = if start > 0 && is_blank(current.get(start - 1)) {
                start - 1
            } else {
                start
            };
            let to = if from == 0 && is_blank(current.get(end)) {
                (end + 1).min(current.len())
            } else {
                end
            };

            current.drain(from..to);
            removed = true;
        }

        Ok(removed.then(|| join(&current, newline)))
    }
}

#[cfg(test)]
mod tests;
