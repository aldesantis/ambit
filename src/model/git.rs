//! The git cache: bare clones, commit checkouts, and resolving a ref against them.
//!
//! git runs as a child process ([`run_git`]) with a cleared environment rebuilt from the [`Env`]
//! the command was given, so the cache location and git's own configuration are a function of the
//! call, not of the process.

use std::path::{Path, PathBuf};

use crate::errors::Result;
use crate::util::env::Env;
use crate::util::string_enum;

/// The directory ambit owns inside the XDG cache root.
pub const CACHE_DIRNAME: &str = "ambit";

/// Bare clones within the cache, keyed by host/owner/repo.
pub const REPOS_DIRNAME: &str = "repos";

/// Checkouts within the cache, keyed by host/owner/repo and then commit.
pub const SOURCES_DIRNAME: &str = "sources";

/// Where a probe writes what the remote says, inside the cached clone.
///
/// Outside `refs/heads/` and `refs/tags/`, which a project's `ref` is resolved against, so a probe
/// cannot change what a later command installs. The refs are kept rather than deleted afterward so
/// git does not garbage-collect the objects a probed checkout needs.
pub const PROBE_NAMESPACE: &str = "refs/ambit/latest";

string_enum! {
    /// How much of the remote one resolve may consult.
    ///
    /// - `None`: the cache alone, refetching only when it cannot answer the ref. Every command but
    ///   the two below.
    /// - `Probe`: ask the remote where the ref points now, without letting the answer become what
    ///   the clone's own refs say. `ambit outdated`, which reports and must change nothing.
    /// - `Advance`: fetch normally, so the clone's refs move and every later resolve follows.
    ///   `ambit update`, which exists to do exactly that.
    pub enum RefreshMode {
        None => "none",
        Probe => "probe",
        Advance => "advance",
    }
}

/// Every refresh mode, in declaration order.
pub const REFRESH_MODES: &[RefreshMode] = RefreshMode::ALL;

/// Whether a string is a full commit SHA, which is what a pin must be.
///
/// Exported so the lock reader can validate a hand-edited pin against the same rule and report it
/// against `ambit.lock` rather than as a git failure.
pub fn is_commit_sha(value: &str) -> bool {
    let _ = value;
    todo!("port model/git.ts:isCommitSha")
}

/// One repository to fetch, and everything the errors and the cache need to know about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitFetchRequest {
    /// The URL as git will receive it.
    pub url: String,
    /// Tag, branch, or commit. Absent means the repository's default branch.
    pub r#ref: Option<String>,
    /// The commit an earlier resolution of this source recorded, from `ambit.lock`.
    ///
    /// When present, this commit is checked out directly and `ref` is not consulted. Only used
    /// under [`RefreshMode::None`]. Must be a full commit SHA ([`is_commit_sha`]).
    pub pin: Option<String>,
    /// How the thing being fetched is named in errors: `catalog "company"`.
    pub subject: String,
    /// The `(file line N)` suffix its config entry sits at.
    pub r#where: String,
    /// Environment the cache location and git itself are read from.
    pub env: Env,
    /// Directory git runs in, so a URL naming a relative path means something definite.
    pub cwd: PathBuf,
    /// `--offline`: answer from the cache, and fail rather than reach the remote.
    pub offline: bool,
    /// How much of the remote this fetch may consult. Absent means [`RefreshMode::None`].
    pub refresh: Option<RefreshMode>,
}

/// A fetched source: a directory to read, and the commit its contents are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedGitSource {
    /// Absolute path to the checkout.
    pub root: PathBuf,
    /// The full commit SHA the ref resolved to.
    pub commit: String,
    /// Whether the `ref` this resolved through can move: a branch, a tag, or the repository's
    /// default branch. False for a `ref` naming a commit, which is already a pin.
    ///
    /// Absent under [`RefreshMode::None`]: deciding it costs an extra `rev-parse`, and answering
    /// it from a clone that may be stale would be answering it wrong.
    pub moving: Option<bool>,
}

/// What one git invocation produced. A non-zero exit is an outcome, not an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOutcome {
    pub ok: bool,
    /// Decoded lossily.
    pub stdout: String,
    /// Decoded lossily.
    pub stderr: String,
}

/// Runs `git <args>` in `cwd` with exactly the environment `env` describes, minus `GIT_DIR`,
/// `GIT_WORK_TREE` and `GIT_INDEX_FILE`, plus `GIT_TERMINAL_PROMPT=0`. Stdin is closed so git never
/// waits on input.
///
/// # Errors
///
/// Exit 4 when git is not on `PATH`, or cannot be spawned at all.
pub fn run_git(args: &[&str], cwd: &Path, env: &Env) -> Result<GitOutcome> {
    let _ = (args, cwd, env);
    todo!("port model/git.ts:git invocation")
}

/// Where the cache lives.
///
/// Read from the environment it is given rather than the process's, so the location is a function
/// of the caller's arguments and a test can point it somewhere disposable.
pub fn cache_root(env: &Env) -> PathBuf {
    let _ = env;
    todo!("port model/git.ts:cacheRoot")
}

/// Where a repository is cached, relative to the cache root: `<host>/<path…>`.
///
/// A trailing `.git` is stripped so `https://github.com/acme/skills` and
/// `https://github.com/acme/skills.git` share one clone, since they are the same repository.
pub fn git_cache_key(url: &str) -> String {
    let _ = url;
    todo!("port model/git.ts:gitCacheKey")
}

/// Fetches a git source into the cache and returns the commit's checkout.
///
/// A [`GitFetchRequest::pin`] short-circuits everything else: the recorded commit is checked out
/// and the ref is never resolved.
///
/// Otherwise, under the default [`RefreshMode::None`], the clone is fetched only when it cannot
/// resolve the ref, so a second run over an unchanged config need not touch the network.
/// [`RefreshMode::Advance`] fetches into the clone's own refs, so later resolves see the result.
/// [`RefreshMode::Probe`] fetches into [`PROBE_NAMESPACE`], which nothing else reads. Both ignore a
/// pin, since both ask a question a pin cannot answer.
///
/// A probe still writes a checkout: checkouts are keyed by commit, so this adds a directory rather
/// than changing what any existing path means.
///
/// # Errors
///
/// Exit 4 if git is missing, a clone/fetch/probe/checkout fails, or `--offline` was given and the
/// cache cannot answer; exit 2 for a ref or a pinned commit the repository does not have.
pub fn fetch_git_source(request: &GitFetchRequest) -> Result<FetchedGitSource> {
    let _ = request;
    todo!("port model/git.ts:fetchGitSource")
}
