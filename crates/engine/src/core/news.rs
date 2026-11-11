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
//! - One call never queues item news behind an `items/reset` of the same subscription
//!   (`Harness::call` checks it on every call every test makes). The core queues nothing
//!   between calls, so a reset shares a batch with a later change only when nobody took the
//!   news between the two calls. `rescan_now` leaves its reset in the queue without an event
//!   for `poll` and without `has_unseen`, so whoever calls it takes the news at once.
//! - An edit of a file another owner reads is news when the owner has answered and the file is
//!   written, with the answer in the same batch.

use super::*;
use crate::gateway::{Filter, Page};
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

/// A subscriber that keeps its items the way `Feed` in the binary does: it queries when it
/// subscribes and on `items/reset`, and applies `items/changed` to what it has. An item it is
/// told of twice is there twice.
struct Follower {
    client: String,
    select: Select,
    sub: SubId,
    items: Vec<Item>,
}

impl Follower {
    fn hear(&mut self, core: &Core, news: &News) {
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
        let kind = OwnedKind {
            extension: "lines".into(),
            owner: OWNER.into(),
            version: "1".into(),
        };
        core.set_owners(vec![kind], None);
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

    /// One call into the core. It must not queue item news behind a reset of the same
    /// subscription: a subscriber queries again on the reset, and would count that news twice.
    fn call<T>(&mut self, f: impl FnOnce(&mut Core) -> T) -> T {
        let before = self.core.news.len();
        let out = f(&mut self.core);
        let queued = &self.core.news[before..];
        for (i, (_, news)) in queued.iter().enumerate() {
            let behind = |(_, later): &(String, News)| later.subscription() == news.subscription();
            let reset = matches!(news, News::Reset { .. });
            assert!(
                !(reset && queued[i + 1..].iter().any(behind)),
                "one call queued news behind a reset: {queued:?}"
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

    /// A rename as the menu makes it: planned, applied, then everything read again.
    pub(crate) fn rename(&mut self, from: &str, to: &str) {
        let note = self.core.index.by_rel(from).unwrap();
        let plan = rename::plan(&self.core.index, note, to).unwrap();
        rename::apply(&plan).unwrap();
        self.rescan();
    }

    /// A recipe as its screen runs it: planned, applied, then everything read again.
    pub(crate) fn recipe(&mut self, id: &str, options: &[(&str, &str)]) -> recipe::Applied {
        let recipe = recipe::find(id).unwrap();
        let plan = recipe::plan(&self.core.index, recipe, named(options)).unwrap();
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
        let run = self.call(|core| core.run_recipe(id, named(options)));
        run.unwrap();
    }

    /// The owner answers the `file/edit` requests waiting for it.
    pub(crate) fn owner_edits(&mut self) {
        for (owner, ask) in self.core.take_file_edits() {
            let text = lines_edit(&ask);
            self.call(|core| core.file_edited(&owner, &ask.path, &ask.rev, Ok(text)));
        }
    }

    /// The owner answers the `file/read` requests waiting for it.
    pub(crate) fn owner_reads(&mut self) {
        for (owner, read) in self.core.take_reads() {
            let given = lines_given(&read.text);
            self.call(|core| core.file_given(&owner, &read.path, &read.rev, Ok(given)));
        }
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

    /// Hand out what the core has to say. Every follower then has what a fresh query finds.
    pub(crate) fn deliver(&mut self) -> Batch {
        let batch = self.deliver_unchecked();
        for f in &self.followers {
            let fresh = sorted(all(&self.core, &f.select));
            assert_eq!(sorted(f.items.clone()), fresh, "what {} has", f.client);
        }
        batch
    }

    /// `deliver`, without checking the followers.
    pub(crate) fn deliver_unchecked(&mut self) -> Batch {
        let batch = self.take();
        for (to, news) in &batch.news {
            let sub = news.subscription();
            let mut followers = self.followers.iter_mut();
            if let Some(f) = followers.find(|f| f.client == *to && f.sub == sub) {
                f.hear(&self.core, news);
            }
        }
        self.log.push(batch.clone());
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
        place: (i + 1).to_string(),
        text: line.to_string(),
    };
    Given {
        items: text.lines().enumerate().map(piece).collect(),
        links: Vec::new(),
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

/// #172 at engine level. The core doesn't keep a reset apart from what follows it: news taken
/// only after the next change holds both, the subscriber's query on the reset already finds the
/// new task, and the change adds it again. So a frontend takes the news right after
/// `rescan_now` (`reload_tree` in the menu). If the core ever keeps them apart itself, this
/// test is the one to turn around.
#[test]
fn a_reset_left_in_the_queue_shares_a_batch_with_the_next_change() {
    let mut h = workspace("merged");
    h.follow("agenda", tasks());
    h.rescan();
    h.write("Chores.md", "- [ ] Sweep the roof\n");
    h.saw("Chores.md");
    let both = h.deliver_unchecked();
    assert_eq!(both.names("agenda"), ["items/reset", "items/changed"]);
    assert_eq!(h.count("agenda", "Sweep the roof"), 2, "counted twice");
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
