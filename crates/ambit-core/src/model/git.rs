//! Git source cache: bare clones under `$XDG_CACHE_HOME/ambit`, fetched on demand, plus one
//! checkout per commit.
//!
//! - A pin (`ambit.lock`'s recorded commit) is checked out directly, without resolving the ref.
//!   This is what lets a committed lock reproduce an install on a different machine's cache.
//! - The clone is refetched only when it cannot resolve the requested ref. Fetching on every
//!   resolve would let a moving ref like `ref: main` mean different commits between runs.
//!   `ambit update` is what advances the cache. A source with no pin has no earlier resolution to
//!   agree with, so `install` always asks the remote for it rather than reusing whatever another
//!   project last left in the shared cache (see `catalog_plan` in `src/project/install.rs`).
//! - A checkout is keyed by commit, not by ref, so projects pinned to different refs of one
//!   repository share the clone and reuse an existing checkout.
//! - Checkouts use `git worktree` rather than `git archive | tar`, so git is the only required
//!   PATH tool.
//! - `--offline` blocks only the clone and the fetch. A checkout ambit can produce from a clone it
//!   already has is still allowed; both places that would otherwise reach the remote fail with
//!   exit 4 instead.
//! - Two commands reach the remote anyway. `ambit update` fetches into the clone's own refs
//!   ([`RefreshMode::Advance`]), so later resolves see the new commit. `ambit outdated` reports
//!   and must change nothing, so it fetches into [`PROBE_NAMESPACE`] instead
//!   ([`RefreshMode::Probe`]), which ref resolution never reads.
//! - git runs as a child process ([`run_git`]) with a cleared environment rebuilt from the [`Env`]
//!   the command was given, so the cache location and git's own configuration are a function of
//!   the call, not of the process. [`GIT_PROGRAM_VAR`] in that environment names the executable;
//!   credentials arrive the same way ([`credentials`]), and what git prints is redacted before
//!   anything can quote it.
//! - Every process sharing a cache serializes clone, fetch and checkout through an OS lock on
//!   [`CACHE_LOCK_FILENAME`] in the cache root. Without it, two processes could both find a clone
//!   missing and race to rename theirs into place.

pub mod credentials;

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::LazyLock;
use std::thread;
use std::time::Duration;

use regex::Regex;

use crate::errors::{AmbitError, Result, config_error, network_error};
use crate::model::git::credentials::{deciding_line, redact};
use crate::util::control::{Control, Progress, Stage, canceled};
use crate::util::env::Env;
use crate::util::fs::{EntryKind, FileLock, mkdir_p, rename, rm_rf, try_lock_file, write_text};
use crate::util::path::join;
use crate::util::string_enum;
use crate::util::text::{is_js_whitespace, js_trim};

/// The directory ambit owns inside the XDG cache root.
pub const CACHE_DIRNAME: &str = "ambit";

/// Bare clones within the cache, keyed by host/owner/repo.
pub const REPOS_DIRNAME: &str = "repos";

/// Checkouts within the cache, keyed by host/owner/repo and then commit.
pub const SOURCES_DIRNAME: &str = "sources";

/// The variable naming the git executable to run, for a library caller that ships its own git.
///
/// The CLI removes it from its environment and always runs the `git` on `PATH`.
pub const GIT_PROGRAM_VAR: &str = "AMBIT_GIT_PROGRAM";

/// The lock file in the cache root that clone, fetch and checkout hold.
pub const CACHE_LOCK_FILENAME: &str = "cache.lock";

/// How often a cancelable wait looks at the cancel flag: a running git, or a held cache lock.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Suffix of the file written beside a checkout once it is complete.
const READY_SUFFIX: &str = ".ready";

/// Suffix of a bare clone's directory, and the one stripped off a URL's last path segment.
const GIT_SUFFIX: &str = ".git";

/// Where a clone lands while it is still incomplete, so a failed one is never mistaken for a hit.
const INCOMING_SUFFIX: &str = ".incoming";

