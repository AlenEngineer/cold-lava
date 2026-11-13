//! What a subscriber hears, and in which order (#182). `Harness` opens a `Core` on a temp
//! workspace, plays the file watcher and an extension that owns a kind of file, applies steps
//! (a write from outside, an `items/edit`, a rescan, a rename, a recipe, an owner's answers)
//! and hands out what the core has to say the way a frontend does: `poll`, then
//! `take_item_news`, then `take_edited`, a `Batch` each time. Nothing here waits or sleeps:
//! the watcher's events are sent by the test, and the settle delay is skipped.
//!
//! What the tests below pin, and frontends rely on:
//!
//! - News is queued inside the call that read the change, in the order the changes were read,
//!   a call's item news before its link news.
//! - A change is news once. A file read again with the text the core already has is no news:
//!   a watcher's event for the core's own write, or two events for one file.
//! - The queue never holds item news behind an `items/reset` of the same subscription
//!   (`Harness::call` checks it after every call every test makes): while a reset waits to be
//!   taken, the core drops that subscription's later item news, which the subscriber's query on
//!   the reset finds anyway. So no caller has to take the news between two calls to keep a
//!   change from being counted twice. `rescan_now` still leaves its reset in the queue without
//!   an event for `poll` and without `has_unseen`, so whoever calls it takes the news at once,
//!   for its subscribers to be up to date.
//! - Not so yet: an item that moves without its own lines changing (a line added above it)
//!   is no news, since its `rev` is the same. A subscriber that applies `items/changed` keeps
//!   its old `line`; one that queries again hears nothing to query on. The tests here leave
//!   every item they keep where it is.
//! - News carries no order. The order is `items/query`'s: notes by path, then the files other
//!   owners read by path, each file's items as the file has them. A subscriber that shows rows
//!   queries again on any news of its subscription, and then has them in that order.
//! - An edit of a file another owner reads is news when the owner has answered and the file is
//!   written, with the answer in the same batch.
//! - A file another owner reads is changed one way, whoever owns it (#183): its owner gives the
//!   new text, reads it back, and only then is it written, over the text the change began from.
//!   The engine's canvas owner answers inside the call and an extension later, so a canvas's
//!   edit is done when `edit_item` returns and a rename of canvases alone when `relink` does;
//!   one owner's refusal, or a file changed meanwhile, writes nothing anywhere.

use super::*;
use crate::gateway::{Filter, Op, Page};
use crate::owner::given::GivenItem;
use crate::{recipe, rename};

/// The extension the harness plays: it owns `.lines` files, each line a Markdown piece at its
/// line number.
const OWNER: &str = "lines";

/// What the core handed out at one delivery, each queue in its order.
#[derive(Clone, Debug)]
pub(crate) struct Batch {
    /// From `poll`.
    pub events: Vec<Event>,
    /// What a `Core::subscribe` channel carried meanwhile.
    pub channel: Vec<Event>,
    /// From `take_item_news`: each piece with the client it is for.
    pub news: Vec<(String, News)>,
    /// From `take_edited`: the answers to edits answered `Later`.
    pub edited: Vec<(String, Ticket, Result<Edited, gateway::Error>)>,
}

impl Batch {
    fn of<'a>(&'a self, client: &'a str) -> impl Iterator<Item = &'a News> {
        let mine = move |(to, _): &&(String, News)| to == client;
        self.news.iter().filter(mine).map(|(_, n)| n)
    }

