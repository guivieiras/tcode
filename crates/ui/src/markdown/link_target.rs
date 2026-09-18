use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LinkTarget {
    Web(String),
    Local(PathBuf),
}

impl LinkTarget {
    pub(super) fn tooltip_text(&self) -> String {
        match self {
            Self::Web(url) => url.clone(),
            Self::Local(path) => path.display().to_string(),
        }
    }
}

/// Classify syntax only. Existence, home expansion and path semantics belong to the host.
pub(super) fn resolve_link(url: &str) -> LinkTarget {
    let windows_path =
        url.as_bytes().get(1) == Some(&b':') && matches!(url.as_bytes().get(2), Some(b'/' | b'\\'));
    let line_suffix = url
        .rsplit_once(':')
        .is_some_and(|(path, line)| !path.is_empty() && line.parse::<u32>().is_ok());
    if !windows_path
        && !url.starts_with("file://")
        && (url.contains("://")
            || url.starts_with("mailto:")
            || url.starts_with("tel:")
            || (!line_suffix && url::Url::parse(url).is_ok()))
    {
        return LinkTarget::Web(url.to_owned());
    }
    LinkTarget::Local(PathBuf::from(url))
}

/// The explicit desktop escape hatch resolves on the viewing machine, as the
/// system opener did before host previews. Preserve literal names before
/// interpreting a trailing source location.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn system_path(target: &str, base_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    let resolve = |target: &str| {
        if target.starts_with("file://") {
            return url::Url::parse(target).ok()?.to_file_path().ok();
        }
        let path = PathBuf::from(target);
        let path = if let Some(rest) = target.strip_prefix("~/") {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            PathBuf::from(home).join(rest)
        } else if path.is_absolute() {
            path
        } else {
            base_dir?.join(path)
        };
        path.exists().then_some(path)
    };
    if let Some(path) = resolve(target) {
        return Some(path);
    }
    let mut target = target;
    if let Some((path, line)) = target.rsplit_once("#L")
        && line.parse::<u32>().is_ok()
    {
        target = path;
    }
    for _ in 0..2 {
        if let Some((path, line)) = target.rsplit_once(':')
            && line.parse::<u32>().is_ok()
        {
            target = path;
        }
    }
    resolve(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_paths_do_not_depend_on_client_files() {
        for path in [
            "/host-only/src/main.rs:42",
            "src/main.rs#L12",
            "README.md:42",
            "Makefile:5",
            "file:///host-only/my%20file.txt",
            "~/notes.md",
            r"C:\work\main.rs:3",
        ] {
            assert_eq!(resolve_link(path), LinkTarget::Local(PathBuf::from(path)));
        }
        for url in [
            "https://example.com",
            "https://localhost:443",
            "mailto:user@example.com",
            "custom:resource",
        ] {
            assert_eq!(resolve_link(url), LinkTarget::Web(url.into()));
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn system_open_resolves_workspace_paths_and_source_locations() {
        let root = std::env::temp_dir().join(format!("tcode-system-link-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("my file.txt");
        std::fs::write(&path, "hello").unwrap();
        for target in [
            "my file.txt".to_owned(),
            "my file.txt:12:3".to_owned(),
            "my file.txt#L12".to_owned(),
            path.display().to_string(),
            url::Url::from_file_path(&path).unwrap().to_string(),
        ] {
            assert_eq!(system_path(&target, Some(&root)), Some(path.clone()));
        }
        assert_eq!(system_path("my file.txt", None), None);
        assert_eq!(system_path("missing.txt", Some(&root)), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
