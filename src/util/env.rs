use std::collections::BTreeMap;
use std::path::PathBuf;

pub type Env = BTreeMap<String, String>;

pub fn snapshot() -> Env {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

pub fn home_dir(env: &Env) -> Option<PathBuf> {
    match env.get("HOME") {
        Some(home) => Some(PathBuf::from(home)),
        #[allow(deprecated)]
        None => std::env::home_dir(),
    }
}