    /// The methods of the news `client` hears, in order.
    pub(crate) fn names(&self, client: &str) -> Vec<&'static str> {
        self.of(client)
            .map(|n| match n {
                News::Changed { .. } => "items/changed",
                News::Reset { .. } => "items/reset",
                News::LinksChanged { .. } => "links/changed",
            })
            .collect()
    }

    /// How many items each `items/changed` for `client` adds, updates and removes.
    pub(crate) fn counts(&self, client: &str) -> Vec<(usize, usize, usize)> {
        self.of(client)
            .filter_map(|n| match n {
                News::Changed {
                    added,
                    updated,
                    removed,
                    ..
                } => Some((added.len(), updated.len(), removed.len())),
                _ => None,
            })
            .collect()
    }

    /// The files each `links/changed` for `client` says came and went.
    pub(crate) fn links(&self, client: &str) -> Vec<(Vec<String>, Vec<String>)> {
        self.of(client)
            .filter_map(|n| match n {
                News::LinksChanged {
                    created, removed, ..
                } => Some((created.clone(), removed.clone())),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.channel.is_empty()
            && self.news.is_empty()
            && self.edited.is_empty()
    }
}

/// A subscriber that keeps its items both ways a subscriber can.
struct Follower {
    client: String,
    select: Select,
    sub: SubId,
    /// As an extension that applies the news does: queried when it subscribes and on
    /// `items/reset`, with `items/changed` applied to what it has. An item it is told of twice
    /// is there twice. In no order, since news carries none.
    items: Vec<Item>,
    /// As `Feed` in the binary and a bound shape do: queried again on any news.
    rows: Vec<Item>,
}

impl Follower {
    fn hear(&mut self, core: &Core, news: &News) {
        if !matches!(news, News::LinksChanged { .. }) {
            self.rows = all(core, &self.select);
        }
        match news {
            News::Reset { .. } => self.items = all(core, &self.select),
            News::Changed {
                added,
                updated,
                removed,
                ..
            } => {
                self.items.retain(|i| !removed.contains(&i.id));
                for item in &mut self.items {
                    if let Some(new) = updated.iter().find(|u| u.id == item.id) {
                        *item = new.clone();
                    }
                }
                self.items.extend(added.iter().cloned());
            }
            News::LinksChanged { .. } => {}
        }
    }
}

pub(crate) struct Harness {
    core: Core,
    dir: PathBuf,
    watcher: Sender<notify::Event>,
    channel: Receiver<Event>,
    followers: Vec<Follower>,
    log: Vec<Batch>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Harness {
    /// On a new workspace with these files.
    pub(crate) fn with(name: &str, files: &[(&str, &str)]) -> Harness {
        let dir = temp(name);
        for (rel, text) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Harness::open(dir)
    }

    /// On a copy of the workspace at `from`.
    pub(crate) fn copy_of(name: &str, from: &Path) -> Harness {
        let dir = temp(name);
        for entry in walkdir::WalkDir::new(from).into_iter().map(Result::unwrap) {
            let to = dir.join(entry.path().strip_prefix(from).unwrap());
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&to).unwrap();
            } else {
                std::fs::copy(entry.path(), &to).unwrap();
            }
        }
        Harness::open(dir)
    }

    fn open(dir: PathBuf) -> Harness {
        let mut core = Core::open(&dir).unwrap();
        // The harness is the watcher from here on: the core hears of a file only when a test
        // says so (`saw`), through the code that reads the real watcher's events.
        let (watcher, fs_events) = mpsc::channel();
        core._watcher = None;
        core.fs_events = Some(fs_events);
        let channel = core.subscribe();
        core.set_owners(vec![lines_kind()], None);
        let mut harness = Harness {
            core,
            dir,
            watcher,
            channel,
            followers: Vec::new(),
            log: Vec::new(),
        };
        harness.owner_reads();
        harness.take();
        harness
    }

    /// One call into the core. It must leave no item news queued behind a reset of the same
    /// subscription: a subscriber queries again on the reset, and would count that news twice.
    fn call<T>(&mut self, f: impl FnOnce(&mut Core) -> T) -> T {
        let out = f(&mut self.core);
        let queued = &self.core.news;
        for (i, (_, news)) in queued.iter().enumerate() {
            let behind = |(_, later): &(String, News)| later.subscription() == news.subscription();
            let reset = matches!(news, News::Reset { .. });
            assert!(
                !(reset && queued[i + 1..].iter().any(behind)),
                "news is queued behind a reset: {queued:?}"
            );
        }
        out
    }

    /// `client` follows the items `select` names.
    pub(crate) fn follow(&mut self, client: &str, select: Select) {
        let sub = self.core.subscribe_items(client, select.clone()).unwrap();
        let items = all(&self.core, &select);
        self.followers.push(Follower {
            client: client.into(),
            select,
            sub,
            rows: items.clone(),
            items,
        });
    }

    /// `client` follows the link graph.
    pub(crate) fn follow_links(&mut self, client: &str) {
        self.core.subscribe_links(client);
    }

    /// Another program writes a file. The core hears of it with `saw`.
    pub(crate) fn write(&self, rel: &str, text: &str) {
        std::fs::write(self.core.root().join(rel), text).unwrap();
    }

    /// Another program deletes a file.
    pub(crate) fn remove(&self, rel: &str) {
        std::fs::remove_file(self.core.root().join(rel)).unwrap();
    }

    pub(crate) fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.core.root().join(rel)).unwrap()
    }

    /// The watcher reports the file at `rel`.
    pub(crate) fn saw(&self, rel: &str) {
        let path = self.core.root().join(rel);
        let event = notify::Event::new(notify::EventKind::Any).add_path(path);
        self.watcher.send(event).unwrap();
    }

    /// `items/edit` by `by`.
    pub(crate) fn edit(&mut self, by: &str, edit: &Edit) -> Result<Answer, gateway::Error> {
        self.call(|core| core.edit_item(by, edit))
    }

    pub(crate) fn rescan(&mut self) {
        self.call(Core::rescan_now);
    }

    /// The version of the file or folder on disk now, as a dialog reads it before it asks
    /// (`workspace/stat`).
    pub(crate) fn stat(&self, rel: &str) -> String {
        self.core.stat_file(rel).unwrap().rev
    }

    /// `workspace/delete` by `by`, of the version `expect`. The harness's core has no system
    /// trash, so what goes is in the temp workspace's own `.trash`.
    pub(crate) fn delete(
        &mut self,
        by: &str,
        rel: &str,
        expect: &str,
    ) -> Result<files::Deleted, gateway::Error> {
        self.call(|core| core.delete_file(by, rel, expect))
    }

    /// `workspace/restore` by `by`, to where it was.
    pub(crate) fn restore(
        &mut self,
        by: &str,
        trashed: &files::TrashId,
    ) -> Result<files::Restored, gateway::Error> {
        let trashed = trashed.clone();
        let restore = files::Restore { trashed, to: None };
        self.call(|core| core.restore_file(by, &restore))
    }

    /// The menu tells the core who owns which kind of file again, as when extensions restart.
    pub(crate) fn set_owners(&mut self) {
        self.call(|core| core.set_owners(vec![lines_kind()], None));
    }

    /// A rename as the menu makes it, when no extension has to answer: planned, the files other
    /// owners read given their new texts (`relink`), applied, then everything read again.
    pub(crate) fn rename(&mut self, from: &str, to: &str) {
        let mut plan = self.plan_rename(from, to);
        let waiting = self.relink(&mut plan.edits);
        assert_eq!(waiting, None, "an extension has to answer: `relinked`");
        self.renamed(&plan);
    }

    pub(crate) fn plan_rename(&self, from: &str, to: &str) -> rename::Plan {
        let note = self.core.index.by_rel(from).unwrap();
        rename::plan(&self.core.index, note, to).unwrap()
    }

    /// Before a rename, a merge or a recipe is made: its owned files' owners are asked for
    /// their new texts (`Core::relink`). The ticket to wait under, when an extension has to
    /// answer.
    pub(crate) fn relink(&mut self, edits: &mut [rename::FileEdit]) -> Option<Ticket> {
        self.call(|core| core.relink(edits)).unwrap()
    }

    /// The renames whose owners have all answered, or one refused (`Core::take_relinked`).
    pub(crate) fn relinked(&mut self) -> Vec<(Ticket, Result<Vec<rename::FileEdit>, String>)> {
        self.core.take_relinked()
    }

    /// The rename is made, once its owners have answered, and everything is read again.
    pub(crate) fn renamed(&mut self, plan: &rename::Plan) {
        rename::apply(plan).unwrap();
        self.call(|core| core.relinked(&plan.edits, |path| plan.moved(path)));
        self.rescan();
    }

    /// A recipe as its screen runs it, when no extension has to answer: planned, the files
    /// other owners read given their new texts, applied, then everything read again.
    pub(crate) fn recipe(&mut self, id: &str, options: &[(&str, &str)]) -> recipe::Applied {
        let recipe = recipe::find(id).unwrap();
        let mut plan = recipe::plan(&self.core.index, recipe, named(options)).unwrap();
        let waiting = self.relink(&mut plan.edits);
        assert_eq!(waiting, None, "an extension has to answer");
        let applied = recipe::apply(&plan).unwrap();
        self.rescan();
        applied
    }

    pub(crate) fn undo(&mut self, applied: &recipe::Applied) {
        recipe::undo(applied).unwrap();
        self.rescan();
    }

    /// A recipe as a shape's step runs it (`Core::run_recipe`).
    pub(crate) fn run_recipe(&mut self, id: &str, options: &[(&str, &str)]) {
        self.try_recipe(id, options).unwrap();
    }

    /// The same, with how it went.
    pub(crate) fn try_recipe(
        &mut self,
        id: &str,
        options: &[(&str, &str)],
    ) -> Result<String, gateway::Error> {
        self.call(|core| core.run_recipe(id, named(options)))
    }

    /// The owner answers the `file/edit` requests waiting for it. How many there were.
    pub(crate) fn owner_edits(&mut self) -> usize {
        let asks = self.core.take_file_edits();
        for (owner, ask) in &asks {
            let text = lines_edit(ask);
            self.call(|core| core.file_edited(owner, &ask.path, &ask.rev, Ok(text)));
        }
        asks.len()
    }

    /// The owner refuses the `file/edit` requests waiting for it.
    pub(crate) fn owner_refuses(&mut self, why: &str) {
        for (owner, ask) in self.core.take_file_edits() {
            let refusal = Err(why.to_string());
            self.call(|core| core.file_edited(&owner, &ask.path, &ask.rev, refusal));
        }
    }

    /// The owner answers the `file/read` requests waiting for it. How many there were.
    pub(crate) fn owner_reads(&mut self) -> usize {
        let reads = self.core.take_reads();
        for (owner, read) in &reads {
            let given = lines_given(&read.text);
            self.call(|core| core.file_given(owner, &read.path, &read.rev, Ok(given)));
        }
        reads.len()
    }

    /// Everything the core has to say now. The watcher's events are read without waiting for
    /// them to settle: the wait is moved to now.
    fn take(&mut self) -> Batch {
        let mut events = self.call(Core::poll);
        if self.core.due.is_some() {
            self.core.due = Some(Instant::now());
            events.extend(self.call(Core::poll));
        }
        Batch {
            events,
            channel: self.channel.try_iter().collect(),
            news: self.core.take_item_news(),
            edited: self.core.take_edited(),
        }
    }

    /// Hand out what the core has to say. Every follower then has what a fresh query finds,
    /// and its rows in the query's order.
    pub(crate) fn deliver(&mut self) -> Batch {
        let batch = self.take();
        for (to, news) in &batch.news {
            let sub = news.subscription();
            let mut followers = self.followers.iter_mut();
            if let Some(f) = followers.find(|f| f.client == *to && f.sub == sub) {
                f.hear(&self.core, news);
            }
        }
        self.log.push(batch.clone());
        for f in &self.followers {
            let fresh = all(&self.core, &f.select);
            assert_eq!(f.rows, fresh, "the rows {} shows", f.client);
            let items = sorted(f.items.clone());
            assert_eq!(items, sorted(fresh), "what {} has", f.client);
        }
        batch
    }

    /// What `client` heard since it subscribed: the methods of its news, a list per delivery.
    pub(crate) fn heard(&self, client: &str) -> Vec<Vec<&'static str>> {
        self.log.iter().map(|b| b.names(client)).collect()
    }

    /// The items `client` has, by file and line.
    pub(crate) fn items(&self, client: &str) -> Vec<Item> {
        let mut followers = self.followers.iter();
        let f = followers.find(|f| f.client == client).unwrap();
        sorted(f.items.clone())
    }

    /// Which of `words` each of `client`'s rows says, in the order it shows them.
    pub(crate) fn rows<'a>(&self, client: &str, words: &[&'a str]) -> Vec<&'a str> {
        let mut followers = self.followers.iter();
        let f = followers.find(|f| f.client == client).unwrap();
        let said = |i: &Item| words.iter().copied().find(|w| text(i).contains(w));
        f.rows.iter().map(|i| said(i).unwrap_or("?")).collect()
    }

    /// How many of `client`'s items say `words`.
    pub(crate) fn count(&self, client: &str, words: &str) -> usize {
        let items = self.items(client);
        items.iter().filter(|i| text(i).contains(words)).count()
    }

    /// The task that says `words`, as a query finds it now.
    pub(crate) fn task(&self, words: &str) -> Item {
        let filter = Filter {
            text: Some(words.into()),
            ..Filter::default()
        };
        let kind = "task".into();
        let found = all(&self.core, &Select { kind, filter });
        found.into_iter().next().unwrap()
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cold-lava-news-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
    dir
}

