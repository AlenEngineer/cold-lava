//! A path in the workspace as the workspace names it: relative to its root, with `/` between
//! folders.

use std::path::Path;

/// `path` relative to `root`, or `None` when it isn't inside it.
pub fn of(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root).ok().map(slashed)
}

/// `path` relative to `root`, or the whole of it when it isn't inside it.
pub fn or_whole(root: &Path, path: &Path) -> String {
    slashed(path.strip_prefix(root).unwrap_or(path))
}

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_the_workspace_is_named_from_its_root() {
        let root = Path::new("/home/me/Notes");
        let note = Path::new("/home/me/Notes/Projects/Plan.md");
        assert_eq!(of(root, note).as_deref(), Some("Projects/Plan.md"));
        assert_eq!(or_whole(root, note), "Projects/Plan.md");
        assert_eq!(of(root, root).as_deref(), Some(""), "the root itself");
    }

    #[test]
    fn a_path_outside_it_is_none_or_whole() {
        let root = Path::new("/home/me/Notes");
        let outside = Path::new("/home/me/Other/Plan.md");
        assert_eq!(of(root, outside), None);
        assert_eq!(or_whole(root, outside), "/home/me/Other/Plan.md");
        let sibling = Path::new("/home/me/Notes2/Plan.md");
        assert_eq!(
            of(root, sibling),
            None,
            "by whole folder names, not by text"
        );
    }

    #[test]
    fn a_backslash_becomes_a_slash() {
        let root = Path::new("/w");
        assert_eq!(of(root, Path::new("/w/a\\b.md")).as_deref(), Some("a/b.md"));
        assert_eq!(or_whole(root, Path::new("C:\\w\\a.md")), "C:/w/a.md");
    }
}
