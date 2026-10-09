#![allow(clippy::case_sensitive_file_extension_comparisons)]

use std::path::Path;
use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use crate::errors::{Result, config_error};
use crate::export::files::{PackageFiles, add_file, collect_files};
use crate::export::resolve::PluginBundle;
use crate::harness::definitions::CLAUDE;
use crate::model::catalog::MergedHook;
use crate::model::hook_entity::{HookType, command_program, script_reference};
use crate::model::mcp_entity::McpTransport;
use crate::model::plugin::PluginMetadata;
use crate::model::yaml::parse_frontmatter_mapping;
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};
use crate::util::path::join;
use crate::util::text::js_trim;

const FILE_MODE: u32 = 0o644;

const PLUGIN_ROOT: &str = "${CLAUDE_PLUGIN_ROOT}/";

static CLAUDE_SKILL_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("a valid pattern"));

static LOCAL_ARGUMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\.{1,2}/|/|[A-Za-z]:[\\/])").expect("a valid pattern"));

static LOCAL_PROGRAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[./~]|[A-Za-z]:[\\/])").expect("a valid pattern"));

static PLUGIN_ASSET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\$\{CLAUDE_PLUGIN_ROOT\}/([^\s"'`]+)"#).expect("a valid pattern")
});

static MARKDOWN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\(([^\s)]+)(?:\s+[^)]*)?\)").expect("a valid pattern"));

static NON_RELATIVE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:[a-z][a-z0-9+.-]*:|/|#|\$)").expect("a valid pattern"));

