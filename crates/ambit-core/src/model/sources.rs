//! Source resolution: a `source` string, plus an optional `ref`, to a directory a parser can read.
//!
//! Catalogs and `skills` entries carrying their own source share this module and its grammar.
//! Formats are the ones people already type into other tools (`acme/skills`, a GitHub URL, an ssh
//! remote) plus two explicit prefixes for what shorthand cannot express: `path:` names a directory,
//! `git:` hands the rest to git verbatim as an escape hatch for any URL shape ambit would otherwise
//! have to guess about.
//!
//! A shorthand with no host means GitHub, since that is where catalogs live. Everything else is
//! taken literally; ambit rewrites no URLs, so what a project writes is what git is asked for.
//!
//! Fetching goes through the shared cache rather than into the project, so two projects on one
//! catalog fetch it once and neither owns it.

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::errors::{AmbitError, Result, config_error};
use crate::model::git::{GitFetchRequest, RefreshMode, fetch_git_source};
use crate::util::control::Control;
use crate::util::env::Env;
use crate::util::path::resolve;
use crate::util::text::js_trim;

/// The prefix marking a source as a local directory.
const PATH_PREFIX: &str = "path:";

/// The prefix marking the remainder as a git URL, whatever its shape.
const GIT_PREFIX: &str = "git:";

/// Where a bare `owner/repo` shorthand resolves to.
const GITHUB_HOST: &str = "github.com";

/// `owner/repo`, optionally `@ref`. Neither part may hold an `@`, so the split is unambiguous.
static SHORTHAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._-]*)/([A-Za-z0-9][A-Za-z0-9._-]*)(?:@(.+))?$")
        .expect("a valid pattern")
});

/// A scp-like git URL: `[user@]host:path`, the shape ssh remotes are written in.
///
/// The colon must not be followed by `/`. [`is_scp_like`] checks that half, since the `regex` crate
/// has no lookahead.
static SCP_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[^@/]+@)?[^@/:]+:").expect("a valid pattern"));

fn is_scp_like(source: &str) -> bool {
    SCP_LIKE
        .find(source)
        .is_some_and(|found| !source[found.end()..].starts_with('/'))
}

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
    /// one catalog and leaves the rest where they were; a run-wide setting could not express that,
    /// and a catalog whose pin moved because a sibling was named would be a pin nobody asked to
    /// move.
    pub refresh: Option<RefreshMode>,
    /// The commit an earlier resolution of this source recorded, from `ambit.lock`.
    ///
    /// Resolving to that commit instead of asking what `ref` names now is how a committed lock
    /// reproduces an install on a machine whose shared cache holds something else. Ignored
    /// alongside a `refresh`, which asks for a newer answer than a pin can give, and meaningless
    /// for a `path:` source, which has no revision to pin.
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
    /// Cancellation and progress for the fetches resolving needs.
    pub control: Control,
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
    /// Absent for a `path:` source, which has no ref, and absent without a refresh, since that
    /// question only arises when one was requested.
    pub moving: Option<bool>,
}

