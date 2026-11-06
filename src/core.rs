//! The core: one live vault. It owns the index, is the only thing watching the vault folder,
//! re-reads just the notes that change, and tells subscribers what changed, whatever caused it
//! (cold-lava, Obsidian, Neovim, git…).

use crate::index::Index;
use crate::vault::{self, NoteChange};
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Something changed in the vault. Note paths are vault-relative, without `.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    NoteCreated(String),
    NoteChanged(String),
    NoteRemoved(String),
    /// The vault was read again from scratch (a folder moved, or a full rescan was asked for):
    /// anything could have changed.
    Rescanned,
    /// A whiteboard (`.canvas`) file changed. Vault-relative path.
    BoardChanged(String),
    /// Obsidian's graph settings (colour groups) changed.
    GraphSettingsChanged,
}

impl From<NoteChange> for Event {
    fn from(c: NoteChange) -> Event {
        match c {
            NoteChange::Created(r) => Event::NoteCreated(r),
            NoteChange::Changed(r) => Event::NoteChanged(r),
            NoteChange::Removed(r) => Event::NoteRemoved(r),
        }
    }
}

/// How long to wait after the last file-system event before reading: editors often write a
/// file in several steps, and a burst of saves should cost one update.
const SETTLE: Duration = Duration::from_millis(250);

pub struct Core {
    pub index: Index,
    fs_events: Option<Receiver<notify::Event>>,
    _watcher: Option<RecommendedWatcher>,
    changed: Vec<PathBuf>,
    boards: Vec<String>,
    rescan: bool,
    graph_settings: bool,
    due: Option<Instant>,
    subscribers: Vec<Sender<Event>>,
}

impl Core {
    pub fn open(root: &Path) -> Result<Core> {
        let index = Index::build(vault::scan(root)?);
        let (fs_events, watcher) = watch(&index.vault.root);
        Ok(Core {
            index,
            fs_events,
            _watcher: watcher,
            changed: Vec::new(),
            boards: Vec::new(),
            rescan: false,
            graph_settings: false,
            due: None,
            subscribers: Vec::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.index.vault.root
    }

    /// Whether the vault folder is being watched (if not, call `note_changed` / `rescan`).
    pub fn is_live(&self) -> bool {
        self.fs_events.is_some()
    }

    /// Get every event from now on. Subscribers that go away are dropped.
    /// This is how extensions will follow the vault (see docs/extensions.md); nothing in
    /// cold-lava itself needs a separate channel yet, since its screens call `poll`.
    #[allow(dead_code)]
    pub fn subscribe(&mut self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        rx
    }

    /// Tell the core a file was just written (e.g. Neovim saved it), so it's picked up sooner
    /// than the watcher alone would, or at all when the folder isn't being watched.
    pub fn note_changed(&mut self, path: &Path) {
        self.changed.push(path.to_path_buf());
        self.schedule(Duration::from_millis(100));
    }

    /// Read the whole vault again on the next `poll`.
    pub fn rescan(&mut self) {
        self.rescan = true;
        self.schedule(Duration::ZERO);
    }

    /// Read the whole vault again right away (after cold-lava itself moved files around).
    /// Subscribers get `Rescanned`.
    pub fn rescan_now(&mut self) {
        if let Ok(v) = vault::scan(&self.index.vault.root) {
            self.index = Index::build(v);
            self.changed.clear();
            self.subscribers
                .retain(|s| s.send(Event::Rescanned).is_ok());
        }
    }

    fn schedule(&mut self, after: Duration) {
        let at = Instant::now() + after;
        self.due = Some(self.due.map_or(at, |d| d.max(at)));
    }

    /// Collect file-system events, and once things settle, update the index and emit events.
    /// Call it regularly (every frame is fine: it's cheap when nothing changed).
    pub fn poll(&mut self) -> Vec<Event> {
        self.collect_fs_events();
        if self.due.is_none_or(|t| Instant::now() < t) {
            return Vec::new();
        }
        self.due = None;
        let mut events: Vec<Event> = Vec::new();
        if std::mem::take(&mut self.rescan) {
            self.changed.clear();
            match vault::scan(self.root()) {
                Ok(v) => self.index = Index::build(v),
                Err(_) => return Vec::new(), // the folder vanished; keep what we had
            }
            events.push(Event::Rescanned);
        } else {
            let paths = std::mem::take(&mut self.changed);
            events.extend(self.index.refresh(&paths).into_iter().map(Event::from));
        }
        events.extend(
            std::mem::take(&mut self.boards)
                .into_iter()
                .map(Event::BoardChanged),
        );
        if std::mem::take(&mut self.graph_settings) {
            events.push(Event::GraphSettingsChanged);
        }
        self.subscribers
            .retain(|s| events.iter().all(|e| s.send(e.clone()).is_ok()));
        events
    }

    fn collect_fs_events(&mut self) {
        let Some(rx) = &self.fs_events else { return };
        let root = self.index.vault.root.clone();
        let mut any = false;
        for ev in rx.try_iter() {
            for p in &ev.paths {
                match classify(&root, p) {
                    Kind::Note => self.changed.push(p.clone()),
                    Kind::Board(rel) => {
                        if !self.boards.contains(&rel) {
                            self.boards.push(rel);
                        }
                    }
                    Kind::GraphSettings => self.graph_settings = true,
                    Kind::Folder => self.rescan = true,
                    Kind::Ignored => continue,
                }
                any = true;
            }
        }
        if any {
            self.schedule(SETTLE);
        }
    }
}

enum Kind {
    Note,
    Board(String),
    GraphSettings,
    /// Something without an extension: most likely a folder that was moved or deleted, which
    /// only a full rescan catches reliably.
    Folder,
    Ignored,
}

fn classify(root: &Path, path: &Path) -> Kind {
    let Ok(rel) = path.strip_prefix(root) else {
        return Kind::Ignored;
    };
    let rel_s = rel.to_string_lossy().replace('\\', "/");
    if rel_s == ".obsidian/graph.json" {
        return Kind::GraphSettings;
    }
    if rel
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Kind::Ignored; // .obsidian, .trash, .git, our own temporary files…
    }
    if vault::is_note_path(rel) {
        return Kind::Note;
    }
    match rel.extension() {
        Some(e) if e == "canvas" => Kind::Board(rel_s),
        // A path without an extension that isn't a file we know: a folder.
        None if !path.is_file() => Kind::Folder,
        _ => Kind::Ignored,
    }
}

fn watch(root: &Path) -> (Option<Receiver<notify::Event>>, Option<RecommendedWatcher>) {
    let (tx, rx) = mpsc::channel();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && !matches!(ev.kind, notify::EventKind::Access(_))
        {
            let _ = tx.send(ev);
        }
    });
    match watcher {
        Ok(mut w) => match w.watch(root, RecursiveMode::Recursive) {
            Ok(()) => (Some(rx), Some(w)),
            Err(_) => (None, None),
        },
        Err(_) => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(core: &mut Core, want: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let mut all = Vec::new();
        for _ in 0..100 {
            all.extend(core.poll());
            if want(&all) {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        all
    }

    #[test]
    fn emits_events_for_outside_changes() {
        let dir = std::env::temp_dir().join(format!("cold-lava-core-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        std::fs::write(dir.join("A.md"), "[[B]]").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let rx = core.subscribe();
        let root = core.root().to_path_buf();

        // Without relying on the watcher: tell the core directly.
        std::fs::write(root.join("B.md"), "hi").unwrap();
        core.note_changed(&root.join("B.md"));
        let ev = wait_for(&mut core, |e| e.contains(&Event::NoteCreated("B".into())));
        assert!(ev.contains(&Event::NoteCreated("B".into())), "{ev:?}");
        assert_eq!(
            core.index.backlinks(core.index.by_rel("B").unwrap()).len(),
            1
        );
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            ev,
            "subscribers get the same events"
        );

        // Through the file watcher, when the platform provides one.
        if core.is_live() {
            std::fs::write(root.join("Sub/C.md"), "x").unwrap();
            let ev = wait_for(&mut core, |e| {
                e.contains(&Event::NoteCreated("Sub/C".into()))
            });
            assert!(ev.contains(&Event::NoteCreated("Sub/C".into())), "{ev:?}");
            std::fs::write(root.join("Board.canvas"), "{}").unwrap();
            let ev = wait_for(&mut core, |e| {
                e.iter().any(|e| matches!(e, Event::BoardChanged(_)))
            });
            assert!(
                ev.contains(&Event::BoardChanged("Board.canvas".into())),
                "{ev:?}"
            );
        }

        core.rescan();
        assert!(wait_for(&mut core, |e| e.contains(&Event::Rescanned)).contains(&Event::Rescanned));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