static SKILL_INVOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:`|\s)/([a-z0-9-]+):([a-z0-9-]+)(?-u:\b)").expect("a valid pattern")
});

fn json(value: &JsonValue) -> Vec<u8> {
    format!("{}\n", stringify_pretty(value)).into_bytes()
}

fn strings(values: &[String]) -> JsonValue {
    JsonValue::Array(values.iter().cloned().map(JsonValue::from).collect())
}

fn plugin_manifest(metadata: &PluginMetadata, dependencies: &[String]) -> JsonValue {
    let mut manifest = JsonObject::new();

    manifest.insert("name".to_owned(), metadata.name.clone().into());

    for (key, value) in [
        ("version", &metadata.version),
        ("description", &metadata.description),
        ("homepage", &metadata.homepage),
        ("repository", &metadata.repository),
        ("license", &metadata.license),
    ] {
        if let Some(value) = value {
            manifest.insert(key.to_owned(), value.clone().into());
        }
    }

    if let Some(keywords) = &metadata.keywords {
        manifest.insert("keywords".to_owned(), strings(keywords));
    }

    if let Some(author) = &metadata.author {
        let author: JsonObject = author
            .iter()
            .map(|(key, value)| (key.clone(), value.clone().into()))
            .collect();

        manifest.insert("author".to_owned(), JsonValue::Object(author));
    }

    if !dependencies.is_empty() {
        manifest.insert("dependencies".to_owned(), strings(dependencies));
    }

    JsonValue::Object(manifest)
}

fn text(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

pub fn render_claude_plugin(plugin: &PluginBundle, catalog_root: &Path) -> Result<PackageFiles> {
    let mut files = PackageFiles::new();

    if let Some(commands) = &plugin.metadata.commands {
        collect_files(
            &mut files,
            &join(catalog_root, commands),
            "commands",
            catalog_root,
            &[],
        )?;

        for (file, entry) in &files {
            if let Some(data) = &entry.data
                && file.ends_with(".md")
            {
                parse_frontmatter_mapping(&text(data), file)?;
            }
        }
    }

    add_file(
        &mut files,
        ".claude-plugin/plugin.json",
        json(&plugin_manifest(&plugin.metadata, &plugin.dependencies)),
        FILE_MODE,
    )?;

    for skill in &plugin.bundle.skills {
        if !CLAUDE_SKILL_NAME.is_match(&skill.name) || skill.name.len() > 64 {
            return Err(config_error(
                format!(
                    "{}/SKILL.md: \"{}\" is not a Claude skill name",
                    skill.path, skill.name
                ),
                [
                    "use a flat skill directory with a lowercase hyphenated name of at most 64 characters",
                ],
            ));
        }

        let destination = format!("skills/{}", skill.name);

        collect_files(
            &mut files,
            &join(&skill.catalog_root, &skill.path),
            &destination,
            &skill.catalog_root,
            &[],
        )?;

        let filename = format!("{destination}/SKILL.md");
        let Some(data) = files.get(&filename).and_then(|file| file.data.as_deref()) else {
            return Err(config_error(
                "cannot export Claude plugins",
                [
                    format!("{filename} is missing"),
                    "check the source files and output directory permissions".to_owned(),
                ],
            ));
        };
        let frontmatter = parse_frontmatter_mapping(&text(data), &filename)?;

        frontmatter.require_string("description")?;

        for key in [
            "argument-hint",
            "model",
            "context",
            "agent",
            "license",
            "compatibility",
        ] {
            frontmatter.optional_string(key)?;
        }

        for key in ["disable-model-invocation", "user-invocable"] {
            frontmatter.optional_boolean(key)?;
        }

        let nested = format!("{destination}/");

        for target in files.keys() {
            if target.starts_with(&nested) && target.ends_with("/SKILL.md") && *target != filename {
                return Err(config_error(
                    format!("{target}: nested skill inside \"{}\"", skill.name),
                    ["move each skill into its own catalog skill directory"],
                ));
            }
        }
    }

    if !plugin.bundle.mcps.is_empty() {
        let mut servers = JsonObject::new();

        for mcp in &plugin.bundle.mcps {
            if let McpTransport::Stdio(stdio) = &mcp.transport {
                if !stdio.command.starts_with(PLUGIN_ROOT)
                    && (stdio.command.contains(['/', '\\']) || stdio.command.starts_with('~'))
                {
                    return Err(config_error(
                        format!("{}: MCP command references a local file", mcp.file),
                        [
                            "use an executable on PATH; exporting local MCP executables is not supported",
                        ],
                    ));
                }

                if stdio
                    .args
                    .iter()
                    .any(|argument| LOCAL_ARGUMENT.is_match(argument))
                {
                    return Err(config_error(
                        format!("{}: MCP argument references a local path", mcp.file),
                        [
                            "bundle the asset in a skill or hook and reference it through ${CLAUDE_PLUGIN_ROOT}, or use a package executable on PATH",
                        ],
                    ));
                }
            }

            servers.insert(mcp.name.clone(), (CLAUDE.server_config)(mcp));
        }

        let mut document = JsonObject::new();
        document.insert("mcpServers".to_owned(), JsonValue::Object(servers));
        add_file(
            &mut files,
            ".mcp.json",
            json(&JsonValue::Object(document)),
            FILE_MODE,
        )?;
    }

    let mut hooks: IndexMap<String, Vec<JsonValue>> = IndexMap::new();

    for hook in &plugin.bundle.hooks {
        let command = hook_command(&mut files, hook)?;
        let mut handler = JsonObject::new();

        handler.insert("type".to_owned(), "command".into());
        handler.insert("command".to_owned(), command.into());

        if let Some(timeout) = hook.timeout {
            handler.insert("timeout".to_owned(), timeout.into());
        }

        let mut entry = JsonObject::new();

        if let Some(matcher) = &hook.matcher {
            entry.insert("matcher".to_owned(), matcher.clone().into());
        }

        entry.insert(
            "hooks".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(handler)]),
        );
        hooks
            .entry(hook.event.as_str().to_owned())
            .or_default()
            .push(JsonValue::Object(entry));
    }

    if !plugin.bundle.hooks.is_empty() {
        let hooks: JsonObject = hooks
            .into_iter()
            .map(|(event, entries)| (event, JsonValue::Array(entries)))
            .collect();
        let mut document = JsonObject::new();
        document.insert("hooks".to_owned(), JsonValue::Object(hooks));
        add_file(
            &mut files,
            "hooks/hooks.json",
            json(&JsonValue::Object(document)),
            FILE_MODE,
        )?;
    }

    validate_package_paths(&files)?;
    validate_markdown_paths(&files)?;

    Ok(files)
}

fn hook_command(files: &mut PackageFiles, hook: &MergedHook) -> Result<String> {
    let command = &hook.command;

    if hook.r#type == HookType::Script {
        collect_files(
            files,
            &join(&hook.catalog_root, &hook.path),
            "hooks",
            &hook.catalog_root,
            &["hook.yml".to_owned(), "hook.yaml".to_owned()],
        )?;

        let program = command_program(command);
        let reference = script_reference(&program);
        let rest = js_trim(command).get(program.len()..).unwrap_or("");

        return Ok(format!("{PLUGIN_ROOT}hooks/{reference}{rest}"));
    }

    if LOCAL_PROGRAM.is_match(&command_program(command)) {
        return Err(config_error(
            format!("{}/hook.yml: command references a local file", hook.path),
            ["use `type: script` and place the script in the hook directory"],
        ));
    }

    Ok(command.clone())
}

fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }

    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();

    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }

    let mut normalized = parts.join("/");

    if normalized.is_empty() && !absolute {
        normalized.push('.');
    }

    if trailing && !normalized.is_empty() {
        normalized.push('/');
    }

    if absolute {
        format!("/{normalized}")
    } else {
        normalized
    }
}

fn posix_dirname(path: &str) -> &str {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) => "/",
        Some(index) => &path[..index],
        None => ".",
    }
}

fn posix_join(a: &str, b: &str) -> String {
    let joined: Vec<&str> = [a, b].into_iter().filter(|part| !part.is_empty()).collect();

    if joined.is_empty() {
        return ".".to_owned();
    }

    posix_normalize(&joined.join("/"))
}

fn validate_package_paths(files: &PackageFiles) -> Result<()> {
    for (file, entry) in files {
        let Some(data) = &entry.data else { continue };

        if file != ".mcp.json" && file != "hooks/hooks.json" {
            continue;
        }

        let text = text(data);

        for captures in PLUGIN_ASSET.captures_iter(&text) {
            let asset = &captures[1];
            let target = posix_normalize(asset);

            if target.starts_with("../") || target == ".." || !files.contains_key(&target) {
                return Err(config_error(
                    format!("{file}: plugin asset \"{asset}\" is missing or escapes the package"),
                    ["reference a file included in this plugin's skills or hooks"],
                ));
            }
        }
    }

    Ok(())
}

fn validate_markdown_paths(files: &PackageFiles) -> Result<()> {
    for (file, entry) in files {
        let Some(data) = &entry.data else { continue };

        if !file.ends_with(".md") {
            continue;
        }

        let text = text(data);

        for captures in MARKDOWN_LINK.captures_iter(&text) {
            let link = &captures[1];

            if NON_RELATIVE_LINK.is_match(link) {
                continue;
            }

            let target = posix_join(posix_dirname(file), link.split('#').next().unwrap_or(""));

            if target == ".." || target.starts_with("../") {
                return Err(config_error(
                    format!("{file}: relative link \"{link}\" escapes the plugin"),
                    [
                        "include the referenced asset inside the plugin or use a plugin skill invocation",
                    ],
                ));
            }
        }
    }

    Ok(())
}

pub fn validate_skill_references(
    plugins: &[PluginBundle],
    rendered: &[PackageFiles],
) -> Result<()> {
    let by_name: IndexMap<&str, &PluginBundle> = plugins
        .iter()
        .map(|plugin| (plugin.metadata.name.as_str(), plugin))
        .collect();

    for (plugin, files) in plugins.iter().zip(rendered) {
        let mut accessible: IndexSet<&str> = IndexSet::new();
        let mut pending = vec![plugin.metadata.name.as_str()];

        while let Some(name) = pending.pop() {
            if !accessible.insert(name) {
                continue;
            }

            if let Some(owner) = by_name.get(name) {
                pending.extend(owner.dependencies.iter().map(String::as_str));
            }
        }

        for (file, entry) in files {
            let Some(data) = &entry.data else { continue };

            if !file.ends_with(".md") {
                continue;
            }

            let text = text(data);

            for captures in SKILL_INVOCATION.captures_iter(&text) {
                let namespace = &captures[1];
                let skill = &captures[2];
                let owner = by_name.get(namespace);

                if !accessible.contains(namespace)
                    || owner.is_some_and(|owner| {
                        !owner.bundle.skills.iter().any(|item| item.name == skill)
                    })
                {
                    return Err(config_error(
                        format!(
                            "{}/{file}: unavailable skill /{namespace}:{skill}",
                            plugin.directory
                        ),
                        ["declare the owning pack as a dependency and use its plugin namespace"],
                    ));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn normalizes_like_posix_path() {
        assert_eq!(posix_normalize("hooks/./a/../b.sh"), "hooks/b.sh");
        assert_eq!(posix_normalize("../x"), "../x");
        assert_eq!(posix_normalize("a/.."), ".");
        assert_eq!(posix_normalize("a/b/"), "a/b/");
        assert_eq!(posix_normalize("/a/../.."), "/");
        assert_eq!(posix_join("skills/x", "../../y.md"), "y.md");
        assert_eq!(posix_join("skills/x", "../../../y.md"), "../y.md");
        assert_eq!(posix_join("skills/x", ""), "skills/x");
        assert_eq!(posix_dirname("skills/x/SKILL.md"), "skills/x");
        assert_eq!(posix_dirname("README.md"), ".");
    }
}
