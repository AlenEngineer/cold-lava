//! The core: one live workspace. It owns the index, is the only thing watching the workspace folder,
//! re-reads just the notes that change, and tells subscribers what changed, whatever caused it
//! (cold-lava, Obsidian, Neovim, git…).

use crate::act::{Answered, Ask, ShapeAnswer};
use crate::bookmarks::Bookmarks;
use crate::files;
use crate::gateway::{
    self, Answer, Edit, Edited, Found, News, Query, Reason, Select, SubId, Subscriptions, Ticket,
};
use crate::index::Index;
use crate::items::Item;
use crate::owner::cache::{FileRead, Kind as OwnedKind, Owners};
use crate::owner::edits::{Edits, FileEdit, Goal, Pending, Relink, Stage};
use crate::owner::given::{self, Change, Given};
use crate::owner::markdown::{self, After};
use crate::trash::{Bin, system_bin};
use crate::workspace::{self, NoteChange, Owned};
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
    /// note's own `NoteChanged` (or the owned file's `OwnedChanged`; `note` is then its path with
    /// its extension). `what` says what, for the status line: "set due", "ticked".
    ItemEdited {
        note: String,
        item: String,
        by: String,
        what: String,
    },
    /// The client named `by` moved a file or a folder to the trash through the gateway
    /// (`workspace/delete`), after the events of what went. Workspace-relative, with its
    /// extension.
    Deleted {
        path: String,
        by: String,
    },
    /// `by` put it back (`workspace/restore`), after the events of what came back.
    Restored {
        path: String,
        by: String,
    },
    /// `by` made a folder (`workspace/create-folder`).
    FolderCreated {
        path: String,
        by: String,
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
    /// Edits of their files waiting for them.
    edits: Edits,
    /// Extensions' answers to `shape/region` and `shape/gesture`, for the frontend's bindings.
    shape_answers: Vec<ShapeAnswer>,
    /// What `workspace/delete` put in a trash, for `workspace/restore`.
    trashed: files::Trash,
    /// The system trash deletes go to, when the workspace's `trashOption` names it.
    system_trash: Option<Bin>,
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
            edits: Edits::default(),
            shape_answers: Vec::new(),
            trashed: files::Trash::default(),
            // This crate's tests get none unless they set one, so none reaches the real trash.
            system_trash: if cfg!(test) { None } else { system_bin() },
        })
    }

    /// Use `bin` as the system trash, or none: deletes then go to the workspace's `.trash`.
    /// A test that deletes sets one under its own folder.
    pub fn set_system_trash(&mut self, bin: Option<Bin>) {
        self.system_trash = bin;
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
        self.tell(news);
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
        self.tell(news);
    }

    /// The extensions that own these kinds of file: their files' items and links join the
    /// workspace. What each owner gave before comes from `cache` (a folder; `None`: none), and
    /// it is asked (`take_reads`) for the files it gave nothing for at their version now.
    pub fn set_owners(&mut self, kinds: Vec<OwnedKind>, cache: Option<&Path>) {
        self.owners = Owners::new(self.root(), kinds, cache);
        let owned = &mut self.index.workspace.owned;
        let before = owned.len();
        owned.retain(|f| crate::owner::built_in(&f.rel).is_some());
        let dropped = before != owned.len();
        let events = self.owned_scan();
        if dropped || !events.is_empty() {
            self.index.relink();
            let news = self.items.reset(&self.index.workspace);
            self.tell(news);
        }
        self.broadcast(&events);
        self.unseen.extend(events);
    }

    /// Find every file of an owned kind again, the built-in owners' included, and read each the
    /// way `owned_changed` does, without telling subscribers (after a scan, which is a reset).
    /// Returns its events.
    fn owned_scan(&mut self) -> Vec<Event> {
        let mut rels = Vec::new();
        let built_in = crate::owner::BUILT_IN.iter().map(|(x, _)| *x);
        for x in self
            .owners
            .kinds()
            .iter()
            .map(|k| k.extension.as_str())
            .chain(built_in)
        {
            rels.extend(files::list(&self.index, x).files);
        }
        (self.index.workspace.owned).retain(|f| rels.contains(&f.rel));
        let (changed, mut events) = self.owned_read(&rels);
        events.extend(changed.into_iter().map(Event::OwnedChanged));
        events
    }

    /// `file/read` requests for the owners, each with its owner's name: files whose version
    /// they gave nothing for. Their answers come back through `file_given`.
    pub fn take_reads(&mut self) -> Vec<(String, FileRead)> {
        let mut reads = self.owners.take_wanted();
        reads.append(&mut self.edits.reads);
        reads
    }

    /// Whether an owned file waits for its owner to read it (`take_reads`, `file_given`).
    pub fn reading(&self) -> bool {
        self.owners.is_reading()
    }

    /// `file/edit` requests for the owners, each with its owner's name. Their answers come back
    /// through `file_edited`.
    pub fn take_file_edits(&mut self) -> Vec<(String, FileEdit)> {
        std::mem::take(&mut self.edits.asks)
    }

    /// Answers to `items/edit` that `edit_item` answered `Later`: who asked, under which ticket,
    /// and the answer.
    pub fn take_edited(&mut self) -> Vec<(String, Ticket, Result<Edited, gateway::Error>)> {
        std::mem::take(&mut self.edits.done)
    }

    /// `owner` answered `file/read` for `path` at `rev`: its items and links join the workspace
    /// when the file is still that version; else it is asked again. An error is reported and
    /// leaves the file's items as they were. Its events come from the next `poll`.
    pub fn file_given(&mut self, owner: &str, path: &str, rev: &str, given: Result<Given, String>) {
        if self.owners.owner_of(path) != Some(owner) {
            return;
        }
        let reading = |p: &Pending| matches!(&p.stage, Stage::Reading { rev: r, .. } if r == rev);
        if self.edits.pending.get(path).is_some_and(reading) {
            return self.edit_read_back(path, given);
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
            self.tell(news);
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
                    owned.insert(i, Owned::new(rel.clone(), root.join(rel), &owner));
                    changed.push(rel.clone());
                    i
                }
            };
            let trouble = |message: String| Event::OwnerTrouble {
                owner: owner.clone(),
                path: rel.clone(),
                message,
            };
            let given = match self.owners.given(&owner, rel, &rev) {
                Some(given) => given.clone(),
                None => {
                    let Ok(text) = String::from_utf8(bytes) else {
                        events.push(trouble("it isn't UTF-8 text".into()));
                        continue;
                    };
                    match crate::owner::give(&owner, rel, &text) {
                        Some(Ok(given)) => given,
                        Some(Err(message)) => {
                            events.push(trouble(message));
                            continue;
                        }
                        None => {
                            let read = FileRead {
                                path: rel.clone(),
                                rev,
                                text,
                            };
                            self.owners.ask(&owner, read);
                            continue;
                        }
                    }
                }
            };
            let read = given::read(rel, &given, self.index.workspace.item_settings());
            let problems = self.index.workspace.owned[i].take(rev, given, read);
            if !problems.is_empty() {
                events.push(trouble(problems.join("; ")));
            }
            if changed.last() != Some(rel) {
                changed.push(rel.clone());
            }
        }
        (changed, events)
    }

    fn broadcast(&mut self, events: &[Event]) {
        self.subscribers
            .retain(|s| events.iter().all(|e| s.send(e.clone()).is_ok()));
        self.links_news(events);
    }

    /// `links/changed` for the link graph's followers, when `events` may have changed it.
    fn links_news(&mut self, events: &[Event]) {
        let (mut created, mut removed) = (Vec::new(), Vec::new());
        let mut any = false;
        for e in events {
            match e {
                Event::NoteCreated(r) => created.push(format!("{r}.md")),
                Event::NoteRemoved(r) => removed.push(format!("{r}.md")),
                Event::NoteChanged(_)
                | Event::OwnedChanged(_)
                | Event::Rescanned
                | Event::GraphSettingsChanged => {}
                _ => continue,
            }
            any = true;
        }
        if any {
            let news = self.items.links_changed(&created, &removed);
            self.tell(news);
        }
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

    /// `links/query`
    pub fn query_links(&self) -> crate::links::LinkGraph {
        crate::links::graph(&self.index)
    }

    /// `links/subscribe` for the extension named `owner`: `take_item_news` has `links/changed`
    /// for it whenever files were read again.
    pub fn subscribe_links(&mut self, owner: &str) -> SubId {
        self.items.add_links(owner)
    }

    /// `items/changed`, `items/reset` and `links/changed` since the last call, each with its
    /// subscription's owner, in order. An `items/reset` is the last item news of its
    /// subscription in it (`tell`), so a subscriber that queries again when it hears one never
    /// hears of the same change twice, however late the news is taken.
    pub fn take_item_news(&mut self) -> Vec<(String, News)> {
        std::mem::take(&mut self.news)
    }

    /// Queue what subscribers are to hear. Item news of a subscription whose `items/reset`
    /// nobody took yet is dropped: the subscriber queries again when it hears the reset, and
    /// that query finds everything the core read before it.
    fn tell(&mut self, news: Vec<(String, News)>) {
        for (owner, told) in news {
            let subscription = told.subscription();
            let reset = News::Reset { subscription };
            if !self.news.iter().any(|(_, waiting)| *waiting == reset) {
                self.news.push((owner, told));
            }
        }
    }

    /// `items/edit` on behalf of the extension named `by`. In a note, the edit goes through the
    /// safe path its op has, then the note is read again at once, so the next query sees it, and
    /// the answer is `Now`; its events, ending with `ItemEdited`, come from the next `poll`. In a
    /// file another owner reads, the owner is asked for the file's new text, and the answer comes
    /// `Later`, from `take_edited` (`edit_owned`).
    pub fn edit_item(&mut self, by: &str, edit: &Edit) -> Result<Answer, gateway::Error> {
        let owned = &self.index.workspace.owned;
        if let Some(i) = owned.iter().position(|f| f.places.contains_key(&edit.id)) {
            return self.edit_owned(by, i, edit);
        }
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
        let edited = gateway::read_back(&self.index.workspace, written)?;
        Ok(Answer::Now(Box::new(edited)))
    }

    /// `items/edit` on the item `edit.id` of the owned file `owned[i]`: the change is made to
    /// what its owner gave (a Markdown piece's text through the Markdown owner, a typed item's
    /// field as asked), and the owner is asked for the file's new text (`file/edit`). Refused at
    /// once when the item isn't the version `rev`, the file changed since its owner read it, or
    /// another edit of it is waiting for its owner.
    fn edit_owned(&mut self, by: &str, i: usize, edit: &Edit) -> Result<Answer, gateway::Error> {
        let settings = self.index.workspace.item_settings();
        let file = &self.index.workspace.owned[i];
        let error = gateway::Error::new;
        let gone = || {
            let message = format!("{} changed since it was read: read it again", edit.id);
            error(Reason::Changed, message)
        };
        let item = (file.items.iter()).find(|it| it.id == edit.id);
        let item = item.filter(|it| it.rev == edit.rev).ok_or_else(gone)?;
        if let Some(p) = self.edits.pending.get(&file.rel) {
            let message = format!(
                "{} has an edit waiting for {}: try again",
                file.rel, p.owner
            );
            return Err(error(Reason::Changed, message));
        }
        let place = file.places[&edit.id].clone();
        // How the owner gave the item: as a piece, as a typed item, or in a Markdown piece.
        let piece = file.given.items.iter().find_map(|g| match g {
            given::GivenItem::Piece(p) if p.place == place => Some(p),
            _ => None,
        });
        let piece =
            piece.filter(|p| given::piece_id(&file.rel, item.data.kind(), &p.place) == edit.id);
        let is_typed = |g: &given::GivenItem| {
            matches!(g, given::GivenItem::Typed { place: p, kind, .. }
                if *p == place && given::typed_id(&file.rel, kind, p) == edit.id)
        };
        let typed = file.given.items.iter().any(is_typed);
        let markdown = file.given.items.iter().find_map(|g| match g {
            given::GivenItem::Markdown { place: p, text } if *p == place => Some(text),
            _ => None,
        });
        let markdown = markdown.filter(|_| piece.is_none() && !typed);
        let unchanged = || {
            Answer::Now(Box::new(Edited {
                item: item.clone(),
                changed: false,
                next: None,
                warning: None,
            }))
        };
        let (changes, what, warning, after) = match (piece, markdown) {
            (Some(p), _) => {
                let made = crate::owner::change::piece(p, item, &edit.op, settings)?;
                (made.changes, made.what, None, None)
            }
            (None, Some(text)) => {
                let key = format!("{}\0{place}", file.rel);
                let workspace = &self.index.workspace;
                let style = crate::style::Lazy::new(|| workspace.style());
                let today = crate::tasks::today();
                let made = markdown::edit(&key, text, settings, &style, item, edit, today)?;
                if !made.changed {
                    return Ok(unchanged());
                }
                let text = made.text;
                let change = Change::Text { place, text };
                (vec![change], made.what, made.warning, Some(made.after))
            }
            (None, None) => {
                let made = crate::owner::change::typed(&place, item, &edit.op, settings)?;
                (made.changes, made.what, None, None)
            }
        };
        let expect = given::apply(&file.given, &changes);
        let read = given::read(&file.rel, &expect, settings);
        if !read.problems.is_empty() {
            return Err(error(Reason::Refused, read.problems.join("; ")));
        }
        let at = |id: Option<&str>, line: usize| {
            let found = (read.items.iter()).find(|it| match id {
                Some(id) => it.id == id,
                None => read.places.get(&it.id) == file.places.get(&edit.id) && it.line == line,
            });
            found.cloned().map(Box::new)
        };
        let (want, next) = match &after {
            Some(After::Set(set)) => (at(Some(&set.id), 0), None),
            Some(After::Toggled { line, next }) => {
                (at(None, *line), next.and_then(|n| at(None, n)))
            }
            None => (at(Some(&edit.id), 0), None),
        };
        let want = want.ok_or_else(|| {
            let message = format!("{} wouldn't read back after the edit", edit.id);
            error(Reason::ReadBack, message)
        })?;
        if after.is_none() && given::same_data(&want, item) {
            return Ok(unchanged());
        }
        let text = std::fs::read(&file.path)
            .map_err(|e| error(Reason::Io, format!("{}: {e}", file.rel)))?;
        if file.rev.as_deref() != Some(files::rev(&text).as_str()) {
            let message = format!("{} changed since it was read: read it again", file.rel);
            return Err(error(Reason::Changed, message));
        }
        let text = String::from_utf8(text)
            .map_err(|_| error(Reason::Refused, format!("{} isn't UTF-8 text", file.rel)))?;
        let (rel, owner) = (file.rel.clone(), file.owner.clone());
        let ticket = self.edits.ticket();
        let goal = Goal::Item {
            by: by.to_string(),
            ticket,
            item: want,
            next,
            what,
            warning,
        };
        let pending = Pending {
            owner,
            rev: files::rev(text.as_bytes()),
            text,
            expect: Some(expect),
            goal,
            stage: Stage::Editing,
        };
        self.ask_edit(rel, changes, pending);
        Ok(Answer::Later(ticket))
    }

    /// Ask the owner of the file at `rel` for its new text with `changes` made (`file/edit`),
    /// and keep `edit` until it has answered.
    fn ask_edit(&mut self, rel: String, changes: Vec<Change>, edit: Pending) {
        let owner = edit.owner.clone();
        let ask = FileEdit {
            path: rel.clone(),
            rev: edit.rev.clone(),
            text: edit.text.clone(),
            changes,
        };
        self.edits.pending.insert(rel, edit);
        self.send_edit(&owner, ask);
    }

    /// `file/edit` goes to `owner`, and its answer comes in through `file_edited`: from an
    /// extension later, once the frontend has handed it the request (`take_file_edits`); from an
    /// owner in the engine here, before this returns.
    fn send_edit(&mut self, owner: &str, ask: FileEdit) {
        match crate::owner::edit(owner, &ask.path, &ask.text, &ask.changes) {
            Some(text) => self.file_edited(owner, &ask.path, &ask.rev, text),
            None => self.edits.asks.push((owner.to_string(), ask)),
        }
    }

    /// `file/read` of a new text `owner` made goes to it, and its answer comes in through
    /// `file_given`: from an extension later (`take_reads`), from an owner in the engine here.
    fn send_read_back(&mut self, owner: &str, read: FileRead) {
        match crate::owner::give(owner, &read.path, &read.text) {
            Some(given) => self.file_given(owner, &read.path, &read.rev, given),
            None => self.edits.reads.push((owner.to_string(), read)),
        }
    }

    /// `owner` answered `file/edit` for the file at `path` at version `rev`: the file's new
    /// text, or why not. The new text is read through the owner before it is written.
    pub fn file_edited(
        &mut self,
        owner: &str,
        path: &str,
        rev: &str,
        text: Result<String, String>,
    ) {
        let asked = |p: &Pending| p.owner == owner && p.rev == rev;
        let Some(p) = self.edits.pending.get_mut(path).filter(|p| asked(p)) else {
            return;
        };
        if !matches!(p.stage, Stage::Editing) {
            return;
        }
        let error = gateway::Error::new;
        match text {
            Err(why) => {
                let message = format!("{owner} didn't edit {path}: {why}");
                self.edit_failed(path, error(Reason::Refused, message));
            }
            Ok(text) if text == p.text => {
                let message = format!("{owner}'s new text for {path} is the same as before");
                self.edit_failed(path, error(Reason::ReadBack, message));
            }
            Ok(text) => {
                let rev = files::rev(text.as_bytes());
                let read = FileRead {
                    path: path.to_string(),
                    rev: rev.clone(),
                    text: text.clone(),
                };
                p.stage = Stage::Reading { rev, text };
                self.send_read_back(owner, read);
            }
        }
    }

    /// The owner read the new text of the file at `path` (`given`): when it reads as asked,
    /// write it, refused if the file changed since the edit began, and take what it gave as the
    /// file's items and links. Else nothing is written.
    fn edit_read_back(&mut self, path: &str, given: Result<Given, String>) {
        let Some(p) = self.edits.pending.get(path) else {
            return;
        };
        let Stage::Reading { rev, text } = &p.stage else {
            return;
        };
        let error = gateway::Error::new;
        let owner = p.owner.clone();
        let given = match given {
            Ok(g) => g,
            // It couldn't read the file before the change either, and gave nothing for it.
            Err(_) if p.expect.is_none() => Given::default(),
            Err(why) => {
                let message = format!("{owner} couldn't read its new text for {path}: {why}");
                return self.edit_failed(path, error(Reason::ReadBack, message));
            }
        };
        let settings = self.index.workspace.item_settings();
        let nothing = Given::default();
        let asked = given::read_back(path, p.expect.as_ref().unwrap_or(&nothing), settings);
        let gave = given::read_back(path, &given, settings);
        if let Err(diff) = given::same(&asked, &gave) {
            let message = format!(
                "{owner}'s new text for {path} doesn't read back as asked ({diff}); nothing was \
                 written"
            );
            return self.edit_failed(path, error(Reason::ReadBack, message));
        }
        if let Goal::Relink(ticket) = p.goal {
            return self.relink_read_back(ticket, path, text.clone(), given);
        }
        let written =
            crate::write::replace(&self.root().join(path), p.text.as_bytes(), text.as_bytes());
        if let Err(e) = written {
            let reason = match e {
                crate::write::WriteError::Changed(_) => Reason::Changed,
                _ => Reason::Io,
            };
            return self.edit_failed(path, error(reason, format!("{path}: {e}")));
        }
        let rev = rev.clone();
        let Some(p) = self.edits.pending.remove(path) else {
            return;
        };
        self.owners.answered(&owner, path, &rev, Some(&given));
        let mut events = self.owned_changed(&[path.to_string()]);
        if let Goal::Item {
            by,
            ticket,
            item,
            next,
            what,
            warning,
        } = p.goal
        {
            let file = (self.index.workspace.owned.iter()).find(|f| f.rel == path);
            let find = |want: &Item| {
                let items = file.map(|f| f.items.as_slice()).unwrap_or_default();
                (items.iter().find(|i| i.id == want.id))
                    .or_else(|| items.iter().find(|i| given::same_data(i, want)))
                    .cloned()
            };
            let answer = find(&item).map(|found| Edited {
                item: found,
                changed: true,
                next: next.as_deref().and_then(find),
                warning,
            });
            let answer = answer.ok_or_else(|| {
                let message = format!("{path} was written, but {} isn't in it now", item.id);
                error(Reason::ReadBack, message)
            });
            events.push(Event::ItemEdited {
                note: path.to_string(),
                item: item.id.clone(),
                by: by.clone(),
                what,
            });
            self.edits.done.push((by, ticket, answer));
        }
        self.broadcast(&events);
        self.unseen.extend(events);
    }

    /// The edit of the file at `path` waiting for its owner failed: tell whoever asked.
    fn edit_failed(&mut self, path: &str, e: gateway::Error) {
        let Some(p) = self.edits.pending.remove(path) else {
            return;
        };
        match p.goal {
            Goal::Item { by, ticket, .. } => self.edits.done.push((by, ticket, Err(e))),
            Goal::Relink(ticket) => self.relink_failed(ticket, e.message),
        }
    }

    /// Before a rename, merge or recipe writes anything: ask the owner of each file among `edits`
    /// that another owner reads for its new text (`file/edit`), and have the owner read it back.
    /// `None` when `edits` are ready: there was nothing to ask, or every owner answered inside
    /// the call (the engine's own do) and `edits` have their new texts. Else the answer comes
    /// from `take_relinked` under the ticket: every owned file's edit with its new text, or the
    /// first owner's refusal, after which nothing else is kept. Refused at once when one of them
    /// changed since it was planned, has an edit waiting, or an owner in the engine refuses.
    pub fn relink(
        &mut self,
        edits: &mut [crate::rename::FileEdit],
    ) -> Result<Option<Ticket>, String> {
        let owned: Vec<_> = edits
            .iter()
            .filter(|e| e.owned.is_some())
            .cloned()
            .collect();
        if owned.is_empty() {
            return Ok(None);
        }
        for e in &owned {
            let o = e.owned.as_ref().expect("owned");
            if self.edits.pending.contains_key(&e.file) {
                return Err(format!("{} has an edit waiting for {}", e.file, o.owner));
            }
            if std::fs::read_to_string(&e.path).ok().as_ref() != Some(&e.old_text) {
                return Err(format!("{} changed since it was planned", e.file));
            }
        }
        let ticket = self.edits.ticket();
        let asks: Vec<_> = (owned.iter())
            .map(|e| {
                let o = e.owned.as_ref().expect("owned");
                let pending = Pending {
                    owner: o.owner.clone(),
                    rev: files::rev(e.old_text.as_bytes()),
                    text: e.old_text.clone(),
                    expect: (o.given.as_ref()).map(|g| given::apply(g, &o.changes)),
                    goal: Goal::Relink(ticket),
                    stage: Stage::Editing,
                };
                (e.file.clone(), o.changes.clone(), pending)
            })
            .collect();
        self.edits.relinks.insert(ticket, Relink { edits: owned });
        for (file, changes, pending) in asks {
            // An owner in the engine answers inside `ask_edit`, and its refusal ends the wait.
            if self.edits.relinks.contains_key(&ticket) {
                self.ask_edit(file, changes, pending);
            }
        }
        let answered = |(t, _): &(Ticket, _)| *t == ticket;
        let Some(at) = self.edits.relinked.iter().position(answered) else {
            // The answers that are in go into `edits`, so they say whose are still to come.
            let relink = self.edits.relinks.get(&ticket);
            let so_far = relink.map(|r| r.answered()).unwrap_or_default();
            crate::rename::answered(edits, so_far);
            return Ok(Some(ticket));
        };
        let (_, answers) = self.edits.relinked.remove(at);
        crate::rename::answered(edits, answers?);
        Ok(None)
    }

    /// Renames and merges whose owners have all answered (`relink`), or one refused.
    pub fn take_relinked(&mut self) -> Vec<(Ticket, Result<Vec<crate::rename::FileEdit>, String>)> {
        std::mem::take(&mut self.edits.relinked)
    }

    /// Stop waiting for the owners of the rename or merge `ticket`. Their answers are dropped.
    pub fn cancel_relink(&mut self, ticket: Ticket) {
        self.edits.relinks.remove(&ticket);
        let ours = |p: &Pending| matches!(p.goal, Goal::Relink(t) if t == ticket);
        self.edits.pending.retain(|_, p| !ours(p));
        let pending = &self.edits.pending;
        (self.edits.asks).retain(|(_, a)| pending.contains_key(&a.path));
        (self.edits.reads).retain(|(_, r)| pending.contains_key(&r.path));
    }

    /// One owned file of the rename or merge `ticket` read back as asked: keep its new text.
    fn relink_read_back(&mut self, ticket: Ticket, path: &str, text: String, given: Given) {
        self.edits.pending.remove(path);
        let Some(relink) = self.edits.relinks.get_mut(&ticket) else {
            return;
        };
        if let Some(e) = relink.edits.iter_mut().find(|e| e.file == path) {
            e.new_text = text;
            if let Some(o) = e.owned.as_mut() {
                o.after = Some(given);
            }
        }
        if relink.done() {
            let relink = self.edits.relinks.remove(&ticket).expect("waiting");
            self.edits.relinked.push((ticket, Ok(relink.edits)));
        }
    }

    /// An owner refused, or didn't answer, one file of the rename or merge `ticket`: it is
    /// cancelled, with `message`.
    fn relink_failed(&mut self, ticket: Ticket, message: String) {
        if self.edits.relinks.contains_key(&ticket) {
            self.cancel_relink(ticket);
            self.edits.relinked.push((ticket, Err(message)));
        }
    }

    /// After a rename or merge with `edits` was made, before reading the workspace again: what
    /// the owners read in their files' new texts, at `moved` (where each file is now), is kept as
    /// their answer for that version, so they aren't asked again.
    pub fn relinked(
        &mut self,
        edits: &[crate::rename::FileEdit],
        moved: impl Fn(&Path) -> PathBuf,
    ) {
        for e in edits {
            let Some((o, after)) = e.owned.as_ref().and_then(|o| Some((o, o.after.as_ref()?)))
            else {
                continue;
            };
            let path = moved(&e.path);
            let rel = path.strip_prefix(self.root()).unwrap_or(&path);
            let rel = rel.to_string_lossy().replace('\\', "/");
            let rev = files::rev(e.new_text.as_bytes());
            self.owners.answered(&o.owner, &rel, &rev, Some(after));
        }
    }

    /// The answer of the extension named `by` to `ask` (`shape/region`, `shape/gesture`). Only
    /// the extension that ships the shape may answer, and only in the form the ask has.
    pub fn shape_answered(&mut self, by: &str, ask: &Ask, answer: Result<Answered, String>) {
        if !ask.shape().starts_with(&format!("{by}.")) {
            return;
        }
        let answer = answer.and_then(|a| match (ask, a) {
            (Ask::Region { .. }, a @ Answered::Region { .. })
            | (Ask::Gesture { .. }, a @ Answered::Gesture { .. }) => Ok(a),
            (Ask::Region { .. }, _) => Err("it answered shape/region with steps".into()),
            (Ask::Gesture { .. }, _) => Err("it answered shape/gesture with items".into()),
        });
        self.shape_answers.push(ShapeAnswer {
            by: by.to_string(),
            ask: ask.clone(),
            answer,
        });
    }

    /// The extensions' answers since the last call, in order: the frontend hands a region's to
    /// its binding (`Binding::answered`) and runs a gesture's steps (`Binding::run`) through the
    /// extension's gateway.
    pub fn take_shape_answers(&mut self) -> Vec<ShapeAnswer> {
        std::mem::take(&mut self.shape_answers)
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

    /// `workspace/stat`
    pub fn stat_file(&self, rel: &str) -> Result<files::Stat, gateway::Error> {
        files::stat(self.root(), rel)
    }

    /// `workspace/delete` on behalf of the client named `by`: to the trash, refused unless the
    /// file or folder is still the version `expect`. What went is read again at once, and its
    /// events, ending with `Deleted`, come from the next `poll`.
    pub fn delete_file(
        &mut self,
        by: &str,
        rel: &str,
        expect: &str,
    ) -> Result<files::Deleted, gateway::Error> {
        let rel = &files::normal(rel)?;
        let trashed = files::delete(self.root(), rel, expect, self.system_trash.as_ref())?;
        let mut events = self.moved(&trashed.from, trashed.at.is_dir());
        events.push(Event::Deleted {
            path: rel.to_string(),
            by: by.to_string(),
        });
        self.broadcast(&events);
        self.unseen.extend(events);
        let went = trashed.went;
        let trashed = self.trashed.keep(by, trashed);
        Ok(files::Deleted { trashed, went })
    }

    /// `workspace/restore` on behalf of `by`: what a `workspace/delete` of this core answered
    /// goes back where it was, or to `to`, never over something. Its events, ending with
    /// `Restored`, come from the next `poll`. Refused, the token still works.
    pub fn restore_file(
        &mut self,
        by: &str,
        restore: &files::Restore,
    ) -> Result<files::Restored, gateway::Error> {
        let Some(trashed) = self.trashed.get(by, &restore.trashed) else {
            let message = "nothing you deleted goes by that token: it was put back, or \
                           cold-lava started again since it was deleted";
            return Err(gateway::Error::new(Reason::Changed, message));
        };
        let restored = files::restore(self.root(), trashed, restore.to.as_deref())?;
        self.trashed.forget(by, &restore.trashed);
        let path = self.root().join(&restored.path);
        let mut events = self.moved(&path, path.is_dir());
        events.push(Event::Restored {
            path: restored.path.clone(),
            by: by.to_string(),
        });
        self.broadcast(&events);
        self.unseen.extend(events);
        Ok(restored)
    }

    /// Everything `by` deleted through this core, out of it: for a frontend that is about to
    /// drop this core and still wants to undo (`keep_trashed` on the next one). Its tokens
    /// stop working here.
    pub fn take_trashed(&mut self, by: &str) -> files::Kept {
        self.trashed.take(by)
    }

    /// What `take_trashed` gave for `by`, from an earlier core on this workspace: the tokens
    /// work here as they did there.
    pub fn keep_trashed(&mut self, by: &str, kept: files::Kept) {
        self.trashed.give(by, kept);
    }

    /// `workspace/create-folder` on behalf of `by`. An empty folder changes nothing the index
    /// reads; `FolderCreated` comes from the next `poll`.
    pub fn create_folder(&mut self, by: &str, rel: &str) -> Result<files::Folder, gateway::Error> {
        let made = files::create_folder(self.root(), rel)?;
        let events = [Event::FolderCreated {
            path: made.path.clone(),
            by: by.to_string(),
        }];
        self.broadcast(&events);
        self.unseen.extend(events);
        Ok(made)
    }

    /// cold-lava itself just moved the file or folder at `path` out of the workspace or into
    /// it: read what that changes now, as `poll` does what the watcher reports. Its events.
    fn moved(&mut self, path: &Path, folder: bool) -> Vec<Event> {
        let kind = if folder {
            Kind::Folder
        } else {
            classify(self.root(), path)
        };
        match kind {
            Kind::Note => self.refresh(&[path.to_path_buf()]),
            Kind::Board(rel) => {
                let mut events = self.owned_changed(std::slice::from_ref(&rel));
                events.push(Event::BoardChanged(rel));
                events
            }
            Kind::File(rel) => {
                let owned = self.owners.owner_of(&rel).is_some();
                let mut events = Vec::new();
                if owned {
                    events = self.owned_changed(std::slice::from_ref(&rel));
                } else {
                    // Links to it resolve, or stop resolving, from now on.
                    self.index.relink();
                }
                events.push(Event::FileChanged(rel));
                events
            }
            Kind::Folder => match workspace::scan(self.root()) {
                Ok(scanned) => {
                    self.rescanned(Index::build(scanned));
                    vec![Event::Rescanned]
                }
                Err(_) => Vec::new(),
            },
            Kind::GraphSettings | Kind::Bookmarks | Kind::Ignored => Vec::new(),
        }
    }

    /// A recipe planned and applied at once, the files it wrote read again (their events come
    /// from the next `poll`); its title. The files other owners read get their new texts from
    /// their owners first (`relink`). Refused when one of them has to wait for its owner, an
    /// extension: such a recipe runs from its own screen.
    pub fn run_recipe(
        &mut self,
        id: &str,
        options: crate::recipe::Options,
    ) -> Result<String, gateway::Error> {
        use crate::recipe;
        let refused = |m: String| gateway::Error::new(Reason::Refused, m);
        let r = recipe::find(id).ok_or_else(|| refused(format!("no recipe called {id}")))?;
        let mut plan = recipe::plan(&self.index, r, options).map_err(|e| refused(e.to_string()))?;
        let waiting = (self.relink(&mut plan.edits))
            .map_err(|why| refused(format!("{}: {why}", plan.title())))?;
        if let Some(ticket) = waiting {
            let pending = &self.edits.pending;
            let waits = plan.edits.iter().find(|e| pending.contains_key(&e.file));
            let message = format!(
                "{} would change {}, which another owner reads: run it from the recipe screen",
                plan.title(),
                waits.map(|e| e.file.as_str()).unwrap_or_default()
            );
            self.cancel_relink(ticket);
            return Err(refused(message));
        }
        let applied = recipe::apply(&plan)
            .map_err(|e| gateway::Error::new(Reason::Changed, format!("{}: {e}", plan.title())))?;
        let paths: Vec<PathBuf> = plan.edits.iter().map(|e| e.path.clone()).collect();
        let mut events = self.refresh(&paths);
        let owned = plan.edits.iter().filter(|e| e.owned.is_some());
        let owned: Vec<String> = owned.map(|e| e.file.clone()).collect();
        events.extend(self.owned_changed(&owned));
        self.broadcast(&events);
        self.unseen.extend(events);
        Ok(applied.title)
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

    /// Whether `poll` has events waiting that no file change caused (an edit's), or owners have
    /// something to be asked, or an edit an answer to hand out.
    pub fn has_unseen(&self) -> bool {
        !self.unseen.is_empty() || self.owners.has_wanted() || self.edits.has_news()
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
        let boards = std::mem::take(&mut self.boards);
        events.extend(self.owned_changed(&boards));
        events.extend(boards.into_iter().map(Event::BoardChanged));
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

/// The harness for what subscribers hear, and its tests.
#[cfg(test)]
pub(crate) mod news;

/// An owner of a made-up format that isn't Markdown, and what the workspace makes of its pieces.
#[cfg(test)]
mod pieces;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Op;

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
        let csv = core.index.backlinks_of(&core.root().join("Eruptions.csv"));
        assert!(
            matches!(csv, Some([b]) if b.from == From::Note(lava) && b.place.is_none()),
            "the owned file's backlinks: Lava links to it"
        );
        assert_eq!(
            core.index.backlinks_of(&core.root().join("Krakatoa.md")),
            Some(backs),
            "a note's, by its path"
        );

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

    #[test]
    fn items_in_owned_files_are_edited_through_their_owner() {
        use crate::gateway::{Page, Query, Reason, Select};
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("cold-lava-owned-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        std::fs::write(
            dir.join(".cold-lava/types.toml"),
            "version = 1\n[types.eruption]\non = \"table-row\"\nwhere.header = [\"VEI\"]\n\
             fields.vei = { column = \"VEI\", type = \"number\" }\n",
        )
        .unwrap();
        // The owner's format: `vei;task`, one row, its place `1`.
        let csv = |vei: &str, task: &str| format!("{vei};{task}\n");
        let given = |vei: &str, task: &str| -> Given {
            serde_json::from_value(json!({"items": [
                {"place": "1", "text": format!("| VEI |\n| --- |\n| {vei} |")},
                {"place": "1#task", "text": task},
            ]}))
            .unwrap()
        };
        let open = "- [ ] Read the report 📅 2026-11-20";
        std::fs::write(dir.join("Eruptions.csv"), csv("6", open)).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let kind = OwnedKind {
            extension: "csv".into(),
            owner: "csv-table".into(),
            version: "1".into(),
        };
        core.set_owners(vec![kind], None);
        let (_, read) = core.take_reads().pop().unwrap();
        core.file_given(
            "csv-table",
            "Eruptions.csv",
            &read.rev,
            Ok(given("6", open)),
        );
        core.poll();
        let first = |core: &Core, kind: &str| {
            let select = Select {
                kind: kind.into(),
                ..Select::default()
            };
            let q = Query {
                select,
                page: Page::default(),
            };
            core.query_items(&q).unwrap().items.remove(0)
        };
        let csv_now = |core: &Core| std::fs::read_to_string(core.root().join("Eruptions.csv"));
        let tick = |item: &Item| Edit {
            id: item.id.clone(),
            rev: item.rev.clone(),
            op: Op::Toggle,
        };
        let task = first(&core, "task");

        // Ticked: the engine ticks the piece, the owner puts it back and reads it.
        let Ok(Answer::Later(ticket)) = core.edit_item("agenda", &tick(&task)) else {
            panic!("an owned file's item is answered later");
        };
        let busy = core.edit_item("agenda", &tick(&task)).unwrap_err();
        assert_eq!(busy.reason, Reason::Changed, "one edit of a file at a time");
        let (owner, ask) = core.take_file_edits().pop().unwrap();
        assert_eq!(
            (owner.as_str(), ask.text.as_str()),
            ("csv-table", csv("6", open).as_str())
        );
        let Change::Text {
            place,
            text: ticked,
        } = &ask.changes[0]
        else {
            panic!("{ask:?}");
        };
        assert_eq!(place, "1#task");
        assert!(ticked.starts_with("- [x] Read the report"), "{ticked}");
        core.file_edited("csv-table", "Eruptions.csv", &ask.rev, Ok(csv("6", ticked)));
        assert_eq!(
            csv_now(&core).unwrap(),
            csv("6", open),
            "not written before it reads back"
        );
        let (_, back) = core.take_reads().pop().unwrap();
        assert_eq!(back.text, csv("6", ticked));
        core.file_given(
            "csv-table",
            "Eruptions.csv",
            &back.rev,
            Ok(given("6", ticked)),
        );
        assert_eq!(csv_now(&core).unwrap(), csv("6", ticked));
        let done = core.take_edited();
        let [(by, t, Ok(edited))] = &done[..] else {
            panic!("{done:?}");
        };
        assert_eq!((by.as_str(), *t, edited.changed), ("agenda", ticket, true));
        assert_eq!(
            serde_json::to_value(&edited.item).unwrap()["status_type"],
            "done"
        );
        let events = core.poll();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::ItemEdited { note, by, what, .. }
                if note == "Eruptions.csv" && by == "agenda" && what == "ticked")),
            "{events:?}"
        );
        assert!(
            core.take_reads().is_empty(),
            "what it read back is what it gave"
        );

        // An owner that writes something else: refused, and nothing is written.
        let row = first(&core, "eruption");
        let set = |item: &Item, value: serde_json::Value| Edit {
            id: item.id.clone(),
            rev: item.rev.clone(),
            op: Op::SetField {
                field: "vei".into(),
                value,
            },
        };
        let unchanged = core.edit_item("table", &set(&row, json!(6))).unwrap();
        assert!(matches!(unchanged, Answer::Now(e) if !e.changed));
        let refused = |core: &mut Core, answer: Result<&str, &str>, gives: Option<Given>| {
            let Ok(Answer::Later(_)) = core.edit_item("table", &set(&row, json!(7))) else {
                panic!("answered later");
            };
            let (_, ask) = core.take_file_edits().pop().unwrap();
            let answer = answer.map(String::from).map_err(String::from);
            core.file_edited("csv-table", "Eruptions.csv", &ask.rev, answer);
            if let (Some(g), Some((_, back))) = (gives, core.take_reads().pop()) {
                core.file_given("csv-table", "Eruptions.csv", &back.rev, Ok(g));
            }
            let mut done = core.take_edited();
            assert_eq!(csv_now(core).unwrap(), csv("6", ticked), "nothing written");
            done.pop().unwrap().2.unwrap_err()
        };
        let wrong = refused(&mut core, Ok("7;\n"), Some(given("7", "")));
        assert_eq!(wrong.reason, Reason::ReadBack, "{wrong}");
        assert!(wrong.message.contains("nothing was written"), "{wrong}");
        let no = refused(&mut core, Err("it is read-only"), None);
        assert_eq!(no.reason, Reason::Refused);
        assert!(no.message.contains("it is read-only"), "{no}");

        // Set as asked.
        let Ok(Answer::Later(_)) = core.edit_item("table", &set(&row, json!(7))) else {
            panic!("answered later");
        };
        let (_, ask) = core.take_file_edits().pop().unwrap();
        core.file_edited("csv-table", "Eruptions.csv", &ask.rev, Ok(csv("7", ticked)));
        let (_, back) = core.take_reads().pop().unwrap();
        core.file_given(
            "csv-table",
            "Eruptions.csv",
            &back.rev,
            Ok(given("7", ticked)),
        );
        let (_, _, answer) = core.take_edited().pop().unwrap();
        assert_eq!(
            serde_json::to_value(answer.unwrap().item).unwrap()["vei"],
            7
        );
        assert_eq!(csv_now(&core).unwrap(), csv("7", ticked));

        // Stale: an old version is refused at once.
        let stale = core.edit_item("table", &set(&row, json!(5))).unwrap_err();
        assert_eq!(stale.reason, Reason::Changed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rename_asks_owners_for_their_new_texts_before_moving_anything() {
        use crate::rename;
        let dir = std::env::temp_dir().join(format!("cold-lava-owned-mv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Krakatoa.md"), "# Krakatoa\n").unwrap();
        std::fs::write(dir.join("Tambora.md"), "# Tambora\n").unwrap();
        let piece = |to: &str| format!("Read about [[{to}]]");
        let given = |to: &str, link: &str| -> Given {
            serde_json::from_value(serde_json::json!({
                "items": [{"place": "1", "text": piece(to)}],
                "links": [{"target": link, "place": "2"}],
            }))
            .unwrap()
        };
        let csv = |to: &str, link: &str| format!("{};{link}\n", piece(to));
        let csv_now = || std::fs::read_to_string(dir.join("Eruptions.csv")).unwrap();
        std::fs::write(dir.join("Eruptions.csv"), csv("Krakatoa", "Tambora")).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let kind = OwnedKind {
            extension: "csv".into(),
            owner: "csv-table".into(),
            version: "1".into(),
        };
        core.set_owners(vec![kind], None);
        let (_, read) = core.take_reads().pop().unwrap();
        core.file_given(
            "csv-table",
            "Eruptions.csv",
            &read.rev,
            Ok(given("Krakatoa", "Tambora")),
        );
        let plan = |core: &Core, from: &str, to: &str| {
            let note = core.index.by_rel(from).unwrap();
            rename::plan(&core.index, note, to).unwrap()
        };
        // The owner answers `text`, and reads `gives` in it.
        let answer = |core: &mut Core, text: Result<String, &str>, gives: Option<Given>| {
            let (_, ask) = core.take_file_edits().pop().unwrap();
            let text = text.map_err(String::from);
            core.file_edited("csv-table", "Eruptions.csv", &ask.rev, text);
            if let (Some(g), Some((_, back))) = (gives, core.take_reads().pop()) {
                core.file_given("csv-table", "Eruptions.csv", &back.rev, Ok(g));
            }
            ask
        };

        let mut p = plan(&core, "Krakatoa", "Krakatau");
        let owned = p.edits.iter().find(|e| e.file == "Eruptions.csv").unwrap();
        assert_eq!(owned.links, 1, "listed in the plan");
        assert!(rename::apply(&p).is_err(), "not before the owner answered");
        let ticket = core
            .relink(&mut p.edits)
            .unwrap()
            .expect("waits for the owner");
        assert!(
            core.relink(&mut p.edits).is_err(),
            "one edit of a file at a time"
        );
        let ask = answer(
            &mut core,
            Ok(csv("Krakatau", "Tambora")),
            Some(given("Krakatau", "Tambora")),
        );
        assert_eq!(
            ask.changes,
            [Change::Text {
                place: "1".into(),
                text: piece("Krakatau")
            }]
        );
        assert!(dir.join("Krakatoa.md").is_file(), "nothing moved yet");
        assert_eq!(csv_now(), csv("Krakatoa", "Tambora"), "nothing written yet");
        let relinked = core.take_relinked();
        let [(t, Ok(answers))] = &relinked[..] else {
            panic!("{relinked:?}");
        };
        assert_eq!(*t, ticket);
        rename::answered(&mut p.edits, answers.clone());
        rename::apply(&p).unwrap();
        core.relinked(&p.edits, |path| p.moved(path));
        core.rescan_now();
        assert_eq!(csv_now(), csv("Krakatau", "Tambora"));
        assert!(core.take_reads().is_empty(), "what it read back is kept");
        let krakatau = core.index.by_rel("Krakatau").unwrap();
        assert_eq!(
            core.index.backlinks(krakatau).len(),
            1,
            "its link leads there now"
        );

        // A link the owner reported, and an owner that doesn't write what was asked, or
        // refuses: the rename is cancelled and nothing changes.
        let mut p = plan(&core, "Tambora", "Tambora volcano");
        let ticket = core.relink(&mut p.edits).unwrap().unwrap();
        let ask = answer(
            &mut core,
            Ok(csv("Krakatau", "x")),
            Some(given("Krakatau", "x")),
        );
        assert!(matches!(&ask.changes[..], [Change::Link { link, to, .. }]
            if link == "Tambora" && to == "Tambora volcano"));
        let relinked = core.take_relinked();
        assert!(
            matches!(&relinked[..], [(t, Err(m))] if *t == ticket && m.contains("links to x")),
            "{relinked:?}"
        );
        core.relink(&mut p.edits).unwrap().unwrap();
        answer(&mut core, Err("it is read-only"), None);
        let relinked = core.take_relinked();
        assert!(
            matches!(&relinked[..], [(_, Err(m))]
                if m == "csv-table didn't edit Eruptions.csv: it is read-only"),
            "{relinked:?}"
        );

        // Cancelled while it waits: a late answer is dropped.
        let ticket = core.relink(&mut p.edits).unwrap().unwrap();
        core.cancel_relink(ticket);
        assert!(core.take_file_edits().is_empty(), "not asked after all");
        core.file_edited("csv-table", "Eruptions.csv", &ask.rev, Ok(csv("a", "b")));
        assert!(core.take_relinked().is_empty() && core.take_reads().is_empty());
        assert!(dir.join("Tambora.md").is_file());
        assert_eq!(csv_now(), csv("Krakatau", "Tambora"), "nothing written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recipe_changes_owned_files_through_their_owner() {
        use crate::{recipe, rename};
        let dir =
            std::env::temp_dir().join(format!("cold-lava-owned-recipe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Note.md"), "#lava\n").unwrap();
        let given = |tag: &str| -> Given {
            serde_json::from_value(serde_json::json!({
                "items": [{"place": "1", "text": format!("- [ ] sample #{tag}/ash")}],
            }))
            .unwrap()
        };
        let csv = |tag: &str| format!("- [ ] sample #{tag}/ash\n");
        std::fs::write(dir.join("Samples.csv"), csv("lava")).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let kind = OwnedKind {
            extension: "csv".into(),
            owner: "csv-table".into(),
            version: "1".into(),
        };
        core.set_owners(vec![kind], None);
        let (_, read) = core.take_reads().pop().unwrap();
        core.file_given("csv-table", "Samples.csv", &read.rev, Ok(given("lava")));
        let options = recipe::Options::from([
            ("from".to_string(), "lava".to_string()),
            ("to".to_string(), "magma".to_string()),
        ]);
        let mut p =
            recipe::plan(&core.index, recipe::find("rename-tag").unwrap(), options).unwrap();
        let files: Vec<_> = p.edits.iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, ["Note.md", "Samples.csv"]);
        assert!(recipe::apply(&p).is_err(), "not before the owner answered");
        let ticket = core
            .relink(&mut p.edits)
            .unwrap()
            .expect("waits for the owner");
        let (_, ask) = core.take_file_edits().pop().unwrap();
        assert_eq!(
            ask.changes,
            [Change::Text {
                place: "1".into(),
                text: "- [ ] sample #magma/ash".into()
            }]
        );
        core.file_edited("csv-table", "Samples.csv", &ask.rev, Ok(csv("magma")));
        let (_, back) = core.take_reads().pop().unwrap();
        core.file_given("csv-table", "Samples.csv", &back.rev, Ok(given("magma")));
        let relinked = core.take_relinked();
        let [(t, Ok(answers))] = &relinked[..] else {
            panic!("{relinked:?}");
        };
        assert_eq!(*t, ticket);
        assert_eq!(
            std::fs::read_to_string(dir.join("Note.md")).unwrap(),
            "#lava\n"
        );
        rename::answered(&mut p.edits, answers.clone());
        let done = recipe::apply(&p).unwrap();
        core.relinked(&p.edits, Path::to_path_buf);
        assert_eq!(
            std::fs::read_to_string(dir.join("Note.md")).unwrap(),
            "#magma\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("Samples.csv")).unwrap(),
            csv("magma")
        );
        recipe::undo(&done).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("Note.md")).unwrap(),
            "#lava\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("Samples.csv")).unwrap(),
            csv("lava")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merge_relinks_owned_files_through_their_owner() {
        use crate::{merge, rename};
        let dir =
            std::env::temp_dir().join(format!("cold-lava-owned-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        let ws = dir.join("ws");
        std::fs::write(ws.join("Krakatoa.md"), "# Krakatoa\n").unwrap();
        std::fs::write(ws.join("Krakatau.md"), "# Krakatau\n").unwrap();
        let given = |to: &str| -> Given {
            serde_json::from_value(serde_json::json!({"links": [{"target": to, "place": "1"}]}))
                .unwrap()
        };
        std::fs::write(ws.join("Eruptions.csv"), "Krakatoa\n").unwrap();
        let mut core = Core::open(&ws).unwrap();
        let kind = OwnedKind {
            extension: "csv".into(),
            owner: "csv-table".into(),
            version: "1".into(),
        };
        core.set_owners(vec![kind], None);
        let (_, read) = core.take_reads().pop().unwrap();
        core.file_given(
            "csv-table",
            "Eruptions.csv",
            &read.rev,
            Ok(given("Krakatoa")),
        );
        let (source, target) = (
            core.index.by_rel("Krakatoa").unwrap(),
            core.index.by_rel("Krakatau").unwrap(),
        );
        let mut m = merge::plan(&core.index, source, target).unwrap();
        assert_eq!(m.links(), 1, "the CSV's link is counted");
        let bin = crate::trash::Bin::Folder(dir.join("bin"));
        assert!(
            merge::apply(&m, Some(&bin)).is_err(),
            "not before csv-table answered"
        );
        core.relink(&mut m.edits).unwrap().unwrap();
        let (_, ask) = core.take_file_edits().pop().unwrap();
        assert!(matches!(&ask.changes[..], [Change::Link { link, to, .. }]
            if link == "Krakatoa" && to == "Krakatau"));
        core.file_edited(
            "csv-table",
            "Eruptions.csv",
            &ask.rev,
            Ok("Krakatau\n".into()),
        );
        let (_, back) = core.take_reads().pop().unwrap();
        core.file_given(
            "csv-table",
            "Eruptions.csv",
            &back.rev,
            Ok(given("Krakatau")),
        );
        let (_, answers) = core.take_relinked().pop().unwrap();
        rename::answered(&mut m.edits, answers.unwrap());
        merge::apply(&m, Some(&bin)).unwrap();
        core.relinked(&m.edits, Path::to_path_buf);
        core.rescan_now();
        let csv = std::fs::read_to_string(ws.join("Eruptions.csv")).unwrap();
        assert_eq!(csv, "Krakatau\n");
        assert!(!ws.join("Krakatoa.md").exists(), "in the trash");
        assert!(core.take_reads().is_empty(), "what it read back is kept");
        let krakatau = core.index.by_rel("Krakatau").unwrap();
        assert_eq!(core.index.backlinks(krakatau).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_canvas_joins_the_workspace_through_its_built_in_owner() {
        use crate::gateway::{Page, Query, Reason, Select};
        let dir = std::env::temp_dir().join(format!("cold-lava-canvas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Lava.md"), "# Lava\n").unwrap();
        std::fs::write(dir.join("Magma.md"), "# Magma\n").unwrap();
        let canvas = "{\n\t\"nodes\":[\n\t\t{\"id\":\"s\",\"type\":\"text\",\"text\":\"Masks\\n- [ ] Two each, see [[Magma]]\",\"x\":0,\"y\":0,\"width\":1,\"height\":1,\"cold-lava\":{\"shape\":\"sticky\",\"node\":\"text\"}},\n\t\t{\"id\":\"c\",\"type\":\"file\",\"file\":\"Lava.md\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n\t],\n\t\"edges\":[{\"id\":\"e\",\"fromNode\":\"s\",\"toNode\":\"c\"}],\n\t\"mine\": 1\n}\n";
        std::fs::write(dir.join("Board.canvas"), canvas).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let tasks = |core: &Core| {
            let select = Select {
                kind: "task".into(),
                ..Select::default()
            };
            let q = Query {
                select,
                page: Page::default(),
            };
            core.query_items(&q).unwrap().items
        };
        let task = tasks(&core).pop().expect("the sticky's task is a task");
        assert_eq!(task.note, "Board.canvas");
        let from = |core: &Core, rel: &str| {
            let ix = &core.index;
            let backs = ix.backlinks(ix.by_rel(rel).unwrap());
            (backs.iter())
                .map(|b| ix.name(b.from).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            from(&core, "Lava"),
            ["Board.canvas"],
            "a card links to its note"
        );
        assert_eq!(
            from(&core, "Magma"),
            ["Board.canvas"],
            "so does a sticky's link"
        );
        let graph = crate::links::graph(&core.index);
        let edges: Vec<_> = (graph.links.iter())
            .filter(|l| l.from == "Board.canvas")
            .filter_map(|l| l.to.clone())
            .collect();
        assert_eq!(
            edges,
            ["Magma.md", "Lava.md"],
            "an arrow between nodes isn't a link"
        );

        // Ticked: only the sticky's text changes, read back before it is written.
        let tick = |item: &Item| Edit {
            id: item.id.clone(),
            rev: item.rev.clone(),
            op: Op::Toggle,
        };
        let Ok(Answer::Later(ticket)) = core.edit_item("agenda", &tick(&task)) else {
            panic!("an owned file's item is answered later");
        };
        assert!(
            core.take_file_edits().is_empty(),
            "nothing to ask an extension"
        );
        let (by, t, answer) = core.take_edited().pop().expect("answered at once");
        assert_eq!((by.as_str(), t), ("agenda", ticket));
        assert_eq!(answer.unwrap().item.rev, tasks(&core)[0].rev);
        let now = std::fs::read_to_string(dir.join("Board.canvas")).unwrap();
        let (head, tail) = canvas.split_once("- [ ] Two each, see [[Magma]]").unwrap();
        let ticked = now.strip_prefix(head).expect("the bytes before it kept");
        assert!(
            ticked.starts_with("- [x] Two each, see [[Magma]] ✅ "),
            "{now}"
        );
        assert!(now.ends_with(tail), "every byte after it kept: {now}");

        // Changed outside since it was read: refused, and the other program's text stays.
        let task = tasks(&core).pop().unwrap();
        let theirs = now.replace("\"mine\": 1", "\"mine\": 2");
        std::fs::write(dir.join("Board.canvas"), &theirs).unwrap();
        let refused = core.edit_item("agenda", &tick(&task)).unwrap_err();
        assert_eq!(refused.reason, Reason::Changed);
        assert_eq!(read(&dir.join("Board.canvas")), theirs);
        let rev = files::rev(theirs.as_bytes());
        let mut events = Vec::new();
        for _ in 0..100 {
            core.boards.push("Board.canvas".into());
            core.schedule(Duration::ZERO);
            events.extend(core.poll());
            if core.index.workspace.owned[0].rev.as_ref() == Some(&rev) {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(events.contains(&Event::OwnedChanged("Board.canvas".into())));
        let task = tasks(&core).pop().unwrap();
        let again = core.edit_item("agenda", &tick(&task));
        assert!(matches!(again, Ok(Answer::Later(_))), "{:?}", again.err());
        assert!(
            core.take_edited().pop().unwrap().2.is_ok(),
            "read again, it can"
        );
        let now = read(&dir.join("Board.canvas"));
        assert!(
            now.contains("\"mine\": 2") && now.contains("- [ ] Two each"),
            "{now}"
        );

        // Renamed: the card's path and the sticky's link, in place, through the owner, which
        // answers inside `relink`.
        let rename = |core: &mut Core, from: &str, to: &str| {
            let note = core.index.by_rel(from).unwrap();
            let mut plan = crate::rename::plan(&core.index, note, to).unwrap();
            assert!(plan.edits.iter().all(|e| e.owned.is_some()), "its owner's");
            assert_eq!(core.relink(&mut plan.edits), Ok(None), "answered at once");
            assert!(core.take_file_edits().is_empty() && core.take_relinked().is_empty());
            crate::rename::apply(&plan).unwrap();
            core.rescan_now();
        };
        rename(&mut core, "Lava", "Hot/Lava");
        rename(&mut core, "Magma", "Melt");
        assert_eq!(
            read(&dir.join("Board.canvas")),
            now.replace("\"file\":\"Lava.md\"", "\"file\":\"Hot/Lava.md\"")
                .replace("[[Magma]]", "[[Melt]]")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A board's cards are pieces, and nothing a board gave before moves: its items, their ids
    /// and revs, and its links are what its Markdown pieces and file links alone read as. A
    /// type of the user's reads where a card sits, and the owner refuses to write it.
    #[test]
    fn a_boards_cards_are_pieces_and_its_items_dont_move() {
        use crate::gateway::{Page, Query, Reason, Select};
        use crate::owner::given::{GivenItem, GivenLink};
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("cold-lava-cards-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        std::fs::write(
            dir.join(".cold-lava/types.toml"),
            "version = 1\n[types.card]\non = \"text\"\nwhere.attribute = [\"x\"]\n\
             where.file-type = [\"canvas\"]\n\
             fields.x = { attribute = \"x\", type = \"number\" }\n\
             [types.prose]\non = \"text\"\npriority = -1\nfields.words = { rest = true }\n",
        )
        .unwrap();
        std::fs::write(dir.join("Lava.md"), "# Lava\n").unwrap();
        let canvas = "{\"nodes\":[\n\
            {\"id\":\"a\",\"type\":\"text\",\"text\":\"Trip\\n- [ ] Buy masks #trip, see [[Lava]]\",\"x\":40,\"y\":120,\"width\":250,\"height\":60},\n\
            {\"id\":\"b\",\"type\":\"file\",\"file\":\"Lava.md\",\"x\":400,\"y\":0,\"width\":400,\"height\":400},\n\
            {\"id\":\"g\",\"type\":\"group\",\"label\":\"Trip\",\"x\":0,\"y\":0,\"width\":900,\"height\":900},\n\
            {\"id\":\"a\",\"type\":\"text\",\"text\":\"- [ ] Twin\",\"x\":7,\"y\":0,\"width\":1,\"height\":1}\n\
            ],\"edges\":[{\"id\":\"e\",\"fromNode\":\"a\",\"toNode\":\"b\",\"label\":\"needs\"},\
            {\"id\":\"f\",\"fromNode\":\"a\",\"toNode\":\"g\"}]}\n";
        std::fs::write(dir.join("Board.canvas"), canvas).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let settings = core.index.workspace.item_settings();
        let file = &core.index.workspace.owned[0];
        assert_eq!(file.rel, "Board.canvas");

        // What the owner gave before cards were pieces, read alone.
        let markdown = |i: &&GivenItem| matches!(i, GivenItem::Markdown { .. });
        let to_file = |l: &&GivenLink| matches!(l, GivenLink::File { .. });
        let before = Given {
            items: file.given.items.iter().filter(markdown).cloned().collect(),
            links: file.given.links.iter().filter(to_file).cloned().collect(),
        };
        let was = given::read("Board.canvas", &before, settings);
        // The two tasks, and the one paragraph, which a type on every text takes: a card has
        // no words of its own, so that type takes no card, here or below.
        let kinds: Vec<&str> = was.items.iter().map(|i| i.data.kind()).collect();
        assert_eq!(kinds, ["prose", "task", "task"]);
        let apart = |items: &[Item]| -> Vec<serde_json::Value> {
            let not_cards = items.iter().filter(|i| i.data.kind() != "card");
            not_cards
                .map(|i| serde_json::to_value(i).unwrap())
                .collect()
        };
        assert_eq!(apart(&file.items), apart(&was.items), "ids and revs too");
        assert_eq!(file.links, was.links, "and every link, in its order");
        for item in &was.items {
            assert_eq!(file.places.get(&item.id), was.places.get(&item.id));
        }
        let arrows = file.given.links.len() - before.links.len();
        assert_eq!(arrows, 1, "the arrow to the group is not given");

        // The cards a type asks for are items: the two text cards, each where it sits.
        let cards = |core: &Core| {
            let select = Select {
                kind: "card".into(),
                ..Select::default()
            };
            let page = Page::default();
            core.query_items(&Query { select, page }).unwrap().items
        };
        let found = cards(&core);
        let x = |item: &Item| serde_json::to_value(item).unwrap()["x"].as_f64();
        assert_eq!(
            found.iter().map(x).collect::<Vec<_>>(),
            [Some(40.0), Some(7.0)]
        );
        assert!(
            found
                .iter()
                .all(|c| !was.items.iter().any(|i| i.id == c.id))
        );

        // Moving a card is the whiteboard's: the owner refuses, and no byte changes.
        let card = found[0].clone();
        let (field, value) = ("x".to_string(), json!(50));
        let moved = Edit {
            id: card.id.clone(),
            rev: card.rev.clone(),
            op: Op::SetField { field, value },
        };
        let asked = core.edit_item("table", &moved);
        assert!(matches!(asked, Ok(Answer::Later(_))), "{:?}", asked.err());
        let (_, _, answer) = core.take_edited().pop().expect("answered at once");
        let refused = answer.unwrap_err();
        assert_eq!(refused.reason, Reason::Refused, "{refused}");
        assert!(
            refused.message.contains("doesn't write a card's x"),
            "{refused}"
        );
        assert_eq!(read(&dir.join("Board.canvas")), canvas);

        // A task on the card is still ticked through its Markdown, the card's piece beside it.
        let task = file_task(&core);
        let tick = Edit {
            id: task.id.clone(),
            rev: task.rev.clone(),
            op: Op::Toggle,
        };
        assert!(matches!(
            core.edit_item("agenda", &tick),
            Ok(Answer::Later(_))
        ));
        let (_, _, answer) = core.take_edited().pop().expect("answered at once");
        assert!(answer.unwrap().changed);
        let now = read(&dir.join("Board.canvas"));
        assert!(
            now.contains("- [x] Buy masks #trip, see [[Lava]] ✅ "),
            "{now}"
        );
        assert_eq!(cards(&core).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The first task of the first owned file.
    fn file_task(core: &Core) -> Item {
        let items = core.index.workspace.owned[0].items.iter();
        let mut tasks = items.filter(|i| i.data.kind() == "task");
        tasks.next().cloned().expect("a task")
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }
}
