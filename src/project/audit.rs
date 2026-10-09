use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::errors::{AmbitError, ExitCode, Result, config_error};
use crate::model::catalog::{
    Catalog, CatalogLoadOptions, HOOK_FILENAME, MergedCatalog, MergedHook, MergedMcp, MergedPack,
    MergedSkill, load_catalogs, merge_catalogs,
};
use crate::model::config::load_project_config;
use crate::model::hook_entity::{HookType, command_program};
use crate::model::mcp_entity::McpTransport;
use crate::model::requirement::ItemKind;
use crate::model::sources::SourceContext;
use crate::resolution::resolve::Bundle;
use crate::util::cmp::js_cmp;
use crate::util::fs::{EntryKind, io_message, walk_tree};
use crate::util::path::join;
use crate::util::string_enum;
use crate::util::text::js_trim;

#[cfg(test)]
mod tests;

string_enum! {
    pub enum AuditCheck {
        Invisible => "invisible",
        Bidi => "bidi",
        Tag => "tag",
        MixedScript => "mixed-script",
        Command => "command",
    }
}

string_enum! {
    pub enum AuditSeverity {
        Fail => "fail",
        Warn => "warn",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditFinding {
    pub kind: ItemKind,
    pub name: String,
    pub catalog: String,
    pub check: AuditCheck,
    pub severity: AuditSeverity,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditReport {
    pub items: usize,
    pub findings: Vec<AuditFinding>,
}

impl AuditReport {
    pub fn passed(&self) -> bool {
        self.failures().next().is_none()
    }

    pub fn failures(&self) -> impl Iterator<Item = &AuditFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == AuditSeverity::Fail)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &AuditFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == AuditSeverity::Warn)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentFinding {
    pub check: AuditCheck,
    pub severity: AuditSeverity,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub message: String,
}

struct Suspect {
    check: AuditCheck,
    severity: AuditSeverity,
    singular: &'static str,
    plural: &'static str,
    code: String,
}

fn suspect(
    check: AuditCheck,
    severity: AuditSeverity,
    singular: &'static str,
    plural: &'static str,
    c: char,
) -> Suspect {
    Suspect {
        check,
        severity,
        singular,
        plural,
        code: format!("U+{:04X}", u32::from(c)),
    }
}

fn classify(c: char, at_start: bool) -> Option<Suspect> {
    use AuditCheck::{Bidi, Invisible, Tag};
    use AuditSeverity::{Fail, Warn};

    let found = match c {
        '\u{200B}' => suspect(Invisible, Fail, "zero-width space", "zero-width spaces", c),
        '\u{200C}' => suspect(
            Invisible,
            Fail,
            "zero-width non-joiner",
            "zero-width non-joiners",
            c,
        ),
        '\u{200D}' => suspect(
            Invisible,
            Fail,
            "zero-width joiner",
            "zero-width joiners",
            c,
        ),
        '\u{2060}' => suspect(Invisible, Fail, "word joiner", "word joiners", c),
        '\u{00AD}' => suspect(Invisible, Fail, "soft hyphen", "soft hyphens", c),
        '\u{FEFF}' if !at_start => suspect(
            Invisible,
            Fail,
            "zero-width no-break space",
            "zero-width no-break spaces",
            c,
        ),
        '\u{202A}' => suspect(
            Bidi,
            Fail,
            "left-to-right embedding",
            "left-to-right embeddings",
            c,
        ),
        '\u{202B}' => suspect(
            Bidi,
            Fail,
            "right-to-left embedding",
            "right-to-left embeddings",
            c,
        ),
        '\u{202C}' => suspect(
            Bidi,
            Fail,
            "pop directional formatting",
            "pop directional formattings",
            c,
        ),
        '\u{202D}' => suspect(
            Bidi,
            Fail,
            "left-to-right override",
            "left-to-right overrides",
            c,
        ),
        '\u{202E}' => suspect(
            Bidi,
            Fail,
            "right-to-left override",
            "right-to-left overrides",
            c,
        ),
        '\u{2066}' => suspect(
            Bidi,
            Fail,
            "left-to-right isolate",
            "left-to-right isolates",
            c,
        ),
        '\u{2067}' => suspect(
            Bidi,
            Fail,
            "right-to-left isolate",
            "right-to-left isolates",
            c,
        ),
        '\u{2068}' => suspect(
            Bidi,
            Fail,
            "first strong isolate",
            "first strong isolates",
            c,
        ),
        '\u{2069}' => suspect(
            Bidi,
            Fail,
            "pop directional isolate",
            "pop directional isolates",
            c,
        ),
        '\u{200E}' => suspect(Bidi, Warn, "left-to-right mark", "left-to-right marks", c),
        '\u{200F}' => suspect(Bidi, Warn, "right-to-left mark", "right-to-left marks", c),
        '\u{061C}' => suspect(Bidi, Warn, "Arabic letter mark", "Arabic letter marks", c),
        '\u{E0000}'..='\u{E007F}' => Suspect {
            code: "U+E0000 block".to_owned(),
            ..suspect(Tag, Fail, "tag character", "tag characters", c)
        },
        _ => return None,
    };

    Some(found)
}

pub fn scan_text(text: &str, file: &str) -> Vec<ContentFinding> {
    let mut findings = Vec::new();

    for (index, line) in text.split('\n').enumerate() {
        let mut seen: IndexMap<&'static str, (Suspect, usize)> = IndexMap::new();

        for (offset, c) in line.char_indices() {
            let Some(found) = classify(c, index == 0 && offset == 0) else {
                continue;
            };

            seen.entry(found.singular).or_insert((found, 0)).1 += 1;
        }

        for (_, (found, count)) in seen {
            let what = if count == 1 {
                found.singular.to_owned()
            } else {
                format!("{count} {}", found.plural)
            };

            findings.push(ContentFinding {
                check: found.check,
                severity: found.severity,
                file: Some(file.to_owned()),
                line: Some(index + 1),
                message: format!("{what} ({}) at {file}:{}", found.code, index + 1),
            });
        }
    }

    findings
}

fn confusable_script(c: char) -> Option<&'static str> {
    match c {
        'A'..='Z'
        | 'a'..='z'
        | '\u{00C0}'..='\u{00D6}'
        | '\u{00D8}'..='\u{00F6}'
        | '\u{00F8}'..='\u{024F}'
        | '\u{1E00}'..='\u{1EFF}'
        | '\u{2C60}'..='\u{2C7F}'
        | '\u{A720}'..='\u{A7FF}'
        | '\u{FF21}'..='\u{FF3A}'
        | '\u{FF41}'..='\u{FF5A}' => Some("Latin"),
        '\u{0370}'..='\u{03FF}' | '\u{1F00}'..='\u{1FFF}' => Some("Greek"),
        '\u{0400}'..='\u{052F}'
        | '\u{1C80}'..='\u{1C8F}'
        | '\u{2DE0}'..='\u{2DFF}'
        | '\u{A640}'..='\u{A69F}' => Some("Cyrillic"),
        '\u{0530}'..='\u{058F}' => Some("Armenian"),
        '\u{13A0}'..='\u{13FF}' | '\u{AB70}'..='\u{ABBF}' => Some("Cherokee"),
        _ => None,
    }
}

pub fn mixed_scripts(name: &str) -> Option<Vec<&'static str>> {
    let mut scripts: Vec<&'static str> = Vec::new();

    for script in name.chars().filter_map(confusable_script) {
        if !scripts.contains(&script) {
            scripts.push(script);
        }
    }

    (scripts.len() > 1).then_some(scripts)
}