/// Stands in for the host of a git URL naming a local path: `file://…`, `/srv/skills.git`.
const LOCAL_HOST: &str = "local";

/// Where a probe writes what the remote says, inside the cached clone.
///
/// Outside `refs/heads/` and `refs/tags/`, which a project's `ref` is resolved against, so a probe
/// cannot change what a later command installs. The refs are kept rather than deleted afterward so
/// git does not garbage-collect the objects a probed checkout needs.
pub const PROBE_NAMESPACE: &str = "refs/ambit/latest";

/// What a probe fetches, and where it lands. All three run every time.
///
/// An absent `ref` means the remote's `HEAD`. Which of the three resolves also decides
/// [`FetchedGitSource::moving`].
static PROBE_REFSPECS: LazyLock<[String; 3]> = LazyLock::new(|| {
    [
        format!("+refs/heads/*:{PROBE_NAMESPACE}/heads/*"),
        format!("+refs/tags/*:{PROBE_NAMESPACE}/tags/*"),
        format!("+HEAD:{PROBE_NAMESPACE}/HEAD"),
    ]
});

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

/// Env vars that would point git at the caller's repository instead of the cache. Set when ambit
/// runs from inside a git hook or alias.
const REDIRECTING_GIT_VARS: &[&str] = &["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"];

/// A scp-like git URL, `git@github.com:acme/skills.git`, which is not a parseable URL.
///
/// The colon must not be followed by `/`. That half is checked in [`split_url`], since the `regex`
/// crate has no lookahead.
static SCP_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[^@/]+@)?([^@/:]+):(.*)$").expect("a valid pattern"));

/// A full commit SHA: sha1 today, sha256 in a repository built for it.
///
/// Full rather than abbreviated, and hex only, because that is what a pin must be. A pin that
/// could name a branch would be a moving pin, and one that could start with `-` would be a git
/// option.
static COMMIT_SHA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i:[0-9a-f]{40}|[0-9a-f]{64})$").expect("a valid pattern"));

