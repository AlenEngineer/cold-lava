//! The core: one live workspace. It owns the index, is the only thing watching the workspace folder,
//! re-reads just the notes that change, and tells subscribers what changed, whatever caused it
//! (cold-lava, Obsidian, Neovim, git…).

use crate::bookmarks::Bookmarks;
use crate::files;
use crate::gateway::{self, Edit, Edited, Found, News, Query, Select, SubId, Subscriptions};
use crate::index::Index;
use crate::owner::cache::{FileRead, Kind as OwnedKind, Owners};
use crate::owner::given::{self, Given};
use crate::workspace::{self, NoteChange, Owned};
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// Another file that isn't a note was created, changed or removed: workspace-relative,
    /// with its extension. Its owner hears `file/changed`.
    FileChanged(String),
    /// The items and links of a file another owner reads changed: its owner answered, or it
    /// was found, or it is gone. Workspace-relative, with its extension.
    OwnedChanged(String),
    /// An owner couldn't read one of its files, or what it gave didn't all read. Its items stay
    /// as they were.
    OwnerTrouble {
        owner: String,
        path: String,
        message: String,
    },
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
    files: Vec<String>,
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
    /// The extensions that own kinds of file, and what they gave.
    owners: Owners,
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
            files: Vec::new(),
            rescan: false,
            graph_settings: false,
            bookmarks_changed: false,
            due: None,
            subscribers: Vec::new(),
            unseen: Vec::new(),
            items: Subscriptions::default(),
            news: Vec::new(),
            owners: Owners::default(),
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
        let files: Vec<String> = (changes.iter())
            .map(|c| match c {
                NoteChange::Created(r) | NoteChange::Changed(r) | NoteChange::Removed(r) => {
                    format!("{r}.md")
                }
            })
            .collect();
        let news = self.items.changed(&self.index.workspace, &files);
        self.news.extend(news);
        changes.into_iter().map(Event::from).collect()
    }

    /// After the workspace was read from scratch.
    fn rescanned(&mut self, index: Index) {
        self.index = index;
        self.reload_bookmarks();
        self.changed.clear();
        if !self.owners.kinds().is_empty() {
            let _ = self.owned_scan();
            self.index.relink();
        }
        let news = self.items.reset(&self.index.workspace);
        self.news.extend(news);
    }

    /// The extensions that own these kinds of file: their files' items and links join the
    /// workspace. What each owner gave before comes from `cache` (a folder; `None`: none), and
    /// it is asked (`take_reads`) for the files it gave nothing for at their version now.
    pub fn set_owners(&mut self, kinds: Vec<OwnedKind>, cache: Option<&Path>) {
        self.owners = Owners::new(self.root(), kinds, cache);
        let had = !self.index.workspace.owned.is_empty();
        self.index.workspace.owned.clear();
        let events = self.owned_scan();
        if had || !events.is_empty() {
            self.index.relink();
            let news = self.items.reset(&self.index.workspace);
            self.news.extend(news);
        }
        self.broadcast(&events);
        self.unseen.extend(events);
    }

    /// Find every file of an owned kind again, and read each the way `owned_changed` does,
    /// without telling subscribers (after a scan, which is a reset). Returns its events.
    fn owned_scan(&mut self) -> Vec<Event> {
        let mut rels = Vec::new();
        for k in self.owners.kinds() {
            rels.extend(files::list(&self.index, &k.extension).files);
        }
        (self.index.workspace.owned).retain(|f| rels.contains(&f.rel));
        let (changed, mut events) = self.owned_read(&rels);
        events.extend(changed.into_iter().map(Event::OwnedChanged));
        events
    }

    /// `file/read` requests for the owners, each with its owner's name: files whose version
    /// they gave nothing for. Their answers come back through `file_given`.
    pub fn take_reads(&mut self) -> Vec<(String, FileRead)> {
        self.owners.take_wanted()
    }

    /// `owner` answered `file/read` for `path` at `rev`: its items and links join the workspace
    /// when the file is still that version; else it is asked again. An error is reported and
    /// leaves the file's items as they were. Its events come from the next `poll`.
    pub fn file_given(&mut self, owner: &str, path: &str, rev: &str, given: Result<Given, String>) {
        if self.owners.owner_of(path) != Some(owner) {
            return;
        }
        self.owners.answered(owner, path, rev, given.as_ref().ok());
        let events = match given {
            Ok(_) => self.owned_changed(&[path.to_string()]),
            Err(message) => vec![Event::OwnerTrouble {
                owner: owner.to_string(),
                path: path.to_string(),
                message,
            }],
        };
        self.broadcast(&events);
        self.unseen.extend(events);
    }

    /// Owned files at `rels` may have changed on disk: read them again, and tell subscribers.
    fn owned_changed(&mut self, rels: &[String]) -> Vec<Event> {
        let (changed, mut events) = self.owned_read(rels);
        if !changed.is_empty() {
            self.index.relink();
            let news = self.items.changed(&self.index.workspace, &changed);
            self.news.extend(news);
        }
        events.extend(changed.into_iter().map(Event::OwnedChanged));
        events
    }

    /// Bring the owned files at `rels` up to date with the disk: what their owner gave for the
    /// version there, else ask it, keeping what it gave before meanwhile. Returns the files whose
    /// items or links changed, and what went wrong. Links are not resolved again here.
    fn owned_read(&mut self, rels: &[String]) -> (Vec<String>, Vec<Event>) {
        let root = self.root().to_path_buf();
        let (mut changed, mut events) = (Vec::new(), Vec::new());
        for rel in rels {
            let Some(owner) = self.owners.owner_of(rel).map(String::from) else {
                continue;
            };
            let owned = &mut self.index.workspace.owned;
            let at = owned.binary_search_by(|f| f.rel.as_str().cmp(rel));
            let Ok(bytes) = std::fs::read(root.join(rel)) else {
                if let Ok(i) = at {
                    owned.remove(i);
                    self.owners.forget(rel);
                    changed.push(rel.clone());
                }
                continue;
            };
            let rev = files::rev(&bytes);
            let i = match at {
                Ok(i) if owned[i].rev.as_deref() == Some(rev.as_str()) => continue,
                Ok(i) => i,
                Err(i) => {
                    let file = Owned {
                        rel: rel.clone(),
                        path: root.join(rel),
                        owner: owner.clone(),
                        rev: None,
                        items: Vec::new(),
                        places: HashMap::new(),
                        links: Vec::new(),
                    };
                    owned.insert(i, file);
                    changed.push(rel.clone());
                    i
                }
            };
            let settings = self.index.workspace.item_settings();
            let Some(given) = self.owners.given(&owner, rel, &rev) else {
                match String::from_utf8(bytes) {
                    Ok(text) => self.owners.ask(
                        &owner,
                        FileRead {
                            path: rel.clone(),
                            rev,
                            text,
                        },
                    ),
                    Err(_) => events.push(Event::OwnerTrouble {
                        owner,
                        path: rel.clone(),
                        message: "it isn't UTF-8 text".into(),
                    }),
                }
                continue;
            };
            let read = given::read(rel, given, settings);
            if !read.problems.is_empty() {
                events.push(Event::OwnerTrouble {
                    owner,
                    path: rel.clone(),
                    message: read.problems.join("; "),
                });
            }
            let file = &mut self.index.workspace.owned[i];
            file.rev = Some(rev);
            file.items = read.items;
            file.places = read.places;
            file.links = read.links;
            if changed.last() != Some(rel) {
                changed.push(rel.clone());
            }
        }
        (changed, events)
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

    /// `workspace/read`
    pub fn read_file(&self, rel: &str) -> Result<files::Text, gateway::Error> {
        files::read(self.root(), rel)
    }

    /// `workspace/write`: refused unless the file is still the version `expect`. A note is read
    /// again at once, and its events come from the next `poll`; other files are the watcher's.
    pub fn write_file(
        &mut self,
        rel: &str,
        text: &str,
        expect: &str,
    ) -> Result<files::Written, gateway::Error> {
        let written = files::replace(self.root(), rel, text, expect)?;
        self.reread_note(rel);
        Ok(written)
    }

    /// `workspace/create`
    pub fn create_file(&mut self, c: &files::Create) -> Result<files::Created, gateway::Error> {
        let created = files::create(&self.index, c)?;
        self.reread_note(&created.path);
        Ok(created)
    }

    /// `workspace/files`
    pub fn files(&self, extension: &str) -> files::Listed {
        files::list(&self.index, extension)
    }

    /// After cold-lava wrote `rel`: when it is a note, read it now, so the next query sees it.
    fn reread_note(&mut self, rel: &str) {
        if workspace::is_note_path(Path::new(rel)) {
            let events = self.refresh(&[self.root().join(rel)]);
            self.broadcast(&events);
            self.unseen.extend(events);
        }
    }

    /// Whether `poll` has events waiting that no file change caused: an edit's.
    pub fn has_unseen(&self) -> bool {
        !self.unseen.is_empty()
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
        self.owners.save();
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
        let files = std::mem::take(&mut self.files);
        let owned: Vec<String> = (files.iter())
            .filter(|f| self.owners.owner_of(f).is_some())
            .cloned()
            .collect();
        events.extend(self.owned_changed(&owned));
        events.extend(files.into_iter().map(Event::FileChanged));
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
                    Kind::File(rel) => {
                        if !self.files.contains(&rel) {
                            self.files.push(rel);
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
    File(String),
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
        Some(_) if !path.is_dir() => Kind::File(rel_s),
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

    #[test]
    fn owned_files_join_the_index_with_what_their_owner_gave() {
        use crate::gateway::{Page, Query, Select};
        use crate::index::From;
        let dir = std::env::temp_dir().join(format!("cold-lava-owned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("w/.cold-lava")).unwrap();
        std::fs::write(dir.join("w/Krakatoa.md"), "# Krakatoa\n").unwrap();
        std::fs::write(dir.join("w/Lava.md"), "See [[Eruptions.csv]].\n").unwrap();
        std::fs::write(dir.join("w/Eruptions.csv"), "volcano,vei\nKrakatoa,6\n").unwrap();
        std::fs::write(
            dir.join("w/.cold-lava/types.toml"),
            "version = 1\n[types.eruption]\non = \"table-row\"\nwhere.header = [\"VEI\"]\n\
             fields.vei = { column = \"VEI\", type = \"number\" }\n\
             fields.volcano = { column = \"Volcano\", type = \"link\" }\n",
        )
        .unwrap();
        let kinds = || {
            vec![OwnedKind {
                extension: "csv".into(),
                owner: "csv-table".into(),
                version: "1".into(),
            }]
        };
        let cache = dir.join("cache");
        let mut core = Core::open(&dir.join("w")).unwrap();
        core.set_owners(kinds(), Some(&cache));
        let reads = core.take_reads();
        assert_eq!(reads.len(), 1);
        let (owner, read) = &reads[0];
        assert_eq!(
            (owner.as_str(), read.path.as_str()),
            ("csv-table", "Eruptions.csv")
        );
        assert!(read.text.starts_with("volcano,vei"));
        let lava = core.index.by_rel("Lava").unwrap();
        assert_eq!(
            core.index.counts(lava).1,
            1,
            "a link to an owned file leads somewhere"
        );
        assert!(core.take_reads().is_empty(), "asked once");

        let tasks = Select {
            kind: "task".into(),
            ..Select::default()
        };
        let sub = core.subscribe_items("agenda", tasks.clone()).unwrap();
        let given: Given = serde_json::from_value(serde_json::json!({
            "items": [
                {"place": "2", "text": "- [ ] Read the report on [[Krakatoa]] 📅 2026-11-20"},
                {"place": "2", "type": "eruption", "fields": {"volcano": "Krakatoa", "vei": "6"}},
                {"place": "3", "type": "nothing"},
            ],
            "links": [{"target": "Krakatoa", "place": "2", "text": "Krakatoa,6"}],
        }))
        .unwrap();
        core.file_given(
            "someone-else",
            "Eruptions.csv",
            &read.rev,
            Ok(given.clone()),
        );
        assert!(
            core.index.workspace.owned[0].items.is_empty(),
            "only its owner answers"
        );
        core.file_given("csv-table", "Eruptions.csv", &read.rev, Ok(given));
        let events = core.poll();
        assert!(
            events.contains(&Event::OwnedChanged("Eruptions.csv".into())),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::OwnerTrouble { message, .. }
                if message.contains("no type called nothing"))),
            "{events:?}"
        );
        let query = |core: &Core, kind: &str| {
            let q = Query {
                select: Select {
                    kind: kind.into(),
                    ..Select::default()
                },
                page: Page::default(),
            };
            core.query_items(&q).unwrap().items
        };
        let found = query(&core, "task");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].note, "Eruptions.csv");
        let places = &core.index.workspace.owned[0].places;
        assert_eq!(places.get(&found[0].id).map(String::as_str), Some("2"));
        let json = serde_json::to_value(&found[0]).unwrap();
        assert_eq!(json["status_type"], "todo");
        assert_eq!(json["due"], "2026-11-20");
        let rows = query(&core, "eruption");
        let json = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(
            (json["vei"].clone(), json["volcano"].clone()),
            (6.into(), "Krakatoa".into())
        );
        let news = core.take_item_news();
        assert!(
            matches!(&news[..], [(who, News::Changed { subscription, added, .. })]
                if who == "agenda" && *subscription == sub && added.len() == 1),
            "{news:?}"
        );
        let krakatoa = core.index.by_rel("Krakatoa").unwrap();
        let backs = core.index.backlinks(krakatoa);
        assert_eq!(backs.len(), 2, "the piece's link and the owner's");
        assert!(backs.iter().all(|b| b.from == From::Owned(0)));
        assert_eq!(core.index.name(backs[0].from), "Eruptions.csv");

        // Opened again, with nothing to ask: what it gave comes from the cache.
        let mut again = Core::open(&dir.join("w")).unwrap();
        again.set_owners(kinds(), Some(&cache));
        assert!(again.take_reads().is_empty());
        assert_eq!(query(&again, "task").len(), 1);

        // Changed on disk: asked again, and what it gave stays until it answers.
        let changed = |core: &mut Core| {
            core.files.push("Eruptions.csv".into());
            core.schedule(Duration::ZERO);
            wait_for(core, |e| {
                e.contains(&Event::OwnedChanged("Eruptions.csv".into()))
            })
        };
        std::fs::write(again.root().join("Eruptions.csv"), "volcano,vei\n").unwrap();
        let mut reads = Vec::new();
        for _ in 0..100 {
            again.files.push("Eruptions.csv".into());
            again.schedule(Duration::ZERO);
            again.poll();
            reads.extend(again.take_reads());
            if !reads.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(reads.len(), 1);
        assert_eq!(query(&again, "task").len(), 1);
        again.file_given("csv-table", "Eruptions.csv", "old", Err("it broke".into()));
        assert!(
            again
                .poll()
                .iter()
                .any(|e| matches!(e, Event::OwnerTrouble { .. }))
        );
        std::fs::remove_file(again.root().join("Eruptions.csv")).unwrap();
        let events = changed(&mut again);
        assert!(events.contains(&Event::OwnedChanged("Eruptions.csv".into())));
        assert!(query(&again, "task").is_empty(), "gone with its file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
