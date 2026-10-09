use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;

use crate::errors::{AmbitError, Result, config_error, network_error};
use crate::util::env::{Env, home_dir};
use crate::util::fs::{EntryKind, mkdir_p, rm_rf, write_text};
use crate::util::path::join;
use crate::util::string_enum;
use crate::util::text::{is_js_whitespace, js_trim};

pub const CACHE_DIRNAME: &str = "ambit";

pub const REPOS_DIRNAME: &str = "repos";

pub const SOURCES_DIRNAME: &str = "sources";

const READY_SUFFIX: &str = ".ready";

const GIT_SUFFIX: &str = ".git";

const INCOMING_SUFFIX: &str = ".incoming";

const LOCAL_HOST: &str = "local";

// Probe refs are kept so git does not garbage-collect objects a probed checkout needs.
pub const PROBE_NAMESPACE: &str = "refs/ambit/latest";

static PROBE_REFSPECS: LazyLock<[String; 3]> = LazyLock::new(|| {
    [
        format!("+refs/heads/*:{PROBE_NAMESPACE}/heads/*"),
        format!("+refs/tags/*:{PROBE_NAMESPACE}/tags/*"),
        format!("+HEAD:{PROBE_NAMESPACE}/HEAD"),
    ]
});

string_enum! {
    pub enum RefreshMode {
        None => "none",
        Probe => "probe",
        Advance => "advance",
    }
}

const REDIRECTING_GIT_VARS: &[&str] = &["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"];

// "Colon not followed by `/`" is checked in `split_url`: `regex` has no lookahead.
static SCP_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[^@/]+@)?([^@/:]+):(.*)$").expect("a valid pattern"));

static COMMIT_SHA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i:[0-9a-f]{40}|[0-9a-f]{64})$").expect("a valid pattern"));