/// Whether a string is a full commit SHA, which is what a pin must be.
///
/// Exported so the lock reader can validate a hand-edited pin against the same rule and report it
/// against `ambit.lock` rather than as a git failure.
pub fn is_commit_sha(value: &str) -> bool {
    COMMIT_SHA.is_match(value)
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
    /// under [`RefreshMode::None`]; the refreshing modes exist to ask where a ref points now, which
    /// a recorded commit cannot answer.
    ///
    /// Must be a full commit SHA ([`is_commit_sha`]).
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
    /// Cancellation and progress. A cancel stops the running git and leaves the cache as it was.
    pub control: Control,
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
    /// Absent under [`RefreshMode::None`], which does not need it: deciding it costs an extra
    /// `rev-parse`, and answering it from a clone that may be stale would be answering it wrong.
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
/// `GIT_WORK_TREE`, `GIT_INDEX_FILE` and [`GIT_PROGRAM_VAR`], plus `GIT_TERMINAL_PROMPT=0`. Stdin
/// is closed so git never waits on input. Credentials in what git prints are redacted
/// ([`redact`]).
///
/// A non-zero exit is data rather than an error: `rev-parse` failing is how ambit asks whether the
/// cache already knows a ref. Only a git that cannot start at all fails.
///
/// # Errors
///
/// Exit 4 when git is not on `PATH` (or not at [`GIT_PROGRAM_VAR`]), or cannot be spawned at all.
pub fn run_git(args: &[&str], cwd: &Path, env: &Env) -> Result<GitOutcome> {
    run_git_controlled(args, cwd, env, &Control::default())
}

/// [`run_git`], stopping the child when `control` is canceled.
///
/// # Errors
///
/// As [`run_git`]; [`canceled`] once the cancel flag is set, before git starts or while it runs.
pub fn run_git_controlled(
    args: &[&str],
    cwd: &Path,
    env: &Env,
    control: &Control,
) -> Result<GitOutcome> {
    run_git_reporting(args, cwd, env, control, None)
}

/// [`run_git_controlled`], turning git's `--progress` lines into reports about `subject` when one
/// is given.
fn run_git_reporting(
    args: &[&str],
    cwd: &Path,
    env: &Env,
    control: &Control,
    subject: Option<&str>,
) -> Result<GitOutcome> {
    control.check()?;

    let spawned = git_program(env).and_then(|program| {
        Command::new(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(git_environment(env))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    });

    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return Err(cannot_start(&error, env)),
    };

    // Both pipes drain on their own threads, so a child filling one cannot block on it while this
    // thread waits for it to exit.
    let stdout = child.stdout.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let reporter = subject.map(|subject| (control.clone(), subject.to_owned()));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || read_stderr(pipe, reporter.as_ref())));

    let status = wait(&mut child, control)?;
    let collect = |handle: Option<thread::JoinHandle<Vec<u8>>>| {
        let bytes = handle
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();

        redact(&String::from_utf8_lossy(&bytes), env)
    };

    Ok(GitOutcome {
        ok: status.success(),
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Waits for `child`, killing it if `control` is canceled first.
///
/// A canceled run does not wait for the pipe readers: a helper git started (`git-remote-https`,
/// `ssh`) can hold the pipes a little longer than git itself, and nothing is read from them.
///
/// # Errors
///
/// [`canceled`] when the flag is set; exit 1 if the child cannot be waited on.
fn wait(child: &mut Child, control: &Control) -> Result<ExitStatus> {
    if !control.is_cancelable() {
        return Ok(child.wait()?);
    }

    // Starts short, since most git calls are a `rev-parse` that finishes in milliseconds.
    let mut interval = Duration::from_millis(2);

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if control.is_canceled() {
            let _ = child.kill();
            let _ = child.wait();

            return Err(canceled());
        }

        thread::sleep(interval);
        interval = (interval * 2).min(POLL_INTERVAL);
    }
}

/// `<phase>: NN% (current/total)`, as `--progress` writes it.
static PROGRESS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:remote: )?([A-Za-z][A-Za-z ]*):\s+\d+% \((\d+)/(\d+)\)")
        .expect("a valid pattern")
});

/// Reads git's standard error to the end, reporting each progress line as it arrives.
fn read_stderr(mut pipe: impl Read, reporter: Option<&(Control, String)>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let mut line_start = 0;

    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };

        bytes.extend_from_slice(&buffer[..read]);

        let Some((control, subject)) = reporter else {
            continue;
        };

        // A progress line ends in `\r` while it updates and in `\n` once done.
        while let Some(offset) = bytes[line_start..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
        {
            let line = String::from_utf8_lossy(&bytes[line_start..line_start + offset]);

            if let Some(progress) = progress_of(&line, subject) {
                control.report(&progress);
            }

            line_start += offset + 1;
        }
    }

    bytes
}

/// The report one `--progress` line amounts to, if it is one.
fn progress_of(line: &str, subject: &str) -> Option<Progress> {
    let captures = PROGRESS_LINE.captures(js_trim(line))?;

    Some(Progress {
        stage: Stage::Fetching,
        subject: format!("{subject}: {}", js_trim(&captures[1])),
        current: captures[2].parse().ok()?,
        total: captures[3].parse().ok()?,
    })
}

/// The error for a git that could not be started.
fn cannot_start(error: &std::io::Error, env: &Env) -> AmbitError {
    let missing = error.kind() == std::io::ErrorKind::NotFound;

    match configured_program(env) {
        Some(program) if missing => network_error(
            format!("git is not at {program}"),
            [
                "ambit fetches catalogs by running git, and could not start it".to_owned(),
                format!("check {GIT_PROGRAM_VAR}, or install git there"),
            ],
        ),
        Some(program) => network_error(
            format!("cannot run git at {program}"),
            [
                error.to_string(),
                format!("check {GIT_PROGRAM_VAR}, or install git there"),
            ],
        ),
        None if missing => network_error(
            "git is not on PATH",
            [
                "ambit fetches catalogs by running git, and could not start it",
                "install git, or add it to PATH",
            ],
        ),
        None => network_error(
            "cannot run git",
            [
                error.to_string(),
                "install git, or add it to PATH".to_owned(),
            ],
        ),
    }
}

