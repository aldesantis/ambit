use std::path::{Component, Path, PathBuf};

pub fn normalize(p: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let mut absolute = false;

    for component in p.components() {
        match component {
            Component::Prefix(value) => prefix.push(value.as_os_str()),
            Component::RootDir => {
                prefix.push(component.as_os_str());
                absolute = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|last| last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..".into());
                }
            }
            Component::Normal(name) => parts.push(name.to_owned()),
        }
    }

    let mut normalized = prefix;
    normalized.extend(parts);

    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }

    normalized
}

pub fn join(base: &Path, rel: &str) -> PathBuf {
    let mut joined = base.to_path_buf();

    for component in Path::new(rel).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            other => joined.push(other.as_os_str()),
        }
    }

    normalize(&joined)
}

pub fn resolve(base: &Path, p: &str) -> PathBuf {
    let given = Path::new(p);

    if given.has_root() {
        normalize(given)
    } else {
        normalize(&base.join(given))
    }
}

pub fn relative(from: &Path, to: &Path) -> String {
    let from = normalize(from);
    let to = normalize(to);
    let from_parts: Vec<Component<'_>> = from.components().collect();
    let to_parts: Vec<Component<'_>> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut result = PathBuf::new();

    for part in &from_parts[common..] {
        if matches!(part, Component::Normal(_)) {
            result.push("..");
        }
    }

    for part in &to_parts[common..] {
        result.push(part.as_os_str());
    }

    result.to_string_lossy().into_owned()
}

pub fn to_slash(p: &Path) -> String {
    let text = p.to_string_lossy();

    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_node() {
        assert_eq!(normalize(Path::new("/a/./b/../c/")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"));
        assert_eq!(
            normalize(Path::new("../a/../../b")),
            PathBuf::from("../../b")
        );
        assert_eq!(normalize(Path::new("a/..")), PathBuf::from("."));
        assert_eq!(normalize(Path::new("")), PathBuf::from("."));
    }

    #[test]
    fn joins_like_node() {
        assert_eq!(
            join(Path::new("/p"), "skills/../mcps/x.yml"),
            PathBuf::from("/p/mcps/x.yml")
        );
        assert_eq!(join(Path::new("/p"), "/abs"), PathBuf::from("/p/abs"));
        assert_eq!(join(Path::new("/p/q"), ".."), PathBuf::from("/p"));
        assert_eq!(join(Path::new("/p"), ""), PathBuf::from("/p"));
    }

    #[test]
    fn resolves_like_node() {
        assert_eq!(resolve(Path::new("/p"), "sub/../x"), PathBuf::from("/p/x"));
        assert_eq!(resolve(Path::new("/p"), "/q/./r"), PathBuf::from("/q/r"));
        assert_eq!(resolve(Path::new("/p"), "."), PathBuf::from("/p"));
    }

    #[test]
    fn relates_like_node() {
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b/c/d")), "c/d");
        assert_eq!(relative(Path::new("/a/b/c"), Path::new("/a/x")), "../../x");
        assert_eq!(relative(Path::new("/a"), Path::new("/a")), "");
        assert_eq!(relative(Path::new("/"), Path::new("/a")), "a");
    }

    #[test]
    fn keeps_slashes() {
        assert_eq!(to_slash(Path::new("a/b")), "a/b");
    }
}