fn lines_kind() -> OwnedKind {
    OwnedKind {
        extension: "lines".into(),
        owner: OWNER.into(),
        version: "1".into(),
    }
}

fn all(core: &Core, select: &Select) -> Vec<Item> {
    let query = Query {
        select: select.clone(),
        page: Page::default(),
    };
    core.query_items(&query).unwrap().items
}

fn sorted(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by_key(|i| (i.note.clone(), i.line, i.id.clone()));
    items
}

fn text(item: &Item) -> String {
    let json = serde_json::to_value(item).unwrap();
    json["text"].as_str().unwrap_or_default().to_string()
}

fn named(options: &[(&str, &str)]) -> recipe::Options {
    let pair = |(k, v): &(&str, &str)| (k.to_string(), v.to_string());
    options.iter().map(pair).collect()
}

fn lines_given(text: &str) -> Given {
    let piece = |(i, line): (usize, &str)| GivenItem::Markdown {
        place: (i + 1).to_string().into(),
        text: line.into(),
    };
    Given {
        items: text.lines().enumerate().map(piece).collect(),
        ..Given::default()
    }
}

fn lines_edit(ask: &FileEdit) -> String {
    let mut lines: Vec<&str> = ask.text.lines().collect();
    for change in &ask.changes {
        let Change::Text { place, text } = change else {
            panic!("a .lines file has only Markdown pieces: {change:?}");
        };
        lines[place.parse::<usize>().unwrap() - 1] = text.as_str();
    }
    lines.join("\n") + "\n"
}

const PLAN: &str = "# Plan\nSee [[Ash]].\n\n\
                    - [ ] Pay rent 📅 2026-11-07\n\
                    - [ ] Water plants 📅 2026-11-10\n";
const ASH: &str = "# Ash\n- [ ] Sweep the ash off the roof #ash\n";
const READING: &str = "- [ ] Read the report 📅 2026-11-20\n";
const BOARD: &str = "{\n\t\"nodes\":[\n\t\t{\"id\":\"s\",\"type\":\"text\",\"text\":\"- [ ] Check the seismograph\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n\t],\n\t\"edges\":[]\n}\n";
/// `BOARD` with a card before its own, whose task is on that card's second line.
const BOARD_OF_TWO: &str = "{\n\t\"nodes\":[\n\t\t{\"id\":\"t\",\"type\":\"text\",\"text\":\"Bolts\\n- [ ] Tighten the bolts\",\"x\":0,\"y\":0,\"width\":1,\"height\":1},\n\t\t{\"id\":\"s\",\"type\":\"text\",\"text\":\"- [ ] Check the seismograph\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n\t],\n\t\"edges\":[]\n}\n";
/// A canvas as another program wrote it, with its own spacing and fields cold-lava doesn't
/// know: a card of Ash by its path, and a sticky that links to Ash and has a tagged task.
const LINKED_BOARD: &str = "{\n  \"nodes\": [\n    {\"id\": \"c\", \"type\": \"file\", \"file\": \"Ash.md\", \"x\": 0, \"y\": 0, \"width\": 1, \"height\": 1, \"theirs\": {\"file\": \"Ash.md\"}},\n    {\"id\":\"s\",\"type\":\"text\",\"text\":\"See [[Ash]]\\n- [ ] Check the seismograph #ash\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n  ],\n  \"edges\": [],\n  \"plugin\": [1, 2]\n}\n";
const STICKY_TASK: &str = "- [ ] Check the seismograph #ash";
const LINKED_READING: &str = "- [ ] Read the report on [[Ash]]\n";

/// `LINKED_BOARD` after Ash was renamed to Tephra: the card's own path and the sticky's link,
/// and no other byte.
fn relinked_board() -> String {
    let card = (
        "\"file\": \"Ash.md\", \"x\"",
        "\"file\": \"Tephra.md\", \"x\"",
    );
    let board = LINKED_BOARD.replace(card.0, card.1);
    board.replace("[[Ash]]", "[[Tephra]]")
}

/// Two notes, a canvas and a `.lines` file, the last three with a link to Ash.
fn linked(name: &str) -> Harness {
    let files = [
        ("Plan.md", PLAN),
        ("Ash.md", ASH),
        ("Board.canvas", LINKED_BOARD),
        ("Reading.lines", LINKED_READING),
    ];
    Harness::with(name, &files)
}

/// Two notes, a canvas (the engine's own owner) and a `.lines` file (the harness's), with five
/// tasks between them.
fn workspace(name: &str) -> Harness {
    let files = [
        ("Plan.md", PLAN),
        ("Ash.md", ASH),
        ("Board.canvas", BOARD),
        ("Reading.lines", READING),
    ];
    Harness::with(name, &files)
}

fn tasks() -> Select {
    Select {
        kind: "task".into(),
        ..Select::default()
    }
}

fn set_due(item: &Item, date: &str) -> Edit {
    let field = "due".into();
    let value = date.into();
    Edit {
        id: item.id.clone(),
        rev: item.rev.clone(),
        op: Op::SetField { field, value },
    }
}

fn tick(item: &Item) -> Edit {
    Edit {
        id: item.id.clone(),
        rev: item.rev.clone(),
        op: Op::Toggle,
    }
}

fn rels(rels: &[&str]) -> Vec<String> {
    rels.iter().map(|r| r.to_string()).collect()
}

