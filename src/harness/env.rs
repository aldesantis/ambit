use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use crate::util::cmp::js_cmp;

static ENV_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("ENV_PLACEHOLDER is a valid pattern")
});

static SOLE_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$").expect("SOLE_PLACEHOLDER is a valid pattern")
});

pub type EnvRefStyle = fn(&str) -> String;

pub fn shell_ref(name: &str) -> String {
    format!("${{{name}}}")
}

pub fn namespaced_ref(name: &str) -> String {
    format!("${{env:{name}}}")
}

pub fn braced_ref(name: &str) -> String {
    format!("{{env:{name}}}")
}

pub fn translate_refs(value: &str, style: EnvRefStyle) -> String {
    ENV_PLACEHOLDER
        .replace_all(value, |captures: &regex::Captures<'_>| style(&captures[1]))
        .into_owned()
}

pub fn referenced_names(value: &str) -> Vec<String> {
    ENV_PLACEHOLDER
        .captures_iter(value)
        .map(|captures| captures[1].to_owned())
        .collect()
}

pub fn sole_reference(value: &str) -> Option<String> {
    SOLE_PLACEHOLDER
        .captures(value)
        .map(|captures| captures[1].to_owned())
}

pub fn stdio_env(
    expected: &[String],
    declared: &IndexMap<String, String>,
    style: EnvRefStyle,
) -> Option<IndexMap<String, String>> {
    let supplies: IndexSet<String> = declared
        .values()
        .flat_map(|value| referenced_names(value))
        .collect();
    let mut env: IndexMap<String, String> = IndexMap::new();

    for name in expected {
        if !supplies.contains(name) {
            env.insert(name.clone(), style(name));
        }
    }

    for (name, value) in declared {
        env.insert(name.clone(), translate_refs(value, style));
    }

    if env.is_empty() {
        return None;
    }

    env.sort_by(|a, _, b, _| js_cmp(a, b));
    Some(env)
}

#[cfg(test)]
mod tests;
