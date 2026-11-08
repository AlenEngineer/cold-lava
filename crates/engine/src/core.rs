//! The core: one live workspace. It owns the index, is the only thing watching the workspace folder,
//! re-reads just the notes that change, and tells subscribers what changed, whatever caused it
//! (cold-lava, Obsidian, Neovim, git…).

use crate::bookmarks::Bookmarks;
use crate::gateway::{self, Edit, Edited, Found, News, Query, Select, SubId, Subscriptions};
use crate::index::Index;
use crate::workspace::{self, NoteChange};
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Something changed in the workspace. Note paths are workspace-relative, without `.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    NoteCreated(String),
    NoteChanged(String),
    NoteRemoved(String),
    /// The workspace was read again from scratch (a folder moved, or a full rescan was asked for):
    /// anything could have changed.
    Rescanned,
    /// A whiteboard (`.canvas`) file changed. Workspace-relative path.
    BoardChanged(String),
    /// Obsidian's graph settings (colour groups) changed.
    GraphSettingsChanged,
    /// `.obsidian/bookmarks.json` changed; `Core::bookmarks` has been read again.
    BookmarksChanged,
    /// An item was changed through the workspace gateway by the extension named `by`, after the
    /// note's own `NoteChanged`. `what` says what, for the status line: "set due", "ticked".
    ItemEdited {
        note: String,
        item: String,
        by: String,
        what: String,
    },
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

/// Where to put the cursor after opening a note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Jump {
    Line(usize),
    Heading(String),
    Block(String),
}

/// How long to wait after the last file-system event before reading: editors often write a
/// file in several steps, and a burst of saves should cost one update.
const SETTLE: Duration = Duration::from_millis(250);

pub struct Core {
    pub index: Index,
    bookmarks: Result<Bookmarks, String>,
    fs_events: Option<Receiver<notify::Event>>,
    _watcher: Option<RecommendedWatcher>,
    changed: Vec<PathBuf>,
    boards: Vec<String>,
    rescan: bool,
    graph_settings: bool,
    bookmarks_changed: bool,
    due: Option<Instant>,
    subscribers: Vec<Sender<Event>>,
    /// Events of the core's own edits, which the next `poll` returns.
    unseen: Vec<Event>,
    items: Subscriptions,
    /// What item subscribers are to hear, with the owner of each subscription.
    news: Vec<(String, News)>,
}

impl Core {
    pub fn open(root: &Path) -> Result<Core> {
        let index = Index::build(workspace::scan(root)?);
        let (fs_events, watcher) = watch(&index.workspace.root);
        Ok(Core {
            bookmarks: read_bookmarks(&index.workspace.root),
            index,
            fs_events,
            _watcher: watcher,
            changed: Vec::new(),
            boards: Vec::new(),
            rescan: false,
            graph_settings: false,
            bookmarks_changed: false,
            due: None,
            subscribers: Vec::new(),
            unseen: Vec::new(),
            items: Subscriptions::default(),
            news: Vec::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.index.workspace.root
    }

    /// The workspace's bookmarks (Obsidian's), or why they can't be read.
    pub fn bookmarks(&self) -> Result<&Bookmarks, &str> {
        self.bookmarks.as_ref().map_err(String::as_str)
    }

    /// Read the bookmarks again now (after cold-lava itself changed them).
    pub fn reload_bookmarks(&mut self) {
        self.bookmarks = read_bookmarks(self.root());
    }

    /// Whether the workspace folder is being watched (if not, call `note_changed` / `rescan`).
    pub fn is_live(&self) -> bool {
        self.fs_events.is_some()
    }

    /// Get every event from now on. Subscribers that go away are dropped.
    /// This is how extensions will follow the workspace (see docs/extensions.md); nothing in
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

    /// Read one note again right away (cold-lava itself just wrote it), so what shows it is up
    /// to date before the next frame. Subscribers get its events too.
    pub fn note_changed_now(&mut self, path: &Path) -> Vec<Event> {
        let events = self.refresh(&[path.to_path_buf()]);
        self.broadcast(&events);
        events
    }

    /// Read these notes again, and queue what item subscribers are to hear about them.
    fn refresh(&mut self, paths: &[PathBuf]) -> Vec<Event> {
        let changes = self.index.refresh(paths);
        let news = self.items.changed(&self.index.workspace, &changes);
        self.news.extend(news);
        changes.into_iter().map(Event::from).collect()
    }

    /// After the workspace was read from scratch.
    fn rescanned(&mut self, index: Index) {
        self.index = index;
        self.reload_bookmarks();
        self.changed.clear();
        let news = self.items.reset(&self.index.workspace);
        self.news.extend(news);
    }

    fn broadcast(&mut self, events: &[Event]) {
        self.subscribers
            .retain(|s| events.iter().all(|e| s.send(e.clone()).is_ok()));
    }

    /// `items/query`
    pub fn query_items(&self, q: &Query) -> Result<Found, gateway::Error> {
        gateway::query(&self.index.workspace, q)
    }

    /// `items/subscribe` for the extension named `owner`: from now on, `take_item_news` has
    /// what it is to hear.
    pub fn subscribe_items(
        &mut self,
        owner: &str,
        select: Select,
    ) -> Result<SubId, gateway::Error> {
        self.items.add(&self.index.workspace, owner, select)
    }

    /// `items/unsubscribe`, of a subscription `owner` made. Whether there was one.
    pub fn unsubscribe_items(&mut self, owner: &str, sub: SubId) -> bool {
        self.items.remove(owner, sub)
    }

    /// `items/changed` and `items/reset` since the last call, each with its subscription's
    /// owner, in order.
    pub fn take_item_news(&mut self) -> Vec<(String, News)> {
        std::mem::take(&mut self.news)
    }

    /// `items/edit` on behalf of the extension named `by`. The edit goes through the safe path
    /// its op has, then the note is read again at once, so the next query sees it; its events,
    /// ending with `ItemEdited`, come from the next `poll`.
    pub fn edit_item(&mut self, by: &str, edit: &Edit) -> Result<Edited, gateway::Error> {
        let written = gateway::write(self.root(), &self.index.workspace, edit)?;
        if written.changed {
            let mut events = self.refresh(&[self.root().join(&written.note)]);
            events.push(Event::ItemEdited {
                note: written.note.trim_end_matches(".md").to_string(),
                item: edit.id.clone(),
                by: by.to_string(),
                what: written.what.clone(),
            });
            self.broadcast(&events);
            self.unseen.extend(events);
        }
        gateway::read_back(&self.index.workspace, written)
    }

    /// Read the whole workspace again on the next `poll`.
    pub fn rescan(&mut self) {
        self.rescan = true;
        self.schedule(Duration::ZERO);
    }

    /// Read the whole workspace again right away (after cold-lava itself moved files around).
    /// Subscribers get `Rescanned`.
    pub fn rescan_now(&mut self) {
        if let Ok(v) = workspace::scan(&self.index.workspace.root) {
            self.rescanned(Index::build(v));
            self.broadcast(&[Event::Rescanned]);
        }
    }

    fn schedule(&mut self, after: Duration) {
        let at = Instant::now() + after;
        self.due = Some(self.due.map_or(at, |d| d.max(at)));
    }

    /// Collect file-system events, and once things settle, update the index and emit events.
    /// Call it regularly (every frame is fine: it's cheap when nothing changed).
    pub fn poll(&mut self) -> Vec<Event> {
        let unseen = std::mem::take(&mut self.unseen);
        self.collect_fs_events();
        if self.due.is_none_or(|t| Instant::now() < t) {
            return unseen;
        }
        self.due = None;
        let mut events: Vec<Event> = Vec::new();
        if std::mem::take(&mut self.rescan) {
            self.changed.clear();
            match workspace::scan(self.root()) {
                Ok(v) => self.rescanned(Index::build(v)),
                Err(_) => return unseen, // the folder vanished; keep what we had
            }
            events.push(Event::Rescanned);
        } else {
            let paths = std::mem::take(&mut self.changed);
            events.extend(self.refresh(&paths));
        }
        events.extend(
            std::mem::take(&mut self.boards)
                .into_iter()
                .map(Event::BoardChanged),
        );
        if std::mem::take(&mut self.graph_settings) {
            events.push(Event::GraphSettingsChanged);
        }
        if std::mem::take(&mut self.bookmarks_changed) {
            self.reload_bookmarks();
            events.push(Event::BookmarksChanged);
        }
        self.broadcast(&events);
        unseen.into_iter().chain(events).collect()
    }

    fn collect_fs_events(&mut self) {
        let Some(rx) = &self.fs_events else { return };
        let root = self.index.workspace.root.clone();
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
                    Kind::Bookmarks => self.bookmarks_changed = true,
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
    Bookmarks,
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
    if rel_s == crate::bookmarks::FILE {
        return Kind::Bookmarks;
    }
    if rel
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Kind::Ignored; // .obsidian, .trash, .git, our own temporary files…
    }
    if workspace::is_note_path(rel) {
        return Kind::Note;
    }
    match rel.extension() {
        Some(e) if e == "canvas" => Kind::Board(rel_s),
        // A path without an extension that isn't a file we know: a folder.
        None if !path.is_file() => Kind::Folder,
        _ => Kind::Ignored,
    }
}

fn read_bookmarks(root: &Path) -> Result<Bookmarks, String> {
    Bookmarks::read(root).map_err(|e| e.to_string())
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

    /// The events seen until `want` holds for them. A `Rescanned` also satisfies it: the
    /// watcher may report the folders a test just made (macOS does, late), and the rescan that
    /// follows already reads what the test is waiting for, so no finer event comes.
    fn wait_for(core: &mut Core, want: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let mut all = Vec::new();
        for _ in 0..100 {
            all.extend(core.poll());
            if want(&all) || all.contains(&Event::Rescanned) {
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
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("A.md"), "[[B]]").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let rx = core.subscribe();
        let root = core.root().to_path_buf();

        // Without relying on the watcher: tell the core directly.
        std::fs::write(root.join("B.md"), "hi").unwrap();
        core.note_changed(&root.join("B.md"));
        let ev = wait_for(&mut core, |e| e.contains(&Event::NoteCreated("B".into())));
        assert!(
            ev.contains(&Event::NoteCreated("B".into())) || ev.contains(&Event::Rescanned),
            "{ev:?}"
        );
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
            assert!(
                ev.contains(&Event::NoteCreated("Sub/C".into())) || ev.contains(&Event::Rescanned),
                "{ev:?}"
            );
            std::fs::write(root.join("Board.canvas"), "{}").unwrap();
            let ev = wait_for(&mut core, |e| {
                e.iter().any(|e| matches!(e, Event::BoardChanged(_)))
            });
            assert!(
                ev.contains(&Event::BoardChanged("Board.canvas".into()))
                    || ev.contains(&Event::Rescanned),
                "{ev:?}"
            );
            std::fs::write(
                root.join(crate::bookmarks::FILE),
                r#"{"items":[{"type":"search","ctime":1,"query":"lava"}]}"#,
            )
            .unwrap();
            let ev = wait_for(&mut core, |e| e.contains(&Event::BookmarksChanged));
            assert!(
                ev.contains(&Event::BookmarksChanged) || ev.contains(&Event::Rescanned),
                "{ev:?}"
            );
            assert_eq!(core.bookmarks().unwrap().entries().len(), 1);
        }

        core.rescan();
        assert!(wait_for(&mut core, |e| e.contains(&Event::Rescanned)).contains(&Event::Rescanned));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
