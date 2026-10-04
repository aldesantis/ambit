//! The managed `.gitignore` blocks.
//!
//! Everything ambit materializes is derived (copied or linked from a catalog), so committing it
//! would give a project two answers to "what is installed": `ambit.yml` and git. Ambit writes the
//! paths it owns into `.gitignore` itself.
//!
//! Two files: `.agents/.gitignore` lists installed skills (the layout dotagents established; this
//! list churns as `requires` changes), and the root `.gitignore` holds what a nested file cannot
//! express (`.ambit/` and the skills link, `.claude/skills`, owned by the harness), since git only
//! matches a pattern against paths beneath the file that holds it.
//!
//! `.agents/.gitignore` is tracked, not ignored: it is generated from `ambit.yml` and
//! `ambit.lock`, both committed, and is byte-stable, so a fresh clone or worktree gets the ignore
//! list without running ambit first.
//!
//! Ambit owns a block inside each file, marked by sentinel lines; everything outside a block is
//! left untouched, so a pre-existing `.gitignore` is a normal input. The block has no entry in
//! `.ambit/state.json` and needs no prune step: each install rewrites it from scratch. `clean`
//! removes the blocks using the same markers.
//!
//! A file with two blocks, or one missing its end marker, is ambiguous and fails with exit 2 rather
//! than guessing which lines to overwrite.

use std::path::Path;

use indexmap::IndexSet;

use crate::errors::{Result, config_error};
use crate::harness::profile::SHARED_AGENTS_DIR;
use crate::model::state::{ArtifactKind, OwnedArtifact, STATE_DIRNAME};
use crate::util::cmp::js_cmp;
use crate::util::fs;
use crate::util::path::join;
use crate::util::text::js_trim;

/// The file the root block lives in, at the project root.
pub const GITIGNORE_FILENAME: &str = ".gitignore";

/// The file the skill paths are listed in, inside the directory that holds them.
pub const SHARED_GITIGNORE_FILE: &str = ".agents/.gitignore";

/// The sentinel a block's first line starts with.
pub const BLOCK_BEGIN: &str = "# BEGIN ambit";

/// The sentinel a block's last line starts with, and the whole of that line as written.
pub const BLOCK_END: &str = "# END ambit";

/// The opening line as written.
///
/// The explanation is part of the marker line, not a line of its own, so a block adds only one
/// line of prose regardless of how many paths it lists. Detection matches the sentinel prefix, not
/// this exact string, so an older block with different wording is still recognized and rewritten.
const BEGIN_LINE: &str =
    "# BEGIN ambit - managed block, rewritten by `ambit install`; edits are lost";

/// One file ambit maintains a block in, and what that block should list.
///
/// Entries are patterns relative to the file's own directory, since that is what git matches them
/// against; splitting between the two files also rewrites the paths, not just partitions them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoreBlock {
    /// Project-relative path of the file holding the block.
    pub file: String,
    /// The patterns the block should list. Empty means the block should not be there at all.
    pub entries: Vec<String>,
}

/// Whether one managed block is already what an install would write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitignoreStatus {
    /// Project-relative path of the file holding the block.
    pub file: String,
    /// Whether install would rewrite it.
    pub changed: bool,
}

/// A `.gitignore` line is a glob, so characters that would change what it matches get escaped.
///
/// This is nearly always a no-op for a skill name, but a pattern that decides what git tracks
/// should not rely on "nearly". A leading `#` or `!` needs no escape here because every path ambit
/// writes begins with a directory component.
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

/// The lines of a file, without the empty segment a trailing newline leaves behind.
///
/// Splits on `\n` alone so a `\r` stays part of its line and is written back unchanged; a CRLF
/// `.gitignore` should not be rewritten wholesale just because ambit changed a few lines in the
/// middle.
fn split_lines(text: Option<&str>) -> Vec<String> {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Vec::new();
    };

    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();

    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    lines
}

/// Where ambit's block sits, by line index, inclusive of both markers.
struct Block {
    start: usize,
    end: usize,
}

/// Locates the managed block, or reports that the file holds none.
///
/// `file` is the project-relative path, so an error names the file to fix rather than whichever of
/// the two ambit happened to be writing.
///
/// # Errors
///
/// Exit 2 for two blocks, or a block with no end line, since either makes the span ambit may
/// overwrite ambiguous.
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

/// A path inside the shared directory, rewritten as a pattern anchored to that directory.
///
/// The leading `/` is required: without it git would match the pattern at any depth below
/// `.agents/`, so a skill named `skills` would ignore unrelated paths.
fn shared_pattern(artifact_path: &str) -> String {
    format!("/{}", &artifact_path[SHARED_AGENTS_DIR.len() + 1..])
}