fn changed(rel: &str) -> Event {
    Event::NoteChanged(rel.into())
}

/// An `ItemEdited`'s file, who made the edit, and what it did.
fn edited(event: &Event) -> Option<(&str, &str, &str)> {
    let Event::ItemEdited { note, by, what, .. } = event else {
        return None;
    };
    Some((note.as_str(), by.as_str(), what.as_str()))
}

#[test]
fn the_harness_reads_notes_a_canvas_and_an_owned_file() {
    let mut h = workspace("fixture");
    h.follow("agenda", tasks());
    let notes: Vec<String> = h.items("agenda").into_iter().map(|i| i.note).collect();
    let want = [
        "Ash.md",
        "Board.canvas",
        "Plan.md",
        "Plan.md",
        "Reading.lines",
    ];
    assert_eq!(notes, want);
    assert!(h.deliver().is_empty(), "opening left nothing to say");
}

#[test]
fn a_rescan_then_a_change_is_a_reset_then_the_change() {
    let mut h = workspace("reset");
    h.follow("agenda", tasks());
    h.rescan();
    let reset = h.deliver();
    assert_eq!(reset.names("agenda"), ["items/reset"]);
    assert_eq!(reset.events, [], "poll has nothing to say of it");
    assert_eq!(reset.channel, [Event::Rescanned]);

    h.write("Chores.md", "- [ ] Sweep the roof\n");
    h.saw("Chores.md");
    let change = h.deliver();
    assert_eq!(change.events, [Event::NoteCreated("Chores".into())]);
    assert_eq!(change.names("agenda"), ["items/changed"]);
    assert_eq!(change.counts("agenda"), [(1, 0, 0)]);
    assert_eq!(h.count("agenda", "Sweep the roof"), 1);
}

/// #172 at engine level. News taken only after the next change holds the reset alone: the
/// subscriber's query on the reset already finds the new task, so the core doesn't tell of it
/// again. What is read after the reset was taken is news as ever.
#[test]
fn a_reset_left_in_the_queue_covers_the_next_change() {
    let mut h = workspace("merged");
    h.follow("agenda", tasks());
    h.rescan();
    h.write("Chores.md", "- [ ] Sweep the roof\n");
    h.saw("Chores.md");
    let batch = h.deliver();
    assert_eq!(batch.events, [Event::NoteCreated("Chores".into())]);
    assert_eq!(batch.names("agenda"), ["items/reset"]);
    assert_eq!(h.count("agenda", "Sweep the roof"), 1);

    h.write(
        "Chores.md",
        "- [ ] Sweep the roof\n- [ ] Clear the gutters\n",
    );
    h.saw("Chores.md");
    assert_eq!(h.deliver().counts("agenda"), [(1, 0, 0)]);
    assert_eq!(h.count("agenda", "Clear the gutters"), 1);
}

/// A note that sorts after the canvas by name comes before it, and a canvas's cards stay in
/// the file's order, whatever line of its card a task is on.
#[test]
fn rows_are_in_the_query_s_order_after_a_change_and_after_a_reset() {
    let mut h = workspace("order");
    h.follow("table", tasks());
    h.write("Zone.md", "- [ ] Map the zone\n");
    h.saw("Zone.md");
    h.write("Board.canvas", BOARD_OF_TWO);
    h.saw("Board.canvas");
    let batch = h.deliver();
    assert_eq!(batch.names("table"), ["items/changed", "items/changed"]);
    let want = [
        "Sweep the ash",
        "Pay rent",
        "Water plants",
        "Map the zone",
        "Tighten the bolts",
        "Check the seismograph",
        "Read the report",
    ];
    assert_eq!(h.rows("table", &want), want, "after items/changed");

    h.rescan();
    assert_eq!(h.deliver().names("table"), ["items/reset"]);
    assert_eq!(h.rows("table", &want), want, "after items/reset");
}

/// #199. `set_owners` queues a reset when the owned files changed (here the `.lines` file is
/// dropped until its owner reads it again), and its events wait for the next `poll`, which
/// also reads the new note.
#[test]
fn a_reset_from_set_owners_and_the_next_change_are_counted_once() {
    let mut h = workspace("owners");
    h.follow("agenda", tasks());
    h.set_owners();
    h.write("Chores.md", "- [ ] Sweep the roof\n");
    h.saw("Chores.md");
    let batch = h.deliver();
    assert_eq!(batch.names("agenda"), ["items/reset"]);
    assert_eq!(h.count("agenda", "Sweep the roof"), 1);
    assert_eq!(h.count("agenda", "Read the report"), 0, "not read yet");

    h.owner_reads();
    assert_eq!(h.deliver().counts("agenda"), [(1, 0, 0)]);
    assert_eq!(h.count("agenda", "Read the report"), 1);
}

#[test]
fn an_edit_is_heard_once_by_everyone_and_names_who_made_it() {
    let mut h = workspace("edit");
    h.follow("agenda", tasks());
    h.follow("table", tasks());
    let headings = Select {
        kind: "heading".into(),
        ..Select::default()
    };
    h.follow("outline", headings);
    let rent = h.task("Pay rent");
    let answer = h.edit("agenda", &set_due(&rent, "2026-12-01"));
    assert!(matches!(answer, Ok(Answer::Now(e)) if e.changed));
    let batch = h.deliver();
    assert_eq!(batch.events.len(), 2, "{batch:?}");
    assert_eq!(batch.events[0], changed("Plan"));
    let by = Some(("Plan", "agenda", "set due"));
    assert_eq!(edited(&batch.events[1]), by);
    assert_eq!(batch.channel, batch.events);
    for client in ["agenda", "table"] {
        assert_eq!(batch.names(client), ["items/changed"]);
        assert_eq!(batch.counts(client), [(0, 1, 0)]);
    }
    assert!(batch.names("outline").is_empty(), "no heading changed");

    // The watcher reports the core's own write after the core has read it.
    h.saw("Plan.md");
    let late = h.deliver();
    assert!(late.is_empty(), "{late:?}");

    // The note says so already: nothing is written, and nobody hears anything.
    let rent = h.task("Pay rent");
    let same = h.edit("table", &set_due(&rent, "2026-12-01"));
    assert!(matches!(same, Ok(Answer::Now(e)) if !e.changed));
    assert!(h.deliver().is_empty());
}

#[test]
fn writes_the_watcher_reports_twice_or_out_of_order_are_news_once() {
    let mut h = workspace("twice");
    h.follow("agenda", tasks());
    h.follow_links("graph");

    // Two writes to one file before the core polls.
    let plan = h.read("Plan.md");
    h.write("Plan.md", &format!("{plan}- [ ] Call the plumber\n"));
    h.saw("Plan.md");
    h.write(
        "Plan.md",
        &format!("{plan}- [ ] Call the plumber\n- [ ] Buy masks\n"),
    );
    h.saw("Plan.md");
    let batch = h.deliver();
    assert_eq!(batch.events, [changed("Plan")]);
    assert_eq!(batch.counts("agenda"), [(2, 0, 0)]);
    assert_eq!(batch.links("graph"), [(rels(&[]), rels(&[]))]);

    // Two files, reported in the other order than they were written in.
    h.write("Chores.md", "- [ ] Sweep the roof\n");
    h.write("Drills.md", "- [ ] Run the drill\n");
    h.saw("Drills.md");
    h.saw("Chores.md");
    let batch = h.deliver();
    let created = |rel: &str| Event::NoteCreated(rel.into());
    assert_eq!(batch.events, [created("Drills"), created("Chores")]);
    assert_eq!(batch.counts("agenda"), [(2, 0, 0)]);
    let both = rels(&["Drills.md", "Chores.md"]);
    assert_eq!(batch.links("graph"), [(both, rels(&[]))]);
    assert_eq!(h.count("agenda", "Sweep the roof"), 1);
    assert_eq!(h.count("agenda", "Run the drill"), 1);
}

