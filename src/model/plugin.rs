//! Claude plugin metadata a pack can carry for `ambit export`.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::errors::Result;
use crate::model::yaml::YamlMapping;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginMetadata {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<IndexMap<String, String>>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// External plugin names; local dependencies come from pack requirements.
    pub dependencies: Option<Vec<String>>,
    /// Output directory basename. Defaults to the plugin name.
    pub directory: Option<String>,
    /// Catalog-relative directory copied into the plugin commands directory.
    pub commands: Option<String>,
}

static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("valid regex"));

/// Parses export metadata without changing a pack's installation requirements.
///
/// # Errors
///
/// Exit 2 for a malformed `plugin` block.
pub fn parse_plugin_metadata(mapping: &YamlMapping) -> Result<PluginMetadata> {
    mapping.reject_unknown_keys(&[
        "name",
        "version",
        "description",
        "author",
        "homepage",
        "repository",
        "license",
        "keywords",
        "dependencies",
        "directory",
        "commands",
    ])?;

    let name = mapping.require_string("name")?;

    if !NAME.is_match(&name) {
        return Err(mapping.key_error(
            "name",
            "plugin names must use lowercase letters, digits, and single hyphens",
            vec!["use a name such as `company-engineering`".to_owned()],
        ));
    }

    let mut result = PluginMetadata {
        version: mapping.optional_string("version")?,
        description: mapping.optional_string("description")?,
        homepage: mapping.optional_string("homepage")?,
        repository: mapping.optional_string("repository")?,
        license: mapping.optional_string("license")?,
        directory: mapping.optional_string("directory")?,
        commands: mapping.optional_string("commands")?,
        name,
        ..PluginMetadata::default()
    };

    if let Some(directory) = &result.directory
        && !NAME.is_match(directory)
    {
        return Err(mapping.key_error(
            "directory",
            "plugin directory must be a lowercase hyphenated basename",
            vec!["use a directory such as `engineering` without path separators".to_owned()],
        ));
    }

    if let Some(commands) = &result.commands
        && (commands.starts_with('/')
            || commands.contains('\\')
            || commands
                .split('/')
                .any(|part| part == ".." || part == "." || part.is_empty()))
    {
        return Err(mapping.key_error(
            "commands",
            "commands must name a directory inside the catalog",
            vec!["use a catalog-relative path such as `commands/engineering`".to_owned()],
        ));
    }

    result.keywords = mapping.optional_string_list("keywords")?;
    result.dependencies = mapping.optional_string_list("dependencies")?;

    for dependency in result.dependencies.iter().flatten() {
        if !NAME.is_match(dependency) || *dependency == result.name {
            return Err(mapping.key_error(
                "dependencies",
                &format!("invalid plugin dependency \"{dependency}\""),
                vec!["list other plugins by their lowercase hyphenated names".to_owned()],
            ));
        }
    }

    if let Some(author) = mapping.optional_mapping("author")? {
        author.reject_unknown_keys(&["name", "email", "url"])?;
        author.require_string("name")?;
        result.author = Some(author.string_entries()?);
    }

    let author_url = result
        .author
        .as_ref()
        .and_then(|author| author.get("url"))
        .cloned();

    for (field, url) in [
        ("homepage", result.homepage.clone()),
        ("author.url", author_url),
    ] {
        if let Some(url) = url
            && !can_parse_url(&url)
        {
            return Err(mapping.key_error(
                field,
                &format!("invalid URL \"{url}\""),
                vec!["use an absolute URL such as https://example.com".to_owned()],
            ));
        }
    }

    Ok(result)
}

/// The WHATWG special schemes whose URLs must carry a host.
const SPECIAL_SCHEMES: &[&str] = &["http", "https", "ws", "wss", "ftp"];

static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.\-]*:").expect("valid regex"));

/// Whether `text` is an absolute URL, as the WHATWG parser behind JavaScript's `URL.canParse`
/// judges one.
///
/// A subset of that parser, enough for the URLs a plugin manifest names: a scheme is required, and
/// a special scheme other than `file` must carry a host free of forbidden code points, with a
/// numeric port no greater than 65535 when one is written. Other schemes accept any remainder
/// without spaces in an authority.
fn can_parse_url(text: &str) -> bool {
    let trimmed: String = text
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|&c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();

    let Some(scheme) = SCHEME.find(&trimmed) else {
        return false;
    };

    let scheme_name = trimmed[..scheme.end() - 1].to_ascii_lowercase();
    let rest = &trimmed[scheme.end()..];

    if scheme_name == "file" {
        return true;
    }

    if !SPECIAL_SCHEMES.contains(&scheme_name.as_str()) {
        return match rest.strip_prefix("//") {
            Some(authority) => authority_is_valid(authority, false),
            None => true,
        };
    }

    authority_is_valid(rest.trim_start_matches(['/', '\\']), true)
}

/// Whether the authority at the start of `rest` (up to the path, query, or fragment) holds a host
/// the parser accepts. `special` hosts must be non-empty.
fn authority_is_valid(rest: &str, special: bool) -> bool {
    let end = rest
        .find(|c: char| c == '/' || c == '?' || c == '#' || (special && c == '\\'))
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);

    let (host, port) = if host_and_port.starts_with('[') {
        match host_and_port.find(']') {
            Some(close) => {
                let after = &host_and_port[close + 1..];

                match after.strip_prefix(':') {
                    Some(port) => (&host_and_port[..=close], Some(port)),
                    None if after.is_empty() => (host_and_port, None),
                    None => return false,
                }
            }
            None => return false,
        }
    } else {
        match host_and_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_and_port, None),
        }
    };

    if special && host.is_empty() {
        return false;
    }

    if let Some(port) = port
        && !port.is_empty()
        && !(port.chars().all(|c| c.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|n| n <= 65535))
    {
        return false;
    }

    if host.starts_with('[') {
        return host[1..host.len() - 1]
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }

    let forbidden: &[char] = if special {
        &[
            ' ', '#', '%', '/', ':', '<', '>', '?', '@', '[', '\\', ']', '^', '|',
        ]
    } else {
        &[
            ' ', '#', '/', ':', '<', '>', '?', '@', '[', '\\', ']', '^', '|',
        ]
    };

    !host
        .chars()
        .any(|c| forbidden.contains(&c) || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::can_parse_url;

    #[test]
    fn accepts_absolute_urls_as_url_can_parse_does() {
        for url in [
            "https://example.com",
            "http://example.com:8080/path?q#f",
            "https://user:pass@example.com",
            "mailto:someone@example.com",
            "file:///tmp/x",
            "git+ssh://git@github.com/acme/repo",
            "http://[::1]:3000/",
        ] {
            assert!(can_parse_url(url), "{url}");
        }
    }

    #[test]
    fn refuses_what_url_can_parse_refuses() {
        for url in [
            "example.com",
            "/relative/path",
            "not a url",
            "https://",
            "http:",
            "https://exa mple.com",
            "https://example.com:99999",
            "https://example.com:port",
        ] {
            assert!(!can_parse_url(url), "{url}");
        }
    }
}
