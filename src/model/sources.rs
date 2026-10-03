//! `source` strings: the grammar, and resolving one to a directory on disk.

use std::path::PathBuf;

use crate::errors::Result;
use crate::model::git::RefreshMode;
use crate::util::env::Env;

/// A source, read as one of the accepted formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A local directory, read in place. `directory` is as written after the prefix; relative paths
    /// are resolved against the project.
    Path { directory: String },
    /// A git repository, fetched into the cache. `url` is as git will receive it; `ref` is a tag,
    /// branch, or commit, absent for the repository's default branch.
    Git { url: String, r#ref: Option<String> },
}

/// One `source`/`ref` pair, and how to name it when something is wrong with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceRequest {
    pub source: String,
    pub r#ref: Option<String>,
    /// How the thing being resolved is named in errors: `catalog "company"`.
    pub subject: String,
    /// The `(file line N)` suffix its config entry sits at.
    pub r#where: String,
    /// How much of the remote resolving this source may consult. Absent means
    /// [`RefreshMode::None`], which applies to every command except `ambit outdated` and
    /// `ambit update`.
    ///
    /// Set per request rather than on [`SourceContext`] because `ambit update company` refreshes
    /// one catalog and leaves the rest where they were.
    pub refresh: Option<RefreshMode>,
    /// The commit an earlier resolution of this source recorded, from `ambit.lock`.
    ///
    /// Resolving to that commit instead of asking what `ref` names now is how a committed lock
    /// reproduces an install on a machine whose shared cache holds something else. Ignored
    /// alongside a `refresh`, and meaningless for a `path:` source.
    pub pin: Option<String>,
}

/// What resolving a source reads from outside its arguments.
///
/// Passed rather than reached for, so the cache location is a function of the call and a test can
/// point it somewhere disposable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceContext {
    /// What a relative `path:` source, and a relative git URL, are resolved against.
    pub project_dir: PathBuf,
    /// Environment the cache location and git itself are read from.
    pub env: Env,
    /// `--offline`: resolve from the cache alone, and fail with exit 4 rather than fetch.
    pub offline: bool,
}

/// A source resolved to a directory on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSource {
    /// Absolute path to the root of what was resolved.
    pub root: PathBuf,
    /// The commit a git source was pinned to. Absent for `path:`, which has no revision.
    pub commit: Option<String>,
    /// Whether the `ref` this resolved through can move: a branch, a tag, or the repository's
    /// default branch, as against a `ref` naming a commit, which is already a pin.
    ///
    /// Absent for a `path:` source, and absent without a refresh, since that question only arises
    /// when one was requested.
    pub moving: Option<bool>,
}

/// Reads a `source` string as one of the accepted formats.
///
/// Pure, and the only place the grammar lives, so a format works everywhere or nowhere.
///
/// # Errors
///
/// Exit 2 for a source matching no format, an empty `path:`, an empty `git:`, or a `@ref` shorthand
/// contradicting the entry's own `ref`.
pub fn parse_source(request: &SourceRequest) -> Result<Source> {
    let _ = request;
    todo!("port model/sources.ts:parseSource")
}

/// Resolves a source to a directory on disk, fetching it if it is a git source.
///
/// A `path:` source is read in place, so `--offline`, `refresh`, and `pin` have nothing to say
/// about it.
///
/// # Errors
///
/// Exit 2 for a source ambit cannot read, a missing directory, or an unknown ref; exit 4 if git is
/// missing, a fetch fails, or `--offline` was given and the cache cannot answer.
pub fn resolve_source(request: &SourceRequest, context: &SourceContext) -> Result<ResolvedSource> {
    let _ = (request, context);
    todo!("port model/sources.ts:resolveSource")
}