/// The core is behind the disk when the edit comes: another program has written the note, and
/// the watcher's event for that is still on its way.
#[test]
fn an_edit_of_a_note_changed_outside_tells_of_both_changes_once() {
    let mut h = workspace("behind");
    h.follow("agenda", tasks());
    let rent = h.task("Pay rent");
    let plan = h.read("Plan.md");
    h.write("Plan.md", &format!("{plan}- [ ] Call the plumber\n"));
    let answer = h.edit("table", &set_due(&rent, "2026-12-01"));
    if let Err(refused) = &answer {
        assert_eq!(refused.reason, Reason::Changed, "{refused}");
    }
    let first = h.deliver();
    h.saw("Plan.md");
    let late = h.deliver();
    let added = |b: &Batch| b.counts("agenda").iter().map(|c| c.0).sum::<usize>();
    assert_eq!(added(&first) + added(&late), 1, "{first:?} {late:?}");
    assert_eq!(h.count("agenda", "Call the plumber"), 1);
    let due = h.read("Plan.md").contains("Pay rent 📅 2026-12-01");
    assert_eq!(due, answer.is_ok());
}

#[test]
fn an_owned_file_s_edit_is_news_after_its_owner_answered() {
    let mut h = workspace("later");
    h.follow("agenda", tasks());
    h.follow("table", tasks());
    let report = h.task("Read the report");
    let Ok(Answer::Later(ticket)) = h.edit("agenda", &tick(&report)) else {
        panic!("an owned file's item is answered later");
    };
    assert!(h.deliver().is_empty(), "nothing before the owner answers");
    h.owner_edits();
    assert!(h.deliver().is_empty(), "nor before it read its new text");
    assert_eq!(h.read("Reading.lines"), READING, "nor is it written");
    h.owner_reads();
    let batch = h.deliver();
    assert_eq!(batch.events.len(), 2, "{batch:?}");
    assert_eq!(batch.events[0], Event::OwnedChanged("Reading.lines".into()));
    let by = Some(("Reading.lines", "agenda", "ticked"));
    assert_eq!(edited(&batch.events[1]), by);
    for client in ["agenda", "table"] {
        assert_eq!(batch.names(client), ["items/changed"]);
    }
    let [(to, t, Ok(answer))] = &batch.edited[..] else {
        panic!("{batch:?}");
    };
    assert_eq!((to.as_str(), *t, answer.changed), ("agenda", ticket, true));
    assert!(h.read("Reading.lines").starts_with("- [x] Read the report"));

    // The watcher reports the core's own write afterwards.
    h.saw("Reading.lines");
    let late = h.deliver();
    assert_eq!(late.events, [Event::FileChanged("Reading.lines".into())]);
    assert!(late.news.is_empty() && late.edited.is_empty(), "{late:?}");

    // Written by another program: its items are news once its owner has read it.
    let text = h.read("Reading.lines");
    h.write("Reading.lines", &format!("{text}- [ ] Return the book\n"));
    h.saw("Reading.lines");
    let seen = h.deliver();
    assert_eq!(seen.events, [Event::FileChanged("Reading.lines".into())]);
    assert!(seen.news.is_empty(), "{seen:?}");
    h.owner_reads();
    let read = h.deliver();
    assert_eq!(read.events, [Event::OwnedChanged("Reading.lines".into())]);
    assert_eq!(read.names("agenda"), ["items/changed"]);
    assert_eq!(h.count("agenda", "Return the book"), 1);
}

/// The engine's own owner answers inside the call, so the edit is still answered `Later`, and
/// the answer, the events and the news are all in the next batch.
#[test]
fn a_canvas_s_edit_is_answered_and_told_in_one_batch() {
    let mut h = workspace("canvas");
    h.follow("agenda", tasks());
    let check = h.task("Check the seismograph");
    let Ok(Answer::Later(ticket)) = h.edit("agenda", &tick(&check)) else {
        panic!("an owned file's item is answered later");
    };
    let batch = h.deliver();
    assert_eq!(batch.events.len(), 2, "{batch:?}");
    assert_eq!(batch.events[0], Event::OwnedChanged("Board.canvas".into()));
    let by = Some(("Board.canvas", "agenda", "ticked"));
    assert_eq!(edited(&batch.events[1]), by);
    assert_eq!(batch.names("agenda"), ["items/changed"]);
    let [(to, t, Ok(answer))] = &batch.edited[..] else {
        panic!("{batch:?}");
    };
    assert_eq!((to.as_str(), *t, answer.changed), ("agenda", ticket, true));

    h.saw("Board.canvas");
    let late = h.deliver();
    assert_eq!(late.events, [Event::BoardChanged("Board.canvas".into())]);
    assert!(late.news.is_empty(), "{late:?}");
}

/// A tick on a canvas's card is written once, into that card's text, with every other byte of
/// the file as it was, and heard once. A canvas another program wrote since it was read is left
/// as that program wrote it.
#[test]
fn a_canvas_s_edit_changes_its_card_in_place_or_nothing() {
    let files = [("Ash.md", ASH), ("Board.canvas", LINKED_BOARD)];
    let mut h = Harness::with("in-place", &files);
    h.follow("agenda", tasks());
    h.follow("table", tasks());
    let check = h.task("Check the seismograph");
    let answer = h.edit("agenda", &tick(&check));
    assert!(matches!(answer, Ok(Answer::Later(_))));
    let batch = h.deliver();
    let by = Some(("Board.canvas", "agenda", "ticked"));
    assert_eq!(batch.events.last().and_then(edited), by);
    for client in ["agenda", "table"] {
        assert_eq!(batch.counts(client), [(0, 1, 0)]);
    }
    assert!(matches!(&batch.edited[..], [(_, _, Ok(e))] if e.changed));
    let ticked = h.read("Board.canvas");
    let (head, tail) = LINKED_BOARD.split_once(STICKY_TASK).unwrap();
    let card = ticked.strip_prefix(head).and_then(|t| t.strip_suffix(tail));
    let done = "- [x] Check the seismograph #ash";
    assert!(card.is_some_and(|c| c.starts_with(done)), "{ticked}");

    // Another program writes the canvas, and the core hasn't read that yet.
    let theirs = ticked.replace("\"plugin\": [1, 2]", "\"plugin\": [1, 2, 3]");
    h.write("Board.canvas", &theirs);
    let check = h.task("Check the seismograph");
    let refused = h.edit("table", &tick(&check)).unwrap_err();
    assert_eq!(refused.reason, Reason::Changed, "{refused}");
    assert_eq!(h.read("Board.canvas"), theirs);
    assert!(
        h.deliver().is_empty(),
        "an edit that wasn't made is no news"
    );

    // Read again, it can be edited, and what the other program wrote stays.
    h.saw("Board.canvas");
    h.deliver();
    let check = h.task("Check the seismograph");
    let answer = h.edit("table", &tick(&check));
    assert!(matches!(answer, Ok(Answer::Later(_))));
    let batch = h.deliver();
    assert!(matches!(&batch.edited[..], [(to, _, Ok(e))] if to == "table" && e.changed));
    let now = h.read("Board.canvas");
    assert!(now.contains("\"plugin\": [1, 2, 3]") && now.contains("- [ ] Check the seismograph"));
    assert!(now.contains("\"theirs\": {\"file\": \"Ash.md\"}"), "{now}");
}

