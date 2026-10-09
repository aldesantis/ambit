use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::errors::{AmbitError, Result, config_error};
use crate::model::git::{GitFetchRequest, RefreshMode, fetch_git_source};
use crate::util::env::Env;
use crate::util::path::resolve;
use crate::util::text::js_trim;

const PATH_PREFIX: &str = "path:";

pub fn is_path_source(source: &str) -> bool {
    source.starts_with(PATH_PREFIX)
}

const GIT_PREFIX: &str = "git:";

const GITHUB_HOST: &str = "github.com";

static SHORTHAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._-]*)/([A-Za-z0-9][A-Za-z0-9._-]*)(?:@(.+))?$")
        .expect("a valid pattern")
});

/// The colon must not be followed by `/`; [`is_scp_like`] checks that half, since the `regex`
/// crate has no lookahead.
static SCP_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[^@/]+@)?[^@/:]+:").expect("a valid pattern"));

fn is_scp_like(source: &str) -> bool {
    SCP_LIKE
        .find(source)
        .is_some_and(|found| !source[found.end()..].starts_with('/'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Path { directory: String },
    Git { url: String, r#ref: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceRequest {
    pub source: String,
    pub r#ref: Option<String>,
    pub subject: String,
    pub r#where: String,
    pub refresh: Option<RefreshMode>,
    pub pin: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceContext {
    pub project_dir: PathBuf,
    pub env: Env,
    pub offline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSource {
    pub root: PathBuf,
    pub commit: Option<String>,
    pub moving: Option<bool>,
}

fn conflicting_refs(request: &SourceRequest, in_source: &str, declared: &str) -> AmbitError {
    config_error(
        format!("{} names two refs {}", request.subject, request.r#where),
        [
            format!("`source` ends with \"@{in_source}\" and `ref` says \"{declared}\""),
            "drop one of the two, or make them agree".to_owned(),
        ],
    )
}

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

fn resolve_path_root(
    directory: &str,
    request: &SourceRequest,
    context: &SourceContext,
) -> Result<PathBuf> {
    let root = resolve(&context.project_dir, directory);

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