/// The error for two disagreeing refs, which ambit will not pick between.
fn conflicting_refs(request: &SourceRequest, in_source: &str, declared: &str) -> AmbitError {
    config_error(
        format!("{} names two refs {}", request.subject, request.r#where),
        [
            format!("`source` ends with \"@{in_source}\" and `ref` says \"{declared}\""),
            "drop one of the two, or make them agree".to_owned(),
        ],
    )
}

/// The ref a shorthand carried and the one the entry declared, once they are known to agree.
fn ref_of(request: &SourceRequest, in_source: Option<&str>) -> Result<Option<String>> {
    let Some(in_source) = in_source else {
        return Ok(request.r#ref.clone());
    };

    if let Some(declared) = &request.r#ref
        && declared != in_source
    {
        return Err(conflicting_refs(request, in_source, declared));
    }

    Ok(Some(in_source.to_owned()))
}

/// Reads a `source` string as one of the accepted formats.
///
/// Pure, and the only place the grammar lives: `resolve` and `install` both reach a source through
/// here, so a format works everywhere or nowhere.
///
/// # Errors
///
/// Exit 2 for a source matching no format, an empty `path:`, an empty `git:`, or a `@ref` shorthand
/// contradicting the entry's own `ref`.
pub fn parse_source(request: &SourceRequest) -> Result<Source> {
    let source = request.source.as_str();

    if let Some(directory) = source.strip_prefix(PATH_PREFIX) {
        if js_trim(directory).is_empty() {
            return Err(config_error(
                format!(
                    "{} has an empty path source {}",
                    request.subject, request.r#where
                ),
                [
                    format!("`{source}` names no directory"),
                    "write the directory after the prefix, as `path:./dir`".to_owned(),
                ],
            ));
        }

        return Ok(Source::Path {
            directory: directory.to_owned(),
        });
    }

    if let Some(url) = source.strip_prefix(GIT_PREFIX) {
        if js_trim(url).is_empty() {
            return Err(config_error(
                format!(
                    "{} has an empty git source {}",
                    request.subject, request.r#where
                ),
                [
                    format!("`{source}` names no repository"),
                    "write the URL after the prefix, as `git:ssh://host/owner/repo.git`".to_owned(),
                ],
            ));
        }

        return Ok(Source::Git {
            url: url.to_owned(),
            r#ref: request.r#ref.clone(),
        });
    }

    // Taken literally: a URL or an ssh remote is already exactly what git wants.
    if source.contains("://") || is_scp_like(source) {
        return Ok(Source::Git {
            url: source.to_owned(),
            r#ref: request.r#ref.clone(),
        });
    }

    if let Some(shorthand) = SHORTHAND.captures(source) {
        let owner = &shorthand[1];
        let repo = &shorthand[2];
        let in_source = shorthand.get(3).map(|m| m.as_str());

        return Ok(Source::Git {
            url: format!("https://{GITHUB_HOST}/{owner}/{repo}.git"),
            r#ref: ref_of(request, in_source)?,
        });
    }

    Err(config_error(
        format!(
            "{} has an unrecognized source {}",
            request.subject, request.r#where
        ),
        [
            format!("`{source}` matches none of the source formats ambit accepts"),
            "use owner/repo, a git URL, `git:<url>`, or `path:./dir`".to_owned(),
        ],
    ))
}

/// Resolves a `path:` source to the directory it names.
///
/// # Errors
///
/// Exit 2 if the directory is not there.
fn resolve_path_root(
    directory: &str,
    request: &SourceRequest,
    context: &SourceContext,
) -> Result<PathBuf> {
    let root = resolve(&context.project_dir, directory);

    // An unreadable path is reported alongside the not-a-directory case: same mistake, same fix.
    if std::fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Ok(root);
    }

    Err(config_error(
        format!("{} is not a directory {}", request.subject, request.r#where),
        [
            format!("{} does not exist, or is not a directory", root.display()),
            "correct `source`, or create the directory".to_owned(),
        ],
    ))
}

/// Resolves a source to a directory on disk, fetching it if it is a git source.
///
/// A `path:` source is read in place, so `--offline`, `refresh`, and `pin` have nothing to say
/// about it: there is no cache between the project and the directory it names, no remote to ask,
/// and no revision to have moved or been recorded.
///
/// # Errors
///
/// Exit 2 for a source ambit cannot read, a missing directory, or an unknown ref; exit 4 if git is
/// missing, a fetch fails, or `--offline` was given and the cache cannot answer.
pub fn resolve_source(request: &SourceRequest, context: &SourceContext) -> Result<ResolvedSource> {
    match parse_source(request)? {
        Source::Path { directory } => Ok(ResolvedSource {
            root: resolve_path_root(&directory, request, context)?,
            commit: None,
            moving: None,
        }),
        Source::Git { url, r#ref } => {
            let fetched = fetch_git_source(&GitFetchRequest {
                url,
                r#ref,
                pin: request.pin.clone(),
                subject: request.subject.clone(),
                r#where: request.r#where.clone(),
                env: context.env.clone(),
                cwd: context.project_dir.clone(),
                offline: context.offline,
                refresh: request.refresh,
                control: context.control.clone(),
            })?;

            Ok(ResolvedSource {
                root: fetched.root,
                commit: Some(fetched.commit),
                moving: fetched.moving,
            })
        }
    }
}

#[cfg(test)]
mod tests;
