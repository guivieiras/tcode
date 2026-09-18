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
}