/// #86 with both kinds of owner. A rename's plan lists the canvas and the extension's file with
/// the notes. Nothing moves until both owners have given their new texts, the engine's own
/// inside the call and the extension later, each read back; then every file is written.
#[test]
fn a_rename_relinks_a_canvas_and_an_extension_s_file_in_one_plan() {
    let mut h = linked("relink");
    h.follow("agenda", tasks());
    let mut plan = h.plan_rename("Ash", "Tephra");
    let files: Vec<&str> = plan.edits.iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, ["Plan.md", "Board.canvas", "Reading.lines"]);
    assert!(
        rename::apply(&plan).is_err(),
        "not before the owners answered"
    );

    let ticket = h
        .relink(&mut plan.edits)
        .expect("the extension answers later");
    let owned = plan.edits.iter().filter_map(|e| e.owned.as_ref());
    let waits: Vec<&str> = (owned.filter(|o| o.after.is_none()))
        .map(|o| o.owner.as_str())
        .collect();
    assert_eq!(waits, [OWNER], "the canvas's owner has answered");
    assert!(h.relinked().is_empty(), "not before it answered");
    assert_eq!(
        h.owner_edits(),
        1,
        "the canvas's owner answered inside the call"
    );
    assert!(h.relinked().is_empty(), "nor before it read its new text");
    h.owner_reads();
    let relinked = h.relinked();
    let [(t, Ok(answers))] = &relinked[..] else {
        panic!("{relinked:?}");
    };
    assert_eq!(*t, ticket);
    assert_eq!(h.read("Board.canvas"), LINKED_BOARD, "nothing written yet");
    assert_eq!(h.read("Reading.lines"), LINKED_READING);
    assert!(h.deliver().is_empty(), "nor is anything said");

    rename::answered(&mut plan.edits, answers.clone());
    h.renamed(&plan);
    assert_eq!(h.deliver().names("agenda"), ["items/reset"]);
    assert_eq!(h.read("Board.canvas"), relinked_board());
    let reading = "- [ ] Read the report on [[Tephra]]\n";
    assert_eq!(h.read("Reading.lines"), reading);
    assert!(h.read("Plan.md").contains("[[Tephra]]"));
    assert_eq!(h.task("Read the report").note, "Reading.lines");
    assert_eq!(h.owner_reads(), 0, "what the extension read back is kept");
}

/// One owner's refusal changes nothing anywhere: not the notes, and not the canvas, whose owner
/// had answered already. Nothing is left waiting either.
#[test]
fn a_rename_one_owner_refuses_changes_nothing_anywhere() {
    let mut h = linked("refused");
    h.follow("agenda", tasks());
    let mut plan = h.plan_rename("Ash", "Tephra");
    let ticket = h
        .relink(&mut plan.edits)
        .expect("the extension answers later");
    h.owner_refuses("it is read-only");
    let relinked = h.relinked();
    let [(t, Err(why))] = &relinked[..] else {
        panic!("{relinked:?}");
    };
    assert_eq!(*t, ticket);
    assert_eq!(why, "lines didn't edit Reading.lines: it is read-only");
    assert!(
        rename::apply(&plan).is_err(),
        "nor can it be made after all"
    );
    let before = [
        ("Ash.md", ASH),
        ("Plan.md", PLAN),
        ("Board.canvas", LINKED_BOARD),
        ("Reading.lines", LINKED_READING),
    ];
    for (rel, text) in before {
        assert_eq!(h.read(rel), text, "{rel}");
    }
    assert!(h.deliver().is_empty(), "and nobody hears anything");

    let check = h.task("Check the seismograph");
    let answer = h.edit("agenda", &tick(&check));
    assert!(matches!(answer, Ok(Answer::Later(_))), "{:?}", answer.err());
    assert!(matches!(&h.deliver().edited[..], [(_, _, Ok(_))]));
}

/// A slow owner. Another program writes the file, and the owner reads that, before it answers
/// the edit: its answer is for a text that is gone, so nothing is written, and the edit fails
/// as `changed`. The other program's change is heard once.
#[test]
fn an_owner_s_answer_for_a_file_that_changed_meanwhile_writes_nothing() {
    let mut h = workspace("late-answer");
    h.follow("agenda", tasks());
    let report = h.task("Read the report");
    let Ok(Answer::Later(ticket)) = h.edit("agenda", &tick(&report)) else {
        panic!("an owned file's item is answered later");
    };
    let theirs = format!("{READING}- [ ] Return the book\n");
    h.write("Reading.lines", &theirs);
    h.saw("Reading.lines");
    h.deliver();
    h.owner_reads();
    let read = h.deliver();
    assert_eq!(read.counts("agenda"), [(1, 0, 0)]);
    assert!(read.edited.is_empty(), "{read:?}");

    assert_eq!(h.owner_edits(), 1);
    h.owner_reads();
    let late = h.deliver();
    let [(to, t, Err(refused))] = &late.edited[..] else {
        panic!("{late:?}");
    };
    assert_eq!((to.as_str(), *t), ("agenda", ticket));
    assert_eq!(refused.reason, Reason::Changed, "{refused}");
    assert!(late.events.is_empty() && late.news.is_empty(), "{late:?}");
    assert_eq!(h.read("Reading.lines"), theirs);
    assert_eq!(h.count("agenda", "Return the book"), 1);

    // The task as it reads now can be ticked.
    let report = h.task("Read the report");
    let answer = h.edit("agenda", &tick(&report));
    assert!(matches!(answer, Ok(Answer::Later(_))));
    h.owner_edits();
    h.owner_reads();
    assert!(matches!(&h.deliver().edited[..], [(_, _, Ok(_))]));
    assert!(h.read("Reading.lines").starts_with("- [x] Read the report"));
    assert!(h.read("Reading.lines").ends_with("- [ ] Return the book\n"));
}

/// Two edits of one owned file. An extension's file takes one at a time: the second is refused
/// while the first waits, and goes through once the first is written. A canvas's owner answers
/// inside the call, so two edits in a row are each written and each heard once.
#[test]
fn two_edits_of_one_owned_file_are_each_written_once() {
    let reading = format!("{READING}- [ ] Return the book\n");
    let files = [
        ("Board.canvas", BOARD_OF_TWO),
        ("Reading.lines", reading.as_str()),
    ];
    let mut h = Harness::with("two-edits", &files);
    h.follow("agenda", tasks());
    let (report, book) = (h.task("Read the report"), h.task("Return the book"));
    let Ok(Answer::Later(first)) = h.edit("agenda", &tick(&report)) else {
        panic!("an owned file's item is answered later");
    };
    let busy = h.edit("table", &tick(&book)).unwrap_err();
    assert_eq!(busy.reason, Reason::Changed, "{busy}");
    assert_eq!(h.owner_edits(), 1, "the second was never asked");
    h.owner_reads();
    let batch = h.deliver();
    assert!(matches!(&batch.edited[..], [(_, t, Ok(_))] if *t == first));
    assert_eq!(batch.counts("agenda"), [(0, 1, 0)]);

    let book = h.task("Return the book");
    let Ok(Answer::Later(second)) = h.edit("table", &tick(&book)) else {
        panic!("the file is free again");
    };
    h.owner_edits();
    h.owner_reads();
    let batch = h.deliver();
    assert!(matches!(&batch.edited[..], [(_, t, Ok(_))] if *t == second));
    assert_eq!(batch.counts("agenda"), [(0, 1, 0)]);
    let lines = h.read("Reading.lines");
    let ticked = |line: &str| line.starts_with("- [x] ");
    assert!(lines.lines().count() == 2 && lines.lines().all(ticked));

    let (bolts, check) = (h.task("Tighten the bolts"), h.task("Check the seismograph"));
    for task in [&bolts, &check] {
        let answer = h.edit("agenda", &tick(task));
        assert!(matches!(answer, Ok(Answer::Later(_))), "{:?}", answer.err());
    }
    let batch = h.deliver();
    assert!(matches!(&batch.edited[..], [(_, _, Ok(_)), (_, _, Ok(_))]));
    assert_eq!(batch.counts("agenda"), [(0, 1, 0), (0, 1, 0)]);
    let said: Vec<_> = batch.events.iter().filter_map(edited).collect();
    assert_eq!(said, [("Board.canvas", "agenda", "ticked"); 2]);
    let board = h.read("Board.canvas");
    assert_eq!(board.matches("- [x] ").count(), 2, "{board}");
    let (head, rest) = BOARD_OF_TWO.split_once("- [ ] Tighten the bolts").unwrap();
    let (between, tail) = rest.split_once("- [ ] Check the seismograph").unwrap();
    assert!(board.starts_with(head) && board.ends_with(tail), "{board}");
    assert!(board.contains(between), "{board}");
}