/// Whether a path is one the nested file can express: anything under the shared directory.
fn is_shared(artifact_path: &str) -> bool {
    artifact_path
        .strip_prefix(SHARED_AGENTS_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// The two blocks a project should hold, given what an install owns.
///
/// Every installed skill, and every script a hook ships, lands under the shared directory and is
/// listed in the nested file. Ambit's state directory and the skills link cannot be reached from
/// there and stay at the root. The split is by path, not by artifact kind, so a harness that later
/// puts a skills link inside `.agents/` needs no change here.
///
/// Every kind ambit owns as a path must be listed here, or its bytes show up as untracked in `git
/// status`. `.mcp.json` and `ambit.lock` are not listed: teams may want them committed, and
/// `ambit.lock` is not an owned artifact. `.agents/.gitignore` is generated but tracked (see the
/// module header), so it is not listed either.
///
/// `artifacts` is what the install owns: the applied artifacts, or a state file's.
pub fn gitignore_blocks(artifacts: &[OwnedArtifact]) -> Vec<IgnoreBlock> {
    let mut root = vec![format!("{STATE_DIRNAME}/")];
    let mut shared = Vec::new();

    for artifact in artifacts {
        if artifact.kind == ArtifactKind::HarnessConfig {
            continue;
        }

        // No trailing slash: a `path:` skill installs as a symlink, which git does not read as a
        // directory, so a `dir/` pattern would leave linked skills tracked. The skills link is
        // also always a symlink and needs the same fix.
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

/// The `.gitignore` a project should hold, given the one it holds now.
///
/// `existing` is the current contents, or `None` when there is no file yet. `entries` are the
/// paths to list, in any order; empty renders no block, which for the nested file is the ordinary
/// state of a project that installed no skills. `file` is the project-relative path, for the error
/// naming an ambiguous block.
///
/// Returns the new contents, or `None` when nothing would change (so a caller skips the write,
/// keeping a second identical install byte-identical). `Some("")` when the block was the whole
/// file and is now gone, telling a caller to delete the file.
///
/// # Errors
///
/// Exit 2 for a file whose markers cannot be read unambiguously.
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
            // Add one blank line of separation, but only if there is something to separate; a
            // file that already ends in a blank line keeps its own shape.
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

/// The `.gitignore` a project should hold once ambit owns nothing in it: what `clean` writes.
///
/// The blank line above the block is removed too, since [`update_gitignore_text`] is what added
/// it; otherwise `install` followed by `clean` would not return a hand-written file to its original
/// bytes. The edge case is a file whose author already ended it with a blank line of their own,
/// which then gets removed along with ambit's.
///
/// Returns the new contents; `Some("")` when ambit's block was the whole file (caller should delete
/// it); `None` when there is no block to remove.
///
/// # Errors
///
/// Exit 2 for a file whose markers cannot be read unambiguously.
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

/// Reads one of a project's `.gitignore` files, treating an absent one as a file ambit is about to
/// create. `file` is the project-relative path, usually [`GITIGNORE_FILENAME`].
///
/// # Errors
///
/// Exit 2 for a file that is there but cannot be read; overwriting it would discard lines ambit
/// does not own.
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

/// Writes one file's block, or deletes the file when the block was all of it. Returns whether
/// anything was written.
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

    // The nested file sits in a directory the install creates, but `prune` and a bundle that
    // installs no skills both reach here without one.
    if let Some(parent) = target.parent() {
        fs::mkdir_p(parent)?;
    }

    fs::write_text(&target, &next)?;

    Ok(true)
}

/// Rewrites the managed blocks for what an install just wrote. Returns the files that changed, in
/// the order they are written.
///
/// # Errors
///
/// Exit 2 for a `.gitignore` that cannot be read, or whose markers are ambiguous.
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

/// Whether each managed block is already what an install would write.
///
/// Used by `install --dry-run` and `doctor`, both of which ask the same renderer that writes:
/// [`update_gitignore_text`] returning `None` means the file would not change.
///
/// # Errors
///
/// Exit 2 for a `.gitignore` that cannot be read, or whose markers are ambiguous.
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

/// Takes ambit's blocks out of a project's `.gitignore` files. Used by `clean`.
///
/// A file that is nothing but the block is deleted rather than truncated: ambit created it, so
/// leaving an empty one behind would leave a file the project never had. Returns the files a block
/// was removed from.
///
/// # Errors
///
/// Exit 2 for a `.gitignore` that cannot be read, or whose markers are ambiguous.
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
