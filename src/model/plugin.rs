//! Claude plugin metadata a pack can carry for `ambit export`.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use url::Url;

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

/// Whether `text` parses as an absolute URL under the WHATWG URL standard, any scheme allowed.
fn can_parse_url(text: &str) -> bool {
    Url::parse(text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::can_parse_url;

    #[test]
    fn accepts_absolute_urls() {
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
    fn refuses_relative_and_malformed_urls() {
        for url in [
            "example.com",
            "/relative/path",
            "not a url",
            "https://",
            "http:",
            "https://exa mple.com",
            "https://example.com:99999",
            "https://example.com:port",
            "https://999.0.0.1",
            "http://[::g]/",
        ] {
            assert!(!can_parse_url(url), "{url}");
        }
    }
}