/// A recipe from a shape's step changes a canvas's card through its owner, which answers inside
/// the call, and the canvas is read again with the notes. A file an extension owns would have
/// to wait for it, so the recipe is refused and nothing is changed or left waiting.
#[test]
fn a_recipe_from_a_step_changes_a_canvas_through_its_owner() {
    let files = [
        ("Ash.md", ASH),
        ("Board.canvas", LINKED_BOARD),
        ("Reading.lines", READING),
    ];
    let mut h = Harness::with("recipe-canvas", &files);
    h.follow("agenda", tasks());
    h.run_recipe("rename-tag", &[("from", "ash"), ("to", "tephra")]);
    let run = h.deliver();
    let board = Event::OwnedChanged("Board.canvas".into());
    assert_eq!(run.events, [changed("Ash"), board]);
    assert_eq!(run.names("agenda"), ["items/changed", "items/changed"]);
    let renamed = LINKED_BOARD.replace("#ash", "#tephra");
    assert_eq!(h.read("Board.canvas"), renamed);
    assert!(h.read("Ash.md").contains("#tephra"));
    h.saw("Ash.md");
    h.saw("Board.canvas");
    let late = h.deliver();
    assert_eq!(late.events, [Event::BoardChanged("Board.canvas".into())]);
    assert!(late.news.is_empty(), "{late:?}");

    let tagged = "- [ ] Read the report #tephra\n";
    h.write("Reading.lines", tagged);
    h.saw("Reading.lines");
    h.deliver();
    h.owner_reads();
    h.deliver();
    let again = [("from", "tephra"), ("to", "lapilli")];
    let refused = h.try_recipe("rename-tag", &again).unwrap_err();
    assert_eq!(refused.reason, Reason::Refused, "{refused}");
    assert!(refused.message.contains("Reading.lines"), "{refused}");
    assert_eq!(h.owner_edits(), 0, "its owner isn't asked after all");
    assert_eq!(h.read("Board.canvas"), renamed);
    assert_eq!(h.read("Reading.lines"), tagged);
    assert!(h.read("Ash.md").contains("#tephra"));
    assert!(h.deliver().is_empty(), "and nobody hears anything");
    let check = h.task("Check the seismograph");
    let answer = h.edit("agenda", &tick(&check));
    assert!(matches!(answer, Ok(Answer::Later(_))), "{:?}", answer.err());
}

#[test]
fn a_rename_is_a_reset_and_its_late_watcher_events_are_no_news() {
    let mut h = workspace("rename");
    h.follow("agenda", tasks());
    h.follow_links("graph");
    h.rename("Ash", "Tephra");
    let batch = h.deliver();
    assert_eq!(batch.names("agenda"), ["items/reset"]);
    assert_eq!(batch.names("graph"), ["links/changed"]);
    assert!(h.read("Plan.md").contains("[[Tephra]]"));
    let sweep = h.task("Sweep the ash");
    assert_eq!(sweep.note, "Tephra.md");
    assert!(h.items("agenda").contains(&sweep));

    for rel in ["Plan.md", "Ash.md", "Tephra.md"] {
        h.saw(rel);
    }
    let late = h.deliver();
    assert!(late.is_empty(), "{late:?}");
}

#[test]
fn a_recipe_applied_and_undone_leaves_a_follower_where_it_was() {
    let mut h = workspace("recipe");
    h.follow("agenda", tasks());
    let before = h.items("agenda");
    let rename = [("from", "ash"), ("to", "tephra")];

    // From its screen: applied, then undone, the workspace read again after each.
    let applied = h.recipe("rename-tag", &rename);
    assert_eq!(h.deliver().names("agenda"), ["items/reset"]);
    assert!(h.read("Ash.md").contains("#tephra"));
    assert_ne!(h.items("agenda"), before);
    h.undo(&applied);
    assert_eq!(h.deliver().names("agenda"), ["items/reset"]);
    assert_eq!(h.items("agenda"), before);

    // From a shape's step: the core applies it and reads only its notes again.
    h.run_recipe("rename-tag", &rename);
    let run = h.deliver();
    assert_eq!(run.events, [changed("Ash")]);
    assert_eq!(run.names("agenda"), ["items/changed"]);
    assert!(h.read("Ash.md").contains("#tephra"));
}

#[test]
fn item_news_comes_before_link_news_in_every_batch() {
    let mut h = workspace("links");
    h.follow("graph", tasks());
    h.follow_links("graph");

    h.write("Chores.md", "- [ ] Sweep the roof\n");
    h.saw("Chores.md");
    let created = h.deliver();
    let chores = rels(&["Chores.md"]);
    assert_eq!(created.links("graph"), [(chores.clone(), rels(&[]))]);

    let rent = h.task("Pay rent");
    h.edit("agenda", &set_due(&rent, "2026-12-01")).unwrap();
    h.deliver();

    // Only prose changes: the tasks stay as they are, the links may not.
    h.write("Ash.md", &format!("{ASH}\nIt fell for days.\n"));
    h.saw("Ash.md");
    h.deliver();

    h.rescan();
    h.deliver();

    h.remove("Chores.md");
    h.saw("Chores.md");
    let removed = h.deliver();
    assert_eq!(removed.links("graph"), [(rels(&[]), chores)]);
    assert_eq!(removed.counts("graph"), [(0, 0, 1)]);

    let check = h.task("Check the seismograph");
    h.edit("agenda", &tick(&check)).unwrap();
    h.deliver();

    // Nothing changed: no news of either kind.
    h.saw("Plan.md");
    h.deliver();

    let heard = [
        vec!["items/changed", "links/changed"],
        vec!["items/changed", "links/changed"],
        vec!["links/changed"],
        vec!["items/reset", "links/changed"],
        vec!["items/changed", "links/changed"],
        vec!["items/changed", "links/changed"],
        vec![],
    ];
    assert_eq!(h.heard("graph"), heard);
}