fn spoken_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

static PIPED_DOWNLOAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(curl|wget|fetch|iwr|irm|invoke-webrequest|invoke-restmethod)\b[^|]*\|\s*(sudo\s+)?(env\s+)?(sh|bash|zsh|dash|ksh|fish|python[0-9.]*|perl|ruby|node|iex|pwsh|powershell)\b",
    )
    .expect("a valid pattern")
});

static SUBSTITUTED_DOWNLOAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(sh|bash|zsh|dash|ksh|fish|python[0-9.]*|perl|ruby|node)\b[^|;&]*(<\(|\$\(|`)\s*(curl|wget)\b",
    )
    .expect("a valid pattern")
});

static REDIRECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]*[<>&]+").expect("a valid pattern"));

static DRIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z]:[\\/]").expect("a valid pattern"));

const HARMLESS_PATHS: &[&str] = &["/dev/null", "/dev/stdin", "/dev/stdout", "/dev/stderr"];

fn outside_reason(word: &str) -> Option<&'static str> {
    if word.is_empty() || word.contains("://") || HARMLESS_PATHS.contains(&word) {
        return None;
    }

    if word.starts_with('~') || word.starts_with("$HOME") || word.starts_with("${HOME}") {
        return Some("a path in the home directory");
    }

    if word.starts_with('/') || word.starts_with('\\') || DRIVE.is_match(word) {
        return Some("an absolute path");
    }

    if word.split(['/', '\\']).any(|segment| segment == "..") {
        return Some("a path that climbs out with `..`");
    }

    None
}

pub fn command_concerns(command: &str) -> Vec<String> {
    let mut concerns = Vec::new();

    if PIPED_DOWNLOAD.is_match(command) || SUBSTITUTED_DOWNLOAD.is_match(command) {
        concerns.push("pipes a download into a shell".to_owned());
    }

    let mut named: Vec<&str> = Vec::new();

    for raw in command.split(|c: char| c.is_whitespace() || ";|&()".contains(c)) {
        let unquoted = raw.trim_matches(['"', '\'']);
        let word = REDIRECTION.replace(unquoted, "");
        let word = word.split_once('=').map_or(&*word, |(_, value)| value);
        let word = word.trim_matches(['"', '\'']);

        let Some(reason) = outside_reason(word) else {
            continue;
        };

        if named.contains(&raw) {
            continue;
        }

        named.push(raw);
        concerns.push(format!("names {word}, {reason}"));
    }

    concerns
}

#[derive(Clone, Copy, Debug)]
pub struct AuditItems<'a> {
    pub packs: &'a [MergedPack],
    pub skills: &'a [MergedSkill],
    pub mcps: &'a [MergedMcp],
    pub hooks: &'a [MergedHook],
}

impl<'a> AuditItems<'a> {
    pub fn of_catalog(merged: &'a MergedCatalog) -> Self {
        Self {
            packs: &merged.packs,
            skills: &merged.skills,
            mcps: &merged.mcps,
            hooks: &merged.hooks,
        }
    }

    pub fn of_bundle(bundle: &'a Bundle) -> Self {
        Self {
            packs: &bundle.packs,
            skills: &bundle.skills,
            mcps: &bundle.mcps,
            hooks: &bundle.hooks,
        }
    }
}

pub fn catalog_roots(catalogs: &[Catalog]) -> IndexMap<String, PathBuf> {
    catalogs
        .iter()
        .map(|catalog| (catalog.name.clone(), catalog.root.clone()))
        .collect()
}

struct Target<'a> {
    kind: ItemKind,
    name: &'a str,
    catalog: &'a str,
    files: Vec<(String, PathBuf)>,
    commands: Vec<(String, String)>,
}

fn unreadable(kind: ItemKind, name: &str, path: &Path, error: &std::io::Error) -> AmbitError {
    config_error(
        format!("cannot read the files of {kind} \"{name}\" to audit them"),
        [
            io_message(error, path),
            "make the catalog readable, then run the command again".to_owned(),
        ],
    )
}

fn directory_files(kind: ItemKind, name: &str, dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = walk_tree(dir).map_err(|error| unreadable(kind, name, dir, &error))?;

    Ok(entries
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .map(|entry| {
            let path = join(dir, &entry.relative);

            (entry.relative, path)
        })
        .collect())
}

fn root_of<'r>(roots: &'r IndexMap<String, PathBuf>, catalog: &str) -> Result<&'r PathBuf> {
    roots.get(catalog).ok_or_else(|| {
        AmbitError::unexpected(format!("catalog \"{catalog}\" was not among those loaded"))
    })
}

fn targets<'a>(
    items: AuditItems<'a>,
    roots: &IndexMap<String, PathBuf>,
) -> Result<Vec<Target<'a>>> {
    let mut packs = Vec::new();
    let mut skills = Vec::new();
    let mut mcps = Vec::new();
    let mut hooks = Vec::new();

    for pack in items.packs {
        packs.push(Target {
            kind: ItemKind::Pack,
            name: &pack.name,
            catalog: &pack.catalog,
            files: vec![(
                pack.file.clone(),
                join(root_of(roots, &pack.catalog)?, &pack.file),
            )],
            commands: Vec::new(),
        });
    }

    for skill in items.skills {
        skills.push(Target {
            kind: ItemKind::Skill,
            name: &skill.name,
            catalog: &skill.catalog,
            files: directory_files(
                ItemKind::Skill,
                &skill.name,
                &join(&skill.catalog_root, &skill.path),
            )?,
            commands: Vec::new(),
        });
    }

    for mcp in items.mcps {
        let commands = match &mcp.transport {
            McpTransport::Stdio(stdio) => {
                let words: Vec<&str> = std::iter::once(stdio.command.as_str())
                    .chain(stdio.args.iter().map(String::as_str))
                    .collect();

                vec![(words.join(" "), mcp.file.clone())]
            }
            McpTransport::Http(_) => Vec::new(),
        };

        mcps.push(Target {
            kind: ItemKind::Mcp,
            name: &mcp.name,
            catalog: &mcp.catalog,
            files: vec![(
                mcp.file.clone(),
                join(root_of(roots, &mcp.catalog)?, &mcp.file),
            )],
            commands,
        });
    }

    for hook in items.hooks {
        let command = match hook.r#type {
            HookType::Command => hook.command.clone(),
            HookType::Script => {
                let trimmed = js_trim(&hook.command);

                trimmed[command_program(trimmed).len()..].to_owned()
            }
        };

        hooks.push(Target {
            kind: ItemKind::Hook,
            name: &hook.name,
            catalog: &hook.catalog,
            files: directory_files(
                ItemKind::Hook,
                &hook.name,
                &join(&hook.catalog_root, &hook.path),
            )?,
            commands: vec![(command, HOOK_FILENAME.to_owned())],
        });
    }

    let mut all = Vec::new();

    for mut group in [packs, skills, mcps, hooks] {
        group.sort_by(|a, b| js_cmp(a.name, b.name).then_with(|| js_cmp(a.catalog, b.catalog)));
        all.extend(group);
    }

    Ok(all)
}

fn audit_target(target: &Target<'_>) -> Result<Vec<ContentFinding>> {
    let mut findings = Vec::new();

    if let Some(scripts) = mixed_scripts(target.name) {
        findings.push(ContentFinding {
            check: AuditCheck::MixedScript,
            severity: AuditSeverity::Warn,
            file: None,
            line: None,
            message: format!(
                "name \"{}\" mixes {} letters",
                target.name,
                spoken_list(&scripts)
            ),
        });
    }

    for (file, path) in &target.files {
        let bytes = std::fs::read(path)
            .map_err(|error| unreadable(target.kind, target.name, path, &error))?;

        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };

        findings.extend(scan_text(&text, file));
    }

    for (command, file) in &target.commands {
        for concern in command_concerns(command) {
            findings.push(ContentFinding {
                check: AuditCheck::Command,
                severity: AuditSeverity::Warn,
                file: Some(file.clone()),
                line: None,
                message: format!("command {concern} in {file}"),
            });
        }
    }

    Ok(findings)
}

pub fn audit_items(
    items: AuditItems<'_>,
    roots: &IndexMap<String, PathBuf>,
) -> Result<AuditReport> {
    let targets = targets(items, roots)?;
    let mut findings = Vec::new();

    for target in &targets {
        findings.extend(audit_target(target)?.into_iter().map(|found| AuditFinding {
            kind: target.kind,
            name: target.name.to_owned(),
            catalog: target.catalog.to_owned(),
            check: found.check,
            severity: found.severity,
            file: found.file,
            line: found.line,
            message: found.message,
        }));
    }

    Ok(AuditReport {
        items: targets.len(),
        findings,
    })
}

pub fn audit_project(context: &SourceContext) -> Result<AuditReport> {
    let config = load_project_config(&context.project_dir)?;
    let catalogs = load_catalogs(&config, context, &mut CatalogLoadOptions::default())?;
    let merged = merge_catalogs(&catalogs);

    audit_items(AuditItems::of_catalog(&merged), &catalog_roots(&catalogs))
}

pub fn item_label(finding: &AuditFinding) -> String {
    format!("{} \"{}\"", finding.kind, finding.name)
}

pub fn refuse_failures(report: &AuditReport) -> Result<()> {
    let failures: Vec<&AuditFinding> = report.failures().collect();

    if failures.is_empty() {
        return Ok(());
    }

    let mut detail: Vec<String> = failures
        .iter()
        .map(|finding| format!("{}: {}", item_label(finding), finding.message))
        .collect();

    detail.push(
        "remove the characters from the catalog, or pass `--no-audit` to install anyway".to_owned(),
    );

    Err(AmbitError::new(
        ExitCode::Doctor,
        format!(
            "the audit found {} in the bundle",
            count(failures.len(), "hidden-text issue", "hidden-text issues")
        ),
        detail,
    ))
}

pub fn count(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}
