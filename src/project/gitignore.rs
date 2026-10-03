//! The `# BEGIN ambit` / `# END ambit` blocks ambit maintains in a project's `.gitignore` files.

use std::path::Path;

use crate::errors::Result;
use crate::model::state::OwnedArtifact;

/// The file the root block lives in, at the project root.
pub const GITIGNORE_FILENAME: &str = ".gitignore";

/// The file the skill paths are listed in, inside the directory that holds them.
pub const SHARED_GITIGNORE_FILE: &str = ".agents/.gitignore";

/// The sentinel a block's first line starts with.
pub const BLOCK_BEGIN: &str = "# BEGIN ambit";

/// The sentinel a block's last line starts with, and the whole of that line as written.
pub const BLOCK_END: &str = "# END ambit";

/// One file ambit maintains a block in, and what that block should list.
///
/// Entries are patterns relative to the file's own directory, since that is what git matches them
/// against.
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

/// The two blocks a project should hold, given what an install owns.
///
/// Every installed skill, and every script a hook ships, lands under the shared directory and is
/// listed in the nested file. Ambit's state directory and the skills link cannot be reached from
/// there and stay at the root. The split is by path, not by artifact kind.
pub fn gitignore_blocks(artifacts: &[OwnedArtifact]) -> Vec<IgnoreBlock> {
    let _ = artifacts;
    todo!("port project/gitignore.ts:gitignoreBlocks")
}

/// The `.gitignore` a project should hold, given the one it holds now.
///
/// Returns the new contents, or `None` when nothing would change (so a caller skips the write).
/// `Some("")` when the block was the whole file and is now gone, telling a caller to delete the
/// file. `file` is the project-relative path, for the error naming an ambiguous block.
///
/// # Errors
///
/// Exit 2 for a file whose markers cannot be read unambiguously.
pub fn update_gitignore_text(
    existing: Option<&str>,
    entries: &[String],
    file: &str,
) -> Result<Option<String>> {
    let _ = (existing, entries, file);
    todo!("port project/gitignore.ts:updateGitignoreText")
}

/// The `.gitignore` a project should hold once ambit owns nothing in it: what `clean` writes.
///
/// The blank line above the block is removed too, since `update_gitignore_text` is what added it.
/// Returns `Some("")` when ambit's block was the whole file (caller should delete it), and `None`
/// when there is no block to remove.
///
/// # Errors
///
/// Exit 2 for a file whose markers cannot be read unambiguously.
pub fn remove_gitignore_text(existing: Option<&str>, file: &str) -> Result<Option<String>> {
    let _ = (existing, file);
    todo!("port project/gitignore.ts:removeGitignoreText")
}

/// Reads one of a project's `.gitignore` files, treating an absent one as a file ambit is about to
/// create. `file` is the project-relative path; the TS default was [`GITIGNORE_FILENAME`].
///
/// # Errors
///
/// Exit 2 for a file that is there but cannot be read.
pub fn read_gitignore_text(project_dir: &Path, file: &str) -> Result<Option<String>> {
    let _ = (project_dir, file);
    todo!("port project/gitignore.ts:readGitignoreText")
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
    let _ = (project_dir, artifacts);
    todo!("port project/gitignore.ts:writeGitignoreBlocks")
}

/// Whether each managed block is already what an install would write.
///
/// # Errors
///
/// Exit 2 for a `.gitignore` that cannot be read, or whose markers are ambiguous.
pub fn gitignore_status(
    project_dir: &Path,
    artifacts: &[OwnedArtifact],
) -> Result<Vec<GitignoreStatus>> {
    let _ = (project_dir, artifacts);
    todo!("port project/gitignore.ts:gitignoreStatus")
}

/// Takes ambit's blocks out of a project's `.gitignore` files. Used by `clean`. A file that is
/// nothing but the block is deleted rather than truncated. Returns the files a block was removed
/// from.
///
/// # Errors
///
/// Exit 2 for a `.gitignore` that cannot be read, or whose markers are ambiguous.
pub fn remove_gitignore_blocks(project_dir: &Path) -> Result<Vec<String>> {
    let _ = project_dir;
    todo!("port project/gitignore.ts:removeGitignoreBlocks")
}
