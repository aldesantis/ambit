use std::path::Path;

use indexmap::IndexSet;

use crate::errors::{Result, config_error};
use crate::harness::profile::SHARED_AGENTS_DIR;
use crate::model::state::{ArtifactKind, OwnedArtifact, STATE_DIRNAME};
use crate::util::cmp::js_cmp;
use crate::util::fs;
use crate::util::path::join;
use crate::util::text::js_trim;

pub const GITIGNORE_FILENAME: &str = ".gitignore";

pub const SHARED_GITIGNORE_FILE: &str = ".agents/.gitignore";

pub const BLOCK_BEGIN: &str = "# BEGIN ambit";

pub const BLOCK_END: &str = "# END ambit";

const BEGIN_LINE: &str =
    "# BEGIN ambit - managed block, rewritten by `ambit install`; edits are lost";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoreBlock {
    pub file: String,
    pub entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitignoreStatus {
    pub file: String,
    pub changed: bool,
}

fn escape_pattern(entry: &str) -> String {
    let mut escaped = String::with_capacity(entry.len());

    for character in entry.chars() {
        if matches!(character, '\\' | '*' | '?' | '[') {
            escaped.push('\\');
        }

        escaped.push(character);
    }

    escaped
}

fn is_begin(line: &str) -> bool {
    js_trim(line).starts_with(BLOCK_BEGIN)
}

fn is_end(line: &str) -> bool {
    js_trim(line).starts_with(BLOCK_END)
}

fn split_lines(text: Option<&str>) -> Vec<String> {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Vec::new();
    };

    // Not `lines()`: a `\r` must stay in its line so a CRLF file round-trips unchanged.
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();

    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    lines
}

struct Block {
    start: usize,
    end: usize,
}

fn find_block(lines: &[String], file: &str) -> Result<Option<Block>> {
    let mut begins = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_begin(line))
        .map(|(index, _)| index);

    let Some(start) = begins.next() else {
        return Ok(None);
    };

    if let Some(second) = begins.next() {
        return Err(config_error(
            format!("{file} holds more than one ambit block"),
            [
                format!(
                    "`{BLOCK_BEGIN}` appears on line {} and line {}, so ambit cannot tell which block is its own",
                    start + 1,
                    second + 1
                ),
                "delete all but one of them, keeping the paths you want, then run `ambit install` again"
                    .to_owned(),
            ],
        ));
    }

    let Some(offset) = lines[start + 1..].iter().position(|line| is_end(line)) else {
        return Err(config_error(
            format!("{file} holds an unterminated ambit block"),
            [
                format!(
                    "the block opened on line {} has no `{BLOCK_END}` line, so ambit cannot tell where it ends",
                    start + 1
                ),
                format!(
                    "add `{BLOCK_END}` after the last path ambit wrote, or delete line {}",
                    start + 1
                ),
            ],
        ));
    };

    Ok(Some(Block {
        start,
        end: start + 1 + offset,
    }))
}

fn shared_pattern(artifact_path: &str) -> String {
    // Anchored: without the `/`, git matches the pattern at any depth below `.agents/`.
    format!("/{}", &artifact_path[SHARED_AGENTS_DIR.len() + 1..])
}