/// The git executable [`GIT_PROGRAM_VAR`] names, when it names one.
fn configured_program(env: &Env) -> Option<&str> {
    env.get(GIT_PROGRAM_VAR)
        .map(String::as_str)
        .filter(|program| !js_trim(program).is_empty())
}

/// The program to spawn for git: [`GIT_PROGRAM_VAR`] when set, otherwise `git` from `env`'s
/// `PATH`.
///
/// On Unix a bare `git` is looked up in the child's `PATH`, which is the one `env` carries. On
/// Windows the standard library falls back to the system directories and this process's own `PATH`
/// when the child's has no match, so git is looked up in `env`'s `PATH` here instead.
///
/// # Errors
///
/// `NotFound` on Windows when no directory on `env`'s `PATH` holds `git.exe`.
#[cfg(windows)]
fn git_program(env: &Env) -> std::io::Result<PathBuf> {
    if let Some(program) = configured_program(env) {
        return Ok(PathBuf::from(program));
    }

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
#[allow(clippy::unnecessary_wraps)] // Fallible on Windows.
fn git_program(env: &Env) -> std::io::Result<PathBuf> {
    Ok(PathBuf::from(configured_program(env).unwrap_or("git")))
}

/// The environment git is run in: the caller's, minus anything that would redirect it.
fn git_environment(env: &Env) -> Env {
    let mut copy = env.clone();

    // Fails instead of prompting for credentials: a prompt on a non-interactive run is
    // indistinguishable from a hang.
    copy.insert("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned());

    for name in REDIRECTING_GIT_VARS {
        copy.remove(*name);
    }

    // Read by ambit, not by git.
    copy.remove(GIT_PROGRAM_VAR);

    copy
}

/// Where the cache lives.
///
/// Read from the environment it is given rather than the process's, so the location is a function
/// of the caller's arguments and a test can point it somewhere disposable.
pub fn cache_root(env: &Env) -> PathBuf {
    if let Some(xdg) = env.get("XDG_CACHE_HOME")
        && !js_trim(xdg).is_empty()
    {
        return join(Path::new(xdg), CACHE_DIRNAME);
    }

    let home = env
        .get("HOME")
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
        .unwrap_or_default();

    join(&home, &format!(".cache/{CACHE_DIRNAME}"))
}

/// Keeps a key segment inside the cache directory, whatever a URL put in it.
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

/// The host and path of a URL with a scheme, as the WHATWG URL parser would report them, or
/// `None` when it would refuse the URL.
///
/// Covers what a git URL can be: `scheme://[user@]host[:port]/path`. The path is percent-encoded
/// the way the parser encodes one, so a key matches what an earlier ambit derived.
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

    // Dot segments are removed, as the parser does for every URL with an authority.
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

/// The host a git URL names, and the path within it, for whichever of the shapes git accepts.
fn split_url(url: &str) -> (String, String) {
    // A URL the parser refuses falls through to the shapes below rather than refusing the source:
    // git may still understand it, and the cache key is ambit's to choose.
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

/// Where a repository is cached, relative to the cache root: `<host>/<path…>`, host, then owner,
/// then repo.
///
/// A trailing `.git` is stripped so `https://github.com/acme/skills` and
/// `https://github.com/acme/skills.git` share one clone, since they are the same repository.
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

/// What git said last, which is where its `fatal:` line lands.
fn last_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .map(js_trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Runs git for one request, in its `cwd` and environment, under its control.
fn git(args: &[&str], request: &GitFetchRequest) -> Result<GitOutcome> {
    run_git_controlled(args, &request.cwd, &request.env, &request.control)
}

/// Runs a git command that talks to the remote, reporting its progress when anything listens.
///
/// `--progress` replaces `--quiet` only then, so the output the CLI sees is unchanged.
fn git_remote(args: &[&str], request: &GitFetchRequest) -> Result<GitOutcome> {
    if !request.control.is_reporting() {
        return git(args, request);
    }

    request.control.report(&Progress {
        stage: Stage::Fetching,
        subject: request.subject.clone(),
        current: 0,
        total: 0,
    });

    let args: Vec<&str> = args
        .iter()
        .map(|&arg| if arg == "--quiet" { "--progress" } else { arg })
        .collect();

    run_git_reporting(
        &args,
        &request.cwd,
        &request.env,
        &request.control,
        Some(&request.subject),
    )
}

/// The error for a git command that failed, carrying git's own word: the line that explains the
/// failure ([`deciding_line`]) when there is one, otherwise its last.
fn git_failed(summary: String, outcome: &GitOutcome, advice: String) -> AmbitError {
    let stderr = deciding_line(&outcome.stderr)
        .map_or_else(|| last_line(&outcome.stderr), |(_, line)| line.to_owned());
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

/// Clones a repository into the cache.
///
/// `--mirror` rather than plain `--bare`, so the clone gets `remote.origin.fetch` and can be
/// updated later, with every tag and branch resolvable without a second network round trip.
///
/// The clone lands beside its final location and is renamed on success, so an interrupted clone
/// never leaves a directory a later run would treat as a cache hit.
///
/// # Errors
///
/// Exit 4 if the clone fails.
fn clone(repo: &Path, request: &GitFetchRequest) -> Result<()> {
    let mut incoming = repo.as_os_str().to_owned();
    incoming.push(INCOMING_SUFFIX);
    let incoming = PathBuf::from(incoming);

    rm_rf(&incoming)?;

    if let Some(parent) = repo.parent() {
        mkdir_p(parent)?;
    }

    let incoming_arg = path_arg(&incoming);
    let outcome = git_remote(
        &[
            "clone",
            "--mirror",
            "--quiet",
            "--",
            &request.url,
            &incoming_arg,
        ],
        request,
    )
    .inspect_err(|_| {
        // A canceled clone leaves nothing behind either.
        let _ = rm_rf(&incoming);
    })?;

    if !outcome.ok {
        rm_rf(&incoming)?;

        return Err(git_failed(
            format!("cannot clone {} {}", request.subject, request.r#where),
            &outcome,
            REACH_ADVICE.to_owned(),
        ));
    }

    rename(&incoming, repo)?;

    Ok(())
}

/// Updates a cached clone.
///
/// # Errors
///
/// Exit 4 if the fetch fails.
fn fetch_into(repo: &Path, request: &GitFetchRequest) -> Result<()> {
    let repo_arg = path_arg(repo);
    let outcome = git_remote(
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

/// Fetches the remote's refs into [`PROBE_NAMESPACE`], leaving the clone's own refs alone.
///
/// Fetched by URL rather than by `origin`. A mirror clone has `remote.origin.mirror = true`, so
/// `git fetch origin <probe refspec>` would apply the mirror's `+refs/*:refs/*` alongside the probe
/// refspecs and update `refs/heads/*` too. An anonymous remote has no configured refspec and no
/// mirror flag, so it fetches only what it is told.
///
/// # Errors
///
/// Exit 4 if the fetch fails.
fn probe_into(repo: &Path, request: &GitFetchRequest) -> Result<()> {
    let repo_arg = path_arg(repo);
    let mut args = vec![
        "-C",
        &repo_arg,
        "fetch",
        "--quiet",
        "--prune",
        // Without this, git also follows tags reachable from what it just fetched into
        // `refs/tags/*`, the clone's own namespace, which a probe must not touch.
        "--no-tags",
        "--",
        &request.url,
    ];
    args.extend(PROBE_REFSPECS.iter().map(String::as_str));

    let outcome = git_remote(&args, request)?;

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

/// The commit a revision names in the cached clone, or `None` if the clone cannot name it.
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

/// The commit a ref names in the cached clone, or `None` if the clone does not have it.
fn resolve_commit(repo: &Path, request: &GitFetchRequest) -> Result<Option<String>> {
    // `HEAD` in a mirror is the remote's default branch, which is what an absent `ref` asks for.
    rev_parse(repo, request.r#ref.as_deref().unwrap_or("HEAD"), request)
}

/// A ref resolved to a commit, and whether the ref it went through is one that can move.
struct GitRefResolution {
    commit: String,
    moving: bool,
}

/// Resolves the request's ref against what a probe just fetched.
///
/// Tries a branch, then a tag, then the ref taken literally as a commit, in the order that decides
/// [`GitRefResolution::moving`]. Only the literal candidate cannot move; a tag counts as moving
/// because a force-pushed tag can point elsewhere.
///
/// The literal candidate is tried last because it also resolves against the clone's own refs, and
/// a stale `refs/heads/main` there would otherwise answer ahead of the current value the probe just
/// fetched.
fn resolve_probed(repo: &Path, request: &GitFetchRequest) -> Result<Option<GitRefResolution>> {
    let candidates: Vec<(String, bool)> = match &request.r#ref {
        None => vec![(format!("{PROBE_NAMESPACE}/HEAD"), true)],
        Some(r#ref) => vec![
            (format!("{PROBE_NAMESPACE}/heads/{ref}"), true),
            (format!("{PROBE_NAMESPACE}/tags/{ref}"), true),
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

/// Whether the request's ref can move, judged against the clone's own refs.
///
/// The [`RefreshMode::Advance`] counterpart of [`resolve_probed`]'s ordering. Called only after a
/// fetch, so the branches and tags checked are the remote's current ones.
fn is_moving_ref(repo: &Path, request: &GitFetchRequest) -> Result<bool> {
    // An absent ref is the default branch, which is a branch.
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

/// Rejects a ref that git would read as something other than a revision.
///
/// # Errors
///
/// Exit 2 for a ref that cannot name one.
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

/// Rejects a pin that is not a full commit SHA.
///
/// The lock reader checks this first, with a message pointing at the file the pin was written in.
/// This is the backstop for every other caller. A string passing [`is_commit_sha`] cannot be a
/// git option and cannot name a branch, so it can be handed to git without a `--` separator.
///
/// # Errors
///
/// Exit 2 for a pin that is not one.
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

/// The error for a ref the repository does not have, after a fetch has already been tried.
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

/// The error for a repository `--offline` would have had to clone.
///
/// Exit 4, not 2: nothing here says the config is wrong. The source may be correct and reachable;
/// it is simply not in the cache yet.
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

/// The error for a refresh `--offline` forbids.
///
/// Refuses rather than falling back to the cache: only the remote knows where a ref points now, so
/// a cached answer under `--offline` would be a stale commit reported as the current one.
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

/// The error for a recorded commit the repository does not have.
///
/// Exit 2, not a fallback to the ref: falling back would silently install a different commit than
/// the lock names, which is what a lock exists to prevent. Happens from a force-push that dropped
/// the commit, or a lock naming a commit that was never pushed; both are fixed by `ambit update`.
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

/// The error for a recorded commit that is not in the cache, which `--offline` may not fetch for.
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

/// The error for a ref the cached clone cannot answer, which `--offline` may not fetch for.
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

/// Resolves a recorded commit against the clone, fetching once if the clone does not have it.
///
/// Ordinarily just a `rev-parse` with no network: ambit wrote this commit into the lock from a
/// clone it had. The fetch covers a warm clone missing it anyway: the project's first run on this
/// machine, or a teammate's push landing after this clone's last fetch.
///
/// `cloned` says whether the clone was made by this call, in which case it already reflects the
/// remote's current state and a fetch would find nothing.
///
/// # Errors
///
/// Exit 4 if the fetch fails or `--offline` forbids it; exit 2 if the repository does not have the
/// commit.
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

/// Materializes one commit as a directory, reusing the checkout if a previous run made it.
///
/// # Errors
///
/// Exit 4 if the checkout fails.
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
    // Clears the registration a half-finished or hand-deleted checkout left behind, which `add`
    // would otherwise refuse to write over.
    git(&["-C", &repo_arg, "worktree", "prune"], request)?;

    let target_arg = path_arg(&target);
    let outcome = git(
        &[
            "-C",
            &repo_arg,
            // A catalog installs the bytes that were committed, whatever line-ending conversion
            // the machine's git config would otherwise apply.
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
    )
    .inspect_err(|_| {
        // A canceled checkout has no ready marker, so the next run redoes it; this only tidies.
        let _ = rm_rf(&target);
    })?;

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

    // Written last: the marker is what a later run trusts, so it must mean the checkout is complete.
    write_text(&ready, &format!("{commit}\n"))?;

    Ok(target)
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
    fetch(request).map_err(|error| AmbitError {
        message: redact(&error.message, &request.env),
        detail: error
            .detail
            .iter()
            .map(|line| redact(line, &request.env))
            .collect(),
        ..error
    })
}

/// Takes the cache lock, waiting while another process holds it.
///
/// # Errors
///
/// [`canceled`] if the caller cancels while waiting; exit 1 if the lock file cannot be opened.
fn lock_cache(cache: &Path, control: &Control) -> Result<FileLock> {
    mkdir_p(cache)?;

    let path = join(cache, CACHE_LOCK_FILENAME);

    loop {
        if let Some(file) = try_lock_file(&path)? {
            return Ok(file);
        }

        control.check()?;
        thread::sleep(POLL_INTERVAL);
    }
}

/// [`fetch_git_source`], before its errors are redacted.
fn fetch(request: &GitFetchRequest) -> Result<FetchedGitSource> {
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

    // Checked before the lock too, so an offline miss creates nothing in the cache.
    if offline && !is_directory(&repo) {
        return Err(not_cached(request, &repo));
    }

    request.control.check()?;

    // Held to the end of the fetch. Whether the clone exists is decided under it, since another
    // process may have cloned while this one waited.
    let _lock = lock_cache(&cache, &request.control)?;
    let mut cloned = false;

    if !is_directory(&repo) {
        if offline {
            return Err(not_cached(request, &repo));
        }

        clone(&repo, request)?;
        cloned = true;
    }

    // Only consulted when nothing is refreshing: a refreshing run was asked for a newer answer than
    // the recorded commit.
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

    request.control.check()?;

    if refresh == RefreshMode::Probe {
        // Needed even right after a clone: the probe namespace is empty until fetched into.
        probe_into(&repo, request)?;
        let probed = resolve_probed(&repo, request)?.ok_or_else(|| unknown_ref(request))?;
        let root = ensure_checkout(&cache, &key, &repo, &probed.commit, request)?;

        return Ok(FetchedGitSource {
            root,
            commit: probed.commit,
            moving: Some(probed.moving),
        });
    }

    // A fresh clone is already the remote's current answer, so advancing it would fetch nothing.
    if refresh == RefreshMode::Advance && !cloned {
        fetch_into(&repo, request)?;
    }

    let mut commit = resolve_commit(&repo, request)?;

    if commit.is_none() && !cloned && refresh != RefreshMode::Advance {
        // Reported as a cache miss, not a config error: only a fetch can tell whether the ref is
        // simply unfetched or genuinely does not exist.
        if offline {
            return Err(ref_not_cached(request));
        }

        fetch_into(&repo, request)?;
        commit = resolve_commit(&repo, request)?;
    }

    let commit = commit.ok_or_else(|| unknown_ref(request))?;

    request.control.check()?;

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

#[cfg(all(test, feature = "cli"))]
mod tests;

#[cfg(test)]
mod runtime_tests;
