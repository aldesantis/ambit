//! Shaping for command output.
//!
//! Both modes must be deterministic: `--json` is compared as a golden file, and the text form is
//! diffed by hand between runs. JSON records are built in the order given, not by object-key
//! order, and text sections are laid out from one place so `search` and `resolve` look consistent.

use crate::cli::Io;
use crate::util::json::{JsonObject, JsonValue};
use crate::util::text::{js_len, js_trim_end, pad_end};

/// A record with the keys in the order given, so the emitted JSON is byte-stable.
pub fn keyed<T>(
    items: &[T],
    name: impl Fn(&T) -> String,
    value: impl Fn(&T) -> JsonValue,
) -> JsonObject {
    let mut record = JsonObject::new();

    for item in items {
        record.insert(name(item), value(item));
    }

    record
}

/// Pads every column but the last, so the eye can run down a section.
pub fn columns<R: AsRef<[String]>>(rows: &[R]) -> Vec<String> {
    let mut widths: Vec<usize> = Vec::new();

    for row in rows {
        for (index, cell) in row.as_ref().iter().enumerate() {
            if widths.len() <= index {
                widths.resize(index + 1, 0);
            }

            widths[index] = widths[index].max(js_len(cell));
        }
    }

    rows.iter()
        .map(|row| {
            let row = row.as_ref();
            let line = row
                .iter()
                .enumerate()
                .map(|(index, cell)| {
                    if index == row.len() - 1 {
                        cell.clone()
                    } else {
                        pad_end(cell, widths[index])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ");

            js_trim_end(&line).to_owned()
        })
        .collect()
}

/// A titled, counted, indented block, closed by a blank line.
pub fn section<R: AsRef<[String]>>(title: &str, rows: &[R]) -> Vec<String> {
    let body = if rows.is_empty() {
        vec!["(none)".to_owned()]
    } else {
        columns(rows)
    };
    let mut lines = Vec::with_capacity(body.len() + 2);

    lines.push(format!("{title} ({})", rows.len()));
    lines.extend(body.into_iter().map(|line| format!("  {line}")));
    lines.push(String::new());
    lines
}

/// Prints section lines, dropping the trailing blank line that closes the last one: it separates
/// sections from each other, not the output from the shell prompt.
pub fn print_sections(lines: &[String], io: &mut dyn Io) {
    for line in &lines[..lines.len().saturating_sub(1)] {
        io.stdout(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CaptureIo;
    use serde_json::json;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|&cell| cell.to_owned()).collect()
    }

    #[test]
    fn keeps_keys_in_the_order_given() {
        let record = keyed(&["b", "a"], |s| (*s).to_owned(), |s| json!(s.len()));

        assert_eq!(record.keys().collect::<Vec<_>>(), ["b", "a"]);
    }

    #[test]
    fn pads_all_but_the_last_column_and_trims() {
        let lines = columns(&[row(&["a", "bb", ""]), row(&["ccc", "d", "e"])]);

        assert_eq!(lines, ["a    bb", "ccc  d   e"]);
    }

    #[test]
    fn renders_sections_with_counts_and_none() {
        assert_eq!(
            section::<Vec<String>>("skills", &[]),
            ["skills (0)", "  (none)", ""]
        );
        assert_eq!(
            section("mcps", &[row(&["x", "y"])]),
            ["mcps (1)", "  x  y", ""]
        );
    }

    #[test]
    fn drops_the_closing_blank_line() {
        let mut io = CaptureIo::default();
        let mut lines = section("a", &[row(&["x"])]);
        lines.extend(section("b", &[row(&["y"])]));

        print_sections(&lines, &mut io);

        assert_eq!(io.out, ["a (1)", "  x", "", "b (1)", "  y"]);
    }
}