pub fn is_commit_sha(value: &str) -> bool {
    COMMIT_SHA.is_match(value)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitFetchRequest {
    pub url: String,
    pub r#ref: Option<String>,
    pub pin: Option<String>,
    pub subject: String,
    pub r#where: String,
    pub env: Env,
    pub cwd: PathBuf,
    pub offline: bool,
    pub refresh: Option<RefreshMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedGitSource {
    pub root: PathBuf,
    pub commit: String,
    pub moving: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOutcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_git(args: &[&str], cwd: &Path, env: &Env) -> Result<GitOutcome> {
    let spawned = git_program(env).and_then(|program| {
        Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(git_environment(env))
            .stdin(Stdio::null())
            .output()
    });

    match spawned {
        Ok(output) => Ok(GitOutcome {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(network_error(
            "git is not on PATH",
            [
                "ambit fetches catalogs by running git, and could not start it",
                "install git, or add it to PATH",
            ],
        )),
        Err(error) => Err(network_error(
            "cannot run git",
            [
                error.to_string(),
                "install git, or add it to PATH".to_owned(),
            ],
        )),
    }
}

// On Windows, std falls back to this process's PATH when the child's has no match.
#[cfg(windows)]
fn git_program(env: &Env) -> std::io::Result<PathBuf> {
    // Windows variable names are case-insensitive, and the process usually spells it `Path`.
    let path = env
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map_or("", |(_, value)| value.as_str());

    std::env::split_paths(path)
        .map(|dir| dir.join("git.exe"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
}

#[cfg(not(windows))]
#[allow(clippy::unnecessary_wraps)]
fn git_program(_env: &Env) -> std::io::Result<PathBuf> {
    Ok(PathBuf::from("git"))
}

fn git_environment(env: &Env) -> Env {
    let mut copy = env.clone();

    copy.insert("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned());

    for name in REDIRECTING_GIT_VARS {
        copy.remove(*name);
    }

    copy
}

pub fn cache_root(env: &Env) -> PathBuf {
    if let Some(xdg) = env.get("XDG_CACHE_HOME")
        && !js_trim(xdg).is_empty()
    {
        return join(Path::new(xdg), CACHE_DIRNAME);
    }

    let home = home_dir(env).unwrap_or_default();

    join(&home, &format!(".cache/{CACHE_DIRNAME}"))
}

fn sanitize(segment: &str) -> String {
    static UNSAFE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^A-Za-z0-9._-]+").expect("a valid pattern"));

    let cleaned = UNSAFE.replace_all(segment, "-");

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "-".to_owned()
    } else {
        cleaned.into_owned()
    }
}

fn parse_url(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let mut chars = scheme.chars();

    if !chars.next()?.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }

    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    };
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let hostname = if host_port.starts_with('[') {
        host_port
            .find(']')
            .map_or(host_port, |end| &host_port[..=end])
    } else {
        host_port.split(':').next().unwrap_or("")
    };

    if hostname.contains(' ') {
        return None;
    }

    let mut segments: Vec<&str> = Vec::new();
    let mut parts = path.split('/').skip(1).peekable();

    while let Some(segment) = parts.next() {
        let last = parts.peek().is_none();

        match segment {
            "." if last => segments.push(""),
            "." => {}
            ".." => {
                segments.pop();

                if last {
                    segments.push("");
                }
            }
            other => segments.push(other),
        }
    }

    let mut pathname = String::from("/");

    for c in segments.join("/").chars() {
        if c.is_ascii()
            && !c.is_ascii_control()
            && !matches!(c, ' ' | '"' | '<' | '>' | '`' | '{' | '}')
        {
            pathname.push(c);
        } else {
            let mut buffer = [0; 4];

            for byte in c.encode_utf8(&mut buffer).bytes() {
                let _ = write!(pathname, "%{byte:02X}");
            }
        }
    }

    Some((hostname.to_lowercase(), pathname))
}

fn split_url(url: &str) -> (String, String) {
    if url.contains("://")
        && let Some((host, target)) = parse_url(url)
    {
        let host = if host.is_empty() {
            LOCAL_HOST.to_owned()
        } else {
            host
        };

        return (host, target);
    }

    if let Some(captures) = SCP_LIKE.captures(url) {
        let target = captures.get(2).map_or("", |m| m.as_str());

        if !target.starts_with('/') {
            let host = captures.get(1).map_or("", |m| m.as_str());

            return (host.to_lowercase(), target.to_owned());
        }
    }

    (LOCAL_HOST.to_owned(), url.to_owned())
}

pub fn git_cache_key(url: &str) -> String {
    let (host, target) = split_url(url);
    let mut segments: Vec<&str> = target
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    if let Some(last) = segments.pop() {
        segments.push(last.strip_suffix(GIT_SUFFIX).unwrap_or(last));
    }

    std::iter::once(host.as_str())
        .chain(segments)
        .map(sanitize)
        .collect::<Vec<_>>()
        .join("/")
}

fn kind_of(target: &Path) -> EntryKind {
    match std::fs::metadata(target) {
        Ok(metadata) if metadata.is_dir() => EntryKind::Dir,
        Ok(metadata) if metadata.is_file() => EntryKind::File,
        Ok(_) => EntryKind::Other,
        Err(_) => EntryKind::Missing,
    }
}

fn is_directory(target: &Path) -> bool {
    kind_of(target) == EntryKind::Dir
}

fn is_file(target: &Path) -> bool {
    kind_of(target) == EntryKind::File
}

fn last_line(text: &str) -> String {
    text.split('\n')
        .map(js_trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn git(args: &[&str], request: &GitFetchRequest) -> Result<GitOutcome> {
    run_git(args, &request.cwd, &request.env)
}

fn git_failed(summary: String, outcome: &GitOutcome, advice: String) -> AmbitError {
    let stderr = last_line(&outcome.stderr);
    let said = if stderr.is_empty() {
        last_line(&outcome.stdout)
    } else {
        stderr
    };

    network_error(
        summary,
        [
            if said.is_empty() {
                "git reported no reason".to_owned()
            } else {
                format!("git said: {said}")
            },
            advice,
        ],
    )
}

const REACH_ADVICE: &str = "check `source`, and that you can reach the repository";

fn clone(repo: &Path, request: &GitFetchRequest) -> Result<()> {
    let mut incoming = repo.as_os_str().to_owned();
    incoming.push(INCOMING_SUFFIX);
    let incoming = PathBuf::from(incoming);

    rm_rf(&incoming)?;

    if let Some(parent) = repo.parent() {
        mkdir_p(parent)?;
    }

    let incoming_arg = path_arg(&incoming);
    let outcome = git(
        &[
            "clone",
            "--mirror",
            "--quiet",
            "--",
            &request.url,
            &incoming_arg,
        ],
        request,
    )?;

    if !outcome.ok {
        rm_rf(&incoming)?;

        return Err(git_failed(
            format!("cannot clone {} {}", request.subject, request.r#where),
            &outcome,
            REACH_ADVICE.to_owned(),
        ));
    }

    std::fs::rename(&incoming, repo)?;

    Ok(())
}

fn fetch_into(repo: &Path, request: &GitFetchRequest) -> Result<()> {
    let repo_arg = path_arg(repo);
    let outcome = git(
        &["-C", &repo_arg, "fetch", "--quiet", "--prune", "origin"],
        request,
    )?;

    if !outcome.ok {
        return Err(git_failed(
            format!("cannot fetch {} {}", request.subject, request.r#where),
            &outcome,
            REACH_ADVICE.to_owned(),
        ));
    }

    Ok(())
}

fn probe_into(repo: &Path, request: &GitFetchRequest) -> Result<()> {
    let repo_arg = path_arg(repo);
    let mut args = vec![
        "-C",
        &repo_arg,
        "fetch",
        "--quiet",
        "--prune",
        // Otherwise git follows tags into the clone's own `refs/tags/*`.
        "--no-tags",
        "--",
        // By URL, not `origin`: the mirror refspec `+refs/*:refs/*` would update `refs/heads/*`.
        &request.url,
    ];
    args.extend(PROBE_REFSPECS.iter().map(String::as_str));

    let outcome = git(&args, request)?;

    if !outcome.ok {
        return Err(git_failed(
            format!(
                "cannot check {} for updates {}",
                request.subject, request.r#where
            ),
            &outcome,
            REACH_ADVICE.to_owned(),
        ));
    }

    Ok(())
}

fn rev_parse(repo: &Path, revision: &str, request: &GitFetchRequest) -> Result<Option<String>> {
    let repo_arg = path_arg(repo);
    let spec = format!("{revision}^{{commit}}");
    let outcome = git(
        &["-C", &repo_arg, "rev-parse", "--verify", "--quiet", &spec],
        request,
    )?;

    if !outcome.ok {
        return Ok(None);
    }

    let commit = js_trim(&outcome.stdout);

    Ok((!commit.is_empty()).then(|| commit.to_owned()))
}

fn resolve_commit(repo: &Path, request: &GitFetchRequest) -> Result<Option<String>> {
    rev_parse(repo, request.r#ref.as_deref().unwrap_or("HEAD"), request)
}

struct GitRefResolution {
    commit: String,
    moving: bool,
}

fn resolve_probed(repo: &Path, request: &GitFetchRequest) -> Result<Option<GitRefResolution>> {
    let candidates: Vec<(String, bool)> = match &request.r#ref {
        None => vec![(format!("{PROBE_NAMESPACE}/HEAD"), true)],
        Some(r#ref) => vec![
            (format!("{PROBE_NAMESPACE}/heads/{ref}"), true),
            (format!("{PROBE_NAMESPACE}/tags/{ref}"), true),
            // Last: it also resolves against the clone's own, possibly stale, refs.
            (r#ref.clone(), false),
        ],
    };

    for (revision, moving) in candidates {
        if let Some(commit) = rev_parse(repo, &revision, request)? {
            return Ok(Some(GitRefResolution { commit, moving }));
        }
    }

    Ok(None)
}

fn is_moving_ref(repo: &Path, request: &GitFetchRequest) -> Result<bool> {
    let Some(r#ref) = &request.r#ref else {
        return Ok(true);
    };

    for namespace in ["refs/heads", "refs/tags"] {
        if rev_parse(repo, &format!("{namespace}/{ref}"), request)?.is_some() {
            return Ok(true);
        }
    }

    Ok(false)
}

fn assert_usable_ref(request: &GitFetchRequest) -> Result<()> {
    let Some(r#ref) = &request.r#ref else {
        return Ok(());
    };

    if js_trim(r#ref).is_empty() || r#ref.starts_with('-') || r#ref.chars().any(is_js_whitespace) {
        return Err(config_error(
            format!(
                "{} has an unusable ref {}",
                request.subject, request.r#where
            ),
            [
                format!("\"{ref}\" is not a tag, a branch, or a commit"),
                "quote the ref and write it exactly as the repository has it".to_owned(),
            ],
        ));
    }

    Ok(())
}

fn assert_usable_pin(request: &GitFetchRequest) -> Result<()> {
    match &request.pin {
        Some(pin) if !is_commit_sha(pin) => Err(config_error(
            format!(
                "{} has an unusable pin {}",
                request.subject, request.r#where
            ),
            [
                format!("\"{pin}\" is not a full commit SHA"),
                "delete `ambit.lock` and run `ambit install` again to write a correct one"
                    .to_owned(),
            ],
        )),
        _ => Ok(()),
    }
}

fn unknown_ref(request: &GitFetchRequest) -> AmbitError {
    match &request.r#ref {
        None => config_error(
            format!(
                "{} has no default branch {}",
                request.subject, request.r#where
            ),
            [
                format!("{} is empty, or its HEAD points at nothing", request.url),
                "push a commit, or point `source` elsewhere".to_owned(),
            ],
        ),
        Some(r#ref) => config_error(
            format!(
                "cannot resolve ref \"{ref}\" for {} {}",
                request.subject, request.r#where
            ),
            [
                format!("{} has no branch, tag, or commit \"{ref}\"", request.url),
                "correct `ref`, or omit it to take the default branch".to_owned(),
            ],
        ),
    }
}

fn not_cached(request: &GitFetchRequest, repo: &Path) -> AmbitError {
    network_error(
        format!(
            "{} is not in the cache {}",
            request.subject, request.r#where
        ),
        [
            format!(
                "`--offline` was given, and {} has never been fetched into {}",
                request.url,
                repo.display()
            ),
            "run the command again without `--offline` to fetch it".to_owned(),
        ],
    )
}

fn cannot_refresh_offline(request: &GitFetchRequest) -> AmbitError {
    network_error(
        format!(
            "cannot check {} for updates offline {}",
            request.subject, request.r#where
        ),
        [
            "`--offline` forbids reaching the remote, and only the remote knows where a ref points now",
            "run the command again without `--offline`",
        ],
    )
}

fn unknown_pin(request: &GitFetchRequest, pin: &str) -> AmbitError {
    config_error(
        format!(
            "cannot find the locked commit for {} {}",
            request.subject, request.r#where
        ),
        [
            format!(
                "`ambit.lock` pins {pin}, and {} does not have it",
                request.url
            ),
            "run `ambit update` to pin the commit its `ref` names now, and commit the new lock"
                .to_owned(),
        ],
    )
}

fn pin_not_cached(request: &GitFetchRequest, pin: &str) -> AmbitError {
    network_error(
        format!(
            "cannot resolve the locked commit from the cache for {} {}",
            request.subject, request.r#where
        ),
        [
            format!(
                "`--offline` was given, and the cached clone of {} does not have {pin}",
                request.url
            ),
            "run the command again without `--offline` to fetch it".to_owned(),
        ],
    )
}

fn ref_not_cached(request: &GitFetchRequest) -> AmbitError {
    let named = match &request.r#ref {
        None => "the default branch".to_owned(),
        Some(r#ref) => format!("ref \"{ref}\""),
    };

    network_error(
        format!(
            "cannot resolve {named} from the cache for {} {}",
            request.subject, request.r#where
        ),
        [
            format!(
                "`--offline` was given, and the cached clone of {} does not have it",
                request.url
            ),
            "run the command again without `--offline` to fetch it".to_owned(),
        ],
    )
}

fn pinned_commit(
    repo: &Path,
    pin: &str,
    cloned: bool,
    request: &GitFetchRequest,
) -> Result<String> {
    let mut commit = rev_parse(repo, pin, request)?;

    if commit.is_none() && !cloned {
        if request.offline {
            return Err(pin_not_cached(request, pin));
        }

        fetch_into(repo, request)?;
        commit = rev_parse(repo, pin, request)?;
    }

    commit.ok_or_else(|| unknown_pin(request, pin))
}

fn ensure_checkout(
    cache: &Path,
    key: &str,
    repo: &Path,
    commit: &str,
    request: &GitFetchRequest,
) -> Result<PathBuf> {
    let target = join(&join(&join(cache, SOURCES_DIRNAME), key), commit);
    let mut ready = target.as_os_str().to_owned();
    ready.push(READY_SUFFIX);
    let ready = PathBuf::from(ready);

    if is_file(&ready) && is_directory(&target) {
        return Ok(target);
    }

    rm_rf(&ready)?;
    rm_rf(&target)?;

    if let Some(parent) = target.parent() {
        mkdir_p(parent)?;
    }

    let repo_arg = path_arg(repo);
    // Clears a stale registration that would make `worktree add` refuse.
    git(&["-C", &repo_arg, "worktree", "prune"], request)?;

    let target_arg = path_arg(&target);
    let outcome = git(
        &[
            "-C",
            &repo_arg,
            "-c",
            "core.autocrlf=false",
            "worktree",
            "add",
            "--detach",
            "--quiet",
            "--force",
            &target_arg,
            commit,
        ],
        request,
    )?;

    if !outcome.ok {
        rm_rf(&target)?;

        return Err(git_failed(
            format!(
                "cannot check out {commit} of {} {}",
                request.subject, request.r#where
            ),
            &outcome,
            format!("delete {} and run the command again", cache.display()),
        ));
    }

    // Written last: later runs trust the marker to mean the checkout is complete.
    write_text(&ready, &format!("{commit}\n"))?;

    Ok(target)
}

pub fn fetch_git_source(request: &GitFetchRequest) -> Result<FetchedGitSource> {
    assert_usable_ref(request)?;
    assert_usable_pin(request)?;

    let refresh = request.refresh.unwrap_or(RefreshMode::None);
    let offline = request.offline;

    if offline && refresh != RefreshMode::None {
        return Err(cannot_refresh_offline(request));
    }

    let cache = cache_root(&request.env);
    let key = git_cache_key(&request.url);
    let repo = join(&join(&cache, REPOS_DIRNAME), &format!("{key}{GIT_SUFFIX}"));

    let mut cloned = false;

    if !is_directory(&repo) {
        if offline {
            return Err(not_cached(request, &repo));
        }

        clone(&repo, request)?;
        cloned = true;
    }

    let pin = if refresh == RefreshMode::None {
        request.pin.as_deref()
    } else {
        None
    };

    if let Some(pin) = pin {
        let commit = pinned_commit(&repo, pin, cloned, request)?;
        let root = ensure_checkout(&cache, &key, &repo, &commit, request)?;

        return Ok(FetchedGitSource {
            root,
            commit,
            moving: None,
        });
    }

    if refresh == RefreshMode::Probe {
        probe_into(&repo, request)?;
        let probed = resolve_probed(&repo, request)?.ok_or_else(|| unknown_ref(request))?;
        let root = ensure_checkout(&cache, &key, &repo, &probed.commit, request)?;

        return Ok(FetchedGitSource {
            root,
            commit: probed.commit,
            moving: Some(probed.moving),
        });
    }

    if refresh == RefreshMode::Advance && !cloned {
        fetch_into(&repo, request)?;
    }

    let mut commit = resolve_commit(&repo, request)?;

    if commit.is_none() && !cloned && refresh != RefreshMode::Advance {
        if offline {
            return Err(ref_not_cached(request));
        }

        fetch_into(&repo, request)?;
        commit = resolve_commit(&repo, request)?;
    }

    let commit = commit.ok_or_else(|| unknown_ref(request))?;
    let root = ensure_checkout(&cache, &key, &repo, &commit, request)?;
    let moving = if refresh == RefreshMode::None {
        None
    } else {
        Some(is_moving_ref(&repo, request)?)
    };

    Ok(FetchedGitSource {
        root,
        commit,
        moving,
    })
}

#[cfg(test)]
mod tests;