#[test]
fn a_follower_of_the_example_workspace_keeps_what_a_query_finds() {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
    let mut h = Harness::copy_of("example", &from);
    h.follow("agenda", tasks());
    let before = h.items("agenda").len();
    assert!(before > 0, "the example workspace has tasks");

    h.write("Ashfall drill.md", "- [ ] Count the masks\n");
    h.saw("Ashfall drill.md");
    assert_eq!(h.deliver().counts("agenda"), [(1, 0, 0)]);
    h.rescan();
    assert_eq!(h.deliver().names("agenda"), ["items/reset"]);
    h.write(
        "Ashfall drill.md",
        "- [ ] Count the masks\n- [ ] Check the radio\n",
    );
    h.saw("Ashfall drill.md");
    h.saw("Ashfall drill.md");
    assert_eq!(h.deliver().counts("agenda"), [(1, 0, 0)]);
    assert_eq!(h.items("agenda").len(), before + 2);
    h.remove("Ashfall drill.md");
    h.saw("Ashfall drill.md");
    assert_eq!(h.deliver().counts("agenda"), [(0, 0, 2)]);
    assert_eq!(h.items("agenda").len(), before);
}

/// What a delete and a restore are for the client `by`, as events.
fn deleted(path: &str, by: &str) -> Event {
    let (path, by) = (path.to_string(), by.to_string());
    Event::Deleted { path, by }
}

fn restored(path: &str, by: &str) -> Event {
    let (path, by) = (path.to_string(), by.to_string());
    Event::Restored { path, by }
}

/// #193. A delete through the gateway is read inside the call, so the watcher's own report of
/// it, late and twice, is no news; asked twice, the second is refused and says nothing.
#[test]
fn a_delete_and_its_restore_are_heard_once_each_in_order_and_name_who_made_them() {
    let mut h = workspace("trash");
    h.follow("agenda", tasks());
    h.follow_links("graph");
    let seen = h.stat("Ash.md");
    let gone = h.delete("tidy", "Ash.md", &seen).unwrap();
    assert_eq!(gone.went, crate::trash::Went::WorkspaceTrash);
    assert_eq!(h.read(".trash/Ash.md"), ASH, "in the test's own folder");
    let twice = h.delete("tidy", "Ash.md", &seen).unwrap_err();
    assert_eq!(twice.reason, Reason::Changed);

    let batch = h.deliver();
    let events = [Event::NoteRemoved("Ash".into()), deleted("Ash.md", "tidy")];
    assert_eq!(batch.events, events);
    assert_eq!(batch.channel, events);
    assert_eq!(batch.counts("agenda"), [(0, 0, 1)]);
    assert_eq!(batch.links("graph"), [(rels(&[]), rels(&["Ash.md"]))]);
    assert_eq!(h.count("agenda", "Sweep the ash"), 0);
    h.saw("Ash.md");
    h.saw("Ash.md");
    assert!(h.deliver().is_empty(), "the watcher's report is no news");

    let back = h.restore("tidy", &gone.trashed).unwrap();
    assert_eq!(back.path, "Ash.md");
    assert_eq!(h.read("Ash.md"), ASH);
    let spent = h.restore("tidy", &gone.trashed).unwrap_err();
    assert_eq!(spent.reason, Reason::Changed);
    let batch = h.deliver();
    let events = [Event::NoteCreated("Ash".into()), restored("Ash.md", "tidy")];
    assert_eq!(batch.events, events);
    assert_eq!(batch.channel, events);
    assert_eq!(batch.counts("agenda"), [(1, 0, 0)]);
    assert_eq!(batch.links("graph"), [(rels(&["Ash.md"]), rels(&[]))]);
    assert_eq!(h.count("agenda", "Sweep the ash"), 1);
    h.saw("Ash.md");
    assert!(h.deliver().is_empty(), "the watcher's report is no news");

    let once = vec!["items/changed"];
    assert_eq!(h.heard("agenda"), [once.clone(), vec![], once, vec![]]);
}

/// The dialog read the note, another program wrote it, and the watcher hasn't said so yet: the
/// delete is refused, the note stays as they wrote it, and nobody hears of a delete.
#[test]
fn a_note_changed_since_the_dialog_read_it_is_not_deleted() {
    let mut h = workspace("trash-changed");
    h.follow("agenda", tasks());
    let seen = h.stat("Ash.md");
    let theirs = format!("{ASH}- [ ] Buy a broom\n");
    h.write("Ash.md", &theirs);
    let refused = h.delete("tidy", "Ash.md", &seen).unwrap_err();
    assert_eq!(refused.reason, Reason::Changed);
    assert_eq!(h.read("Ash.md"), theirs);
    assert!(h.deliver().is_empty(), "nothing was deleted");

    h.saw("Ash.md");
    let batch = h.deliver();
    assert_eq!(batch.events, [changed("Ash")]);
    assert_eq!(batch.counts("agenda"), [(1, 0, 0)]);
}

/// The other order: the core is behind the disk when the note is deleted at the version on
/// disk. Subscribers lose the task they knew of, and the watcher's late report of the write
/// is no news.
#[test]
fn a_delete_of_a_note_the_core_has_not_read_again_removes_what_subscribers_had() {
    let mut h = workspace("trash-behind");
    h.follow("agenda", tasks());
    h.write("Ash.md", &format!("{ASH}- [ ] Buy a broom\n"));
    let seen = h.stat("Ash.md");
    h.delete("tidy", "Ash.md", &seen).unwrap();
    let batch = h.deliver();
    let events = [Event::NoteRemoved("Ash".into()), deleted("Ash.md", "tidy")];
    assert_eq!(batch.events, events);
    assert_eq!(batch.counts("agenda"), [(0, 0, 1)]);
    h.saw("Ash.md");
    assert!(h.deliver().is_empty(), "the write it never read is no news");
    assert_eq!(h.count("agenda", "Buy a broom"), 0);
}

/// A folder goes and comes back with a rescan, so its subscribers hear a reset and query
/// again. The watcher's report of a folder is a rescan too, as for any folder.
#[test]
fn a_folders_delete_and_restore_are_each_a_reset_then_the_event() {
    let files = [
        ("Plan.md", PLAN),
        ("Zone/Ash.md", ASH),
        ("Zone/Deep/Chores.md", "- [ ] Clear the gutters\n"),
    ];
    let mut h = Harness::with("trash-folder", &files);
    h.follow("agenda", tasks());
    let seen = h.stat("Zone");
    let gone = h.delete("tidy", "Zone", &seen).unwrap();
    let batch = h.deliver();
    assert_eq!(batch.events, [Event::Rescanned, deleted("Zone", "tidy")]);
    assert_eq!(batch.names("agenda"), ["items/reset"]);
    assert_eq!(h.count("agenda", "Sweep the ash"), 0);
    assert_eq!(h.count("agenda", "Clear the gutters"), 0);
    assert_eq!(h.count("agenda", "Pay rent"), 1);

    h.restore("tidy", &gone.trashed).unwrap();
    assert_eq!(h.read("Zone/Deep/Chores.md"), "- [ ] Clear the gutters\n");
    let batch = h.deliver();
    assert_eq!(batch.events, [Event::Rescanned, restored("Zone", "tidy")]);
    assert_eq!(batch.names("agenda"), ["items/reset"]);
    assert_eq!(h.count("agenda", "Sweep the ash"), 1);
    assert_eq!(h.count("agenda", "Clear the gutters"), 1);

    h.saw("Zone");
    let late = h.deliver();
    assert_eq!(late.events, [Event::Rescanned]);
    assert_eq!(late.names("agenda"), ["items/reset"]);
    assert_eq!(h.count("agenda", "Clear the gutters"), 1);
}