fn is_shared(artifact_path: &str) -> bool {
    artifact_path
        .strip_prefix(SHARED_AGENTS_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

pub fn gitignore_blocks(artifacts: &[OwnedArtifact]) -> Vec<IgnoreBlock> {
    let mut root = vec![format!("{STATE_DIRNAME}/")];
    let mut shared = Vec::new();

    for artifact in artifacts {
        if artifact.kind == ArtifactKind::HarnessConfig {
            continue;
        }

        // No trailing slash: git does not treat a symlink as a directory.
        if is_shared(&artifact.path) {
            shared.push(shared_pattern(&artifact.path));
        } else {
            root.push(artifact.path.clone());
        }
    }

    vec![
        IgnoreBlock {
            file: GITIGNORE_FILENAME.to_owned(),
            entries: root,
        },
        IgnoreBlock {
            file: SHARED_GITIGNORE_FILE.to_owned(),
            entries: shared,
        },
    ]
}

pub fn update_gitignore_text(
    existing: Option<&str>,
    entries: &[String],
    file: &str,
) -> Result<Option<String>> {
    if entries.is_empty() {
        return remove_gitignore_text(existing, file);
    }

    let mut lines = split_lines(existing);
    let block = find_block(&lines, file)?;
    let mut unique: Vec<&String> = entries
        .iter()
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();

    unique.sort_by(|a, b| js_cmp(a, b));

    let mut rendered = vec![BEGIN_LINE.to_owned()];

    rendered.extend(unique.into_iter().map(|entry| escape_pattern(entry)));
    rendered.push(BLOCK_END.to_owned());

    match block {
        None => {
            if lines.last().is_some_and(|line| !js_trim(line).is_empty()) {
                lines.push(String::new());
            }

            lines.extend(rendered);
        }
        Some(block) => {
            lines.splice(block.start..=block.end, rendered);
        }
    }

    let text = format!("{}\n", lines.join("\n"));

    Ok(if Some(text.as_str()) == existing {
        None
    } else {
        Some(text)
    })
}

pub fn remove_gitignore_text(existing: Option<&str>, file: &str) -> Result<Option<String>> {
    let mut lines = split_lines(existing);
    let Some(block) = find_block(&lines, file)? else {
        return Ok(None);
    };

    let start = if block.start > 0 && js_trim(&lines[block.start - 1]).is_empty() {
        block.start - 1
    } else {
        block.start
    };

    lines.drain(start..=block.end);

    Ok(Some(if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }))
}

pub fn read_gitignore_text(project_dir: &Path, file: &str) -> Result<Option<String>> {
    let target = join(project_dir, file);

    fs::read_text_opt(&target).map_err(|error| {
        config_error(
            format!("cannot read {file}"),
            [
                fs::io_message(&error, &target),
                format!(
                    "make {} readable, so ambit can rewrite its own block without discarding the rest",
                    target.display()
                ),
            ],
        )
    })
}

fn apply_block(project_dir: &Path, block: &IgnoreBlock) -> Result<bool> {
    let existing = read_gitignore_text(project_dir, &block.file)?;
    let Some(next) = update_gitignore_text(existing.as_deref(), &block.entries, &block.file)?
    else {
        return Ok(false);
    };

    let target = join(project_dir, &block.file);

    if next.is_empty() {
        fs::rm_rf(&target)?;

        return Ok(true);
    }

    if let Some(parent) = target.parent() {
        fs::mkdir_p(parent)?;
    }

    fs::write_text(&target, &next)?;

    Ok(true)
}

pub fn write_gitignore_blocks(
    project_dir: &Path,
    artifacts: &[OwnedArtifact],
) -> Result<Vec<String>> {
    let mut written = Vec::new();

    for block in gitignore_blocks(artifacts) {
        if apply_block(project_dir, &block)? {
            written.push(block.file);
        }
    }

    Ok(written)
}

pub fn gitignore_status(
    project_dir: &Path,
    artifacts: &[OwnedArtifact],
) -> Result<Vec<GitignoreStatus>> {
    let mut rows = Vec::new();

    for block in gitignore_blocks(artifacts) {
        let existing = read_gitignore_text(project_dir, &block.file)?;
        let next = update_gitignore_text(existing.as_deref(), &block.entries, &block.file)?;

        rows.push(GitignoreStatus {
            file: block.file,
            changed: next.is_some(),
        });
    }

    Ok(rows)
}

pub fn remove_gitignore_blocks(project_dir: &Path) -> Result<Vec<String>> {
    let mut removed = Vec::new();

    for file in [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE] {
        let existing = read_gitignore_text(project_dir, file)?;
        let Some(next) = remove_gitignore_text(existing.as_deref(), file)? else {
            continue;
        };

        let target = join(project_dir, file);

        if next.is_empty() {
            fs::rm_rf(&target)?;
        } else {
            fs::write_text(&target, &next)?;
        }

        removed.push(file.to_owned());
    }

    Ok(removed)
}

#[cfg(test)]
mod tests;
