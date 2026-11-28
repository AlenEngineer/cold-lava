//! The watch on a workspace's folder: the one file watcher (CLAUDE.md, `core.rs`). It hears
//! what changed in the folder, and in the folder a linked `.obsidian` leads to, and gives back
//! each path by its names in the workspace (`named`) and what it is (`classify`), which
//! leaves out what the scan doesn't read (`workspace::is_scanned`).

use crate::{workspace, write};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

pub struct Watch {
    root: PathBuf,
    heard: Option<Receiver<notify::Event>>,
    _watcher: Option<RecommendedWatcher>,
    /// `.obsidian`'s own folder, when it is a symlink to one.
    linked_obsidian: Option<PathBuf>,
}

impl Watch {
    /// Watch the workspace at `root`; a folder that can't be watched gives a watch that hears
    /// nothing (`is_live` is false).
    pub fn start(root: &Path) -> Watch {
        let linked_obsidian = linked_obsidian(root);
        let (heard, watcher) = watch(root, linked_obsidian.as_deref());
        Watch {
            root: root.to_path_buf(),
            heard,
            _watcher: watcher,
            linked_obsidian,
        }
    }

    pub fn is_live(&self) -> bool {
        self.heard.is_some()
    }

    /// The paths heard since the last call, each by its names in the workspace (`named`), with
    /// what it is (`classify`). What the core doesn't read (`Kind::Ignored`) is left out.
    pub fn heard(&self) -> Vec<(PathBuf, Kind)> {
        let Some(rx) = &self.heard else {
            return Vec::new();
        };
        let obsidian = self.linked_obsidian.as_deref();
        let mut heard = Vec::new();
        for path in rx.try_iter().flat_map(|ev| ev.paths) {
            for path in named(&self.root, obsidian, &path) {
                match classify(&self.root, &path) {
                    Kind::Ignored => {}
                    kind => heard.push((path, kind)),
                }
            }
        }
        heard
    }

    /// Hear only what is sent on `heard` from now on, instead of the folder: the news harness
    /// plays the watcher.
    #[cfg(test)]
    pub(crate) fn told_by(&mut self, heard: Receiver<notify::Event>) {
        self._watcher = None;
        self.heard = Some(heard);
    }
}

/// What a path the watcher heard, or one cold-lava itself moved, is to the core.
pub enum Kind {
    Note,
    Board(String),
    File(String),
    GraphSettings,
    /// A kept text's name (`write::kept_for`) in a folder the scan reads: one was kept, or put
    /// away (`Core::kept_texts`).
    Kept,
    /// A staged write's name (`write::is_staged`) in a folder the scan reads: one of every
    /// save's, or a killed save's leftover the core lists, which the sweep took away.
    Staged,
    /// Something without an extension: most likely a folder that was moved or deleted, which
    /// only a full rescan catches reliably.
    Folder,
    Ignored,
}

pub fn classify(root: &Path, path: &Path) -> Kind {
    let Ok(rel) = path.strip_prefix(root) else {
        return Kind::Ignored;
    };
    let rel_s = rel.to_string_lossy().replace('\\', "/");
    if rel_s == ".obsidian/graph.json" {
        return Kind::GraphSettings;
    }
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if rel.parent().is_some_and(workspace::is_scanned) {
        if write::kept_for(name).is_some() {
            return Kind::Kept;
        }
        if write::is_staged(name) {
            return Kind::Staged;
        }
    }
    if !workspace::is_scanned(rel) {
        return Kind::Ignored;
    }
    if workspace::is_note_path(rel) {
        return Kind::Note;
    }
    match rel.extension() {
        Some(e) if e == "canvas" => Kind::Board(rel_s),
        Some(_) if !path.is_dir() => Kind::File(rel_s),
        // A path without an extension that isn't a file we know: a folder.
        None if !path.is_file() => Kind::Folder,
        _ => Kind::Ignored,
    }
}

/// `.obsidian`'s own folder, when `.obsidian` is a symlink to a folder: outside the workspace,
/// as people link one set of Obsidian settings into several workspaces, or in it. The watch of
/// the workspace follows no link, so `watch` watches one outside on its own, and `named` names
/// what is heard in it as under `.obsidian`. Read as the watch starts: a link made or removed
/// later is heard after the core opens again.
fn linked_obsidian(root: &Path) -> Option<PathBuf> {
    let at = root.join(".obsidian");
    let link = std::fs::symlink_metadata(&at)
        .ok()?
        .file_type()
        .is_symlink();
    let real = std::fs::canonicalize(&at).ok()?;
    (link && real.is_dir()).then_some(real)
}

/// The names in the workspace of `path`, as the watcher reported it: its own, and, when it is
/// in `obsidian` (the folder a linked `.obsidian` leads to), the same under `.obsidian`, where
/// only `classify`'s settings files come of it. A path outside the workspace comes to nothing
/// under its own name; one in a folder of the workspace, as a note there, still does.
fn named(root: &Path, obsidian: Option<&Path>, path: &Path) -> impl Iterator<Item = PathBuf> {
    let settings = obsidian.and_then(|o| path.strip_prefix(o).ok());
    let settings = settings.map(|rest| root.join(".obsidian").join(rest));
    std::iter::once(path.to_path_buf()).chain(settings)
}

/// The watcher of the folder at `root` and everything in it, following no symlink, as the scan
/// doesn't, and of `obsidian` (a linked `.obsidian`'s folder) without what is inside its
/// folders, when it is outside the workspace: `.obsidian/graph.json` is heard there.
fn watch(
    root: &Path,
    obsidian: Option<&Path>,
) -> (Option<Receiver<notify::Event>>, Option<RecommendedWatcher>) {
    let (tx, rx) = mpsc::channel();
    let heard = move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && !matches!(ev.kind, notify::EventKind::Access(_))
        {
            let _ = tx.send(ev);
        }
    };
    let unfollowed = notify::Config::default().with_follow_symlinks(false);
    let watcher = RecommendedWatcher::new(heard, unfollowed);
    match watcher {
        Ok(mut w) => match w.watch(root, RecursiveMode::Recursive) {
            Ok(()) => {
                if let Some(obsidian) = obsidian.filter(|o| !o.starts_with(root)) {
                    let _ = w.watch(obsidian, RecursiveMode::NonRecursive);
                }
                (Some(rx), Some(w))
            }
            Err(_) => (None, None),
        },
        Err(_) => (None, None),
    }
}
