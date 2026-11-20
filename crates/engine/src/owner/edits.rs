//! The edit desk: edits of files other owners read (`file/edit`). The core makes the change the
//! way it edits a note (a Markdown piece's new text through the Markdown owner, or a typed
//! item's field) and reads the file (`read::Seen`), and the desk asks the file's owner for the
//! file's new text. Before it is written, the desk asks the owner to read that text
//! (`file/read`) and checks it reads as asked: what the owner gave before, with the change made.
//! Only then does it hand the text to the core (`ToWrite`), which writes it over the file as it
//! was read (`Seen::replace`, refused if it changed since) and says how that went (`wrote`): the
//! desk answers whoever asked, whatever happened. An extension answers later, so each edit
//! waits here, one per file, until its owner has answered both and the core has written it; the
//! engine's canvas owner answers both inside the call, through the same steps. A rename, merge
//! or recipe asks every owned file's owner this way before anything is written, and keeps each
//! new text that read back until all are in (`Relink`). The desk reads and writes no file and
//! tells no subscriber: the core does.

use crate::bounded::{Message, Text};
use crate::gateway::{Edited, Error, Reason, Ticket};
use crate::items::{Item, Settings};
use crate::owner::cache::FileRead;
use crate::owner::given::{self, Change, Given};
use crate::read::{self, Seen};
use crate::rename;
use crate::workspace::Owned;
use crate::write::WriteError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// `file/edit` (host → owner): the new text of the file at `path`, whose text is `text` at
/// version `rev`, with `changes` made. The owner answers `{text}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEdit {
    pub path: String,
    pub rev: String,
    pub text: String,
    pub changes: Vec<Change>,
}

/// The owner's answer to `file/edit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewText {
    /// The file's whole text, which the owner made.
    pub text: Text<Message>,
}

/// An `items/edit` the core made into changes to an owned file: who asked, the item as it
/// should read once written and a recurring task's next occurrence, and what was done (for the
/// status line) and a warning, for the answer.
pub(crate) struct ItemEdit {
    by: String,
    item: Box<Item>,
    next: Option<Box<Item>>,
    what: String,
    warning: Option<String>,
}

impl ItemEdit {
    pub fn new(
        by: &str,
        item: Box<Item>,
        next: Option<Box<Item>>,
        what: String,
        warning: Option<String>,
    ) -> ItemEdit {
        ItemEdit {
            by: by.to_string(),
            item,
            next,
            what,
            warning,
        }
    }

    /// The answer once its file at `path` is written and read again, holding `items`: the item
    /// as it reads now, found by its id, else by what it holds, and the next occurrence the
    /// same way.
    fn answer(self, path: &str, items: &[Item]) -> Result<Edited, Error> {
        let ItemEdit {
            item,
            next,
            warning,
            ..
        } = self;
        let find = |want: &Item| {
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
        answer.ok_or_else(|| {
            let message = format!("{path} was written, but {} isn't in it now", item.id);
            Error::new(Reason::ReadBack, message)
        })
    }
}

/// An item edit whose new text read back as asked, for the core to write: `after` over the file
/// as the edit was made on it (`before`, which refuses the write if the file changed since), at
/// the workspace-relative `path`. Once it is written, what `owner` read in it (`given`, at
/// version `rev`) is the file's, and `by`'s edit of `item` did `what`. The core says how the
/// write went (`Edits::wrote`), and the desk answers `by`.
pub(crate) struct ToWrite {
    pub path: String,
    pub before: Seen,
    pub after: String,
    pub rev: String,
    pub owner: String,
    pub given: Given,
    pub by: String,
    pub item: String,
    pub what: String,
}

/// An edit of one owned file, from asking its owner until it is written, or fails.
struct Pending {
    owner: String,
    /// The file's version and text the change was made on.
    rev: String,
    text: String,
    /// What the owner should give for the file once it is written. `None`: it couldn't read the
    /// file before the change, and isn't expected to read it after.
    expect: Option<Given>,
    goal: Goal,
    stage: Stage,
}

enum Stage {
    /// `file/edit` was asked.
    Editing,
    /// The owner answered with the file's new text, at version `rev`, and was asked to read it.
    Reading { rev: String, text: String },
    /// An item edit's new text read back as asked, and the core is writing it (`ToWrite`): it
    /// says how that went (`Edits::wrote`).
    Writing,
}

/// What the edit is for.
enum Goal {
    /// An `items/edit` of the file at `at` on disk, answered under the ticket.
    Item {
        ticket: Ticket,
        edit: ItemEdit,
        at: PathBuf,
    },
    /// Links in one of the files of the rename or merge `Ticket`, whose new text is kept, not
    /// written.
    Relink(Ticket),
}

/// A rename or merge waiting for the owners of its owned files: their edits, each given its
/// new text (and what the owner read in it) as it reads back.
struct Relink {
    edits: Vec<rename::FileEdit>,
}

impl Relink {
    fn done(&self) -> bool {
        self.edits.iter().all(is_answered)
    }

    /// The edits whose owners have answered so far.
    fn answered(&self) -> Vec<rename::FileEdit> {
        let answered = self.edits.iter().filter(|e| is_answered(e));
        answered.cloned().collect()
    }
}

fn is_answered(e: &rename::FileEdit) -> bool {
    e.owned.as_ref().is_some_and(|o| o.after.is_some())
}

/// The edit desk: the edits waiting for owners, and what is to be sent and handed out.
#[derive(Default)]
pub(crate) struct Edits {
    next: u64,
    /// By the file's workspace-relative path.
    pending: HashMap<String, Pending>,
    /// `file/edit` to send, each with its owner.
    asks: Vec<(String, FileEdit)>,
    /// `file/read` of an owner's new text, to send.
    reads: Vec<(String, FileRead)>,
    /// Answers to `items/edit`, by who asked.
    done: Vec<(String, Ticket, Result<Edited, Error>)>,
    /// Renames and merges waiting for owners.
    relinks: HashMap<Ticket, Relink>,
    /// Their answers: the owned files' edits with their new texts, or why the owner didn't.
    relinked: Vec<(Ticket, Result<Vec<rename::FileEdit>, String>)>,
}

impl Edits {
    fn ticket(&mut self) -> Ticket {
        self.next += 1;
        Ticket(self.next)
    }

    /// The owner an edit of the file at `path` waits for, if one does: a file has one edit at
    /// a time, until it is written.
    pub fn waiting(&self, path: &str) -> Option<&str> {
        self.pending.get(path).map(|p| p.owner.as_str())
    }

    /// `edit` of an item in the owned `file`, made into `changes` on the file as the core read
    /// it (`before`), after which its owner should give `expect`: refused when the file changed
    /// since its owner read it, or isn't UTF-8 text. Else its owner is asked for the file's new
    /// text (`file/edit`). The ticket the answer comes under (`take_edited`), and what to write
    /// when an owner in the engine answered inside the call. No other edit of the file may be
    /// waiting (`waiting`).
    pub fn edit_item(
        &mut self,
        file: &Owned,
        before: &Seen,
        changes: Vec<Change>,
        expect: Given,
        edit: ItemEdit,
        s: &Settings,
    ) -> Result<(Ticket, Option<ToWrite>), Error> {
        let error = Error::new;
        let rev = before.rev();
        if file.rev.as_deref() != Some(rev.as_str()) {
            let message = format!("{} changed since it was read: read it again", file.rel);
            return Err(error(Reason::Changed, message));
        }
        let text = match before.text() {
            Ok(text) => text.to_string(),
            Err(not) => return Err(error(Reason::Refused, not.about(&file.rel))),
        };
        let ticket = self.ticket();
        let at = before.path().to_path_buf();
        let pending = Pending {
            owner: file.owner.clone(),
            rev,
            text,
            expect: Some(expect),
            goal: Goal::Item { ticket, edit, at },
            stage: Stage::Editing,
        };
        Ok((ticket, self.ask(file.rel.clone(), changes, pending, s)))
    }

    /// Ask the owner of the file at `rel` for its new text with `changes` made (`file/edit`),
    /// and keep `edit` until it has answered: an extension later, once the frontend has handed
    /// it the request (`take_file_edits`), its answer coming in through `edited`; an owner in
    /// the engine here, before this returns.
    fn ask(
        &mut self,
        rel: String,
        changes: Vec<Change>,
        edit: Pending,
        s: &Settings,
    ) -> Option<ToWrite> {
        let owner = edit.owner.clone();
        let ask = FileEdit {
            path: rel.clone(),
            rev: edit.rev.clone(),
            text: edit.text.clone(),
            changes,
        };
        self.pending.insert(rel, edit);
        match crate::owner::edit(&owner, &ask.path, &ask.text, &ask.changes) {
            Some(text) => self.edited(&owner, &ask.path, &ask.rev, text, s),
            None => {
                self.asks.push((owner, ask));
                None
            }
        }
    }

    /// `owner` answered `file/edit` for the file at `path` at version `rev`: the file's new
    /// text, or why not. The new text is read through the owner before it is written: what to
    /// write when an owner in the engine read it inside the call.
    pub fn edited(
        &mut self,
        owner: &str,
        path: &str,
        rev: &str,
        text: Result<String, String>,
        s: &Settings,
    ) -> Option<ToWrite> {
        let asked = |p: &Pending| p.owner == owner && p.rev == rev;
        let p = self.pending.get_mut(path).filter(|p| asked(p))?;
        if !matches!(p.stage, Stage::Editing) {
            return None;
        }
        let error = Error::new;
        match text {
            Err(why) => {
                let message = format!("{owner} didn't edit {path}: {why}");
                self.failed(path, error(Reason::Refused, message));
                None
            }
            Ok(text) if text == p.text => {
                let message = format!("{owner}'s new text for {path} is the same as before");
                self.failed(path, error(Reason::ReadBack, message));
                None
            }
            Ok(text) => {
                let rev = read::rev(text.as_bytes());
                let read = FileRead {
                    path: path.to_string(),
                    rev: rev.clone(),
                    text: text.clone(),
                };
                p.stage = Stage::Reading { rev, text };
                self.send_read_back(owner, read, s)
            }
        }
    }

    /// `file/read` of a new text `owner` made goes to it: to an extension later
    /// (`take_reads`), its answer coming in through `read_back`; to an owner in the engine
    /// here, read back before this returns.
    fn send_read_back(&mut self, owner: &str, read: FileRead, s: &Settings) -> Option<ToWrite> {
        match crate::owner::give(owner, &read.path, &read.text) {
            Some(given) => {
                let read_back = self.read_back(&read.path, &read.rev, given, s);
                read_back.ok().flatten()
            }
            None => {
                self.reads.push((owner.to_string(), read));
                None
            }
        }
    }

    /// The owner read the file at `path` at version `rev` (`given`). When that is the new text
    /// an edit of the file waits to have read back, the desk takes it: an item edit's new text
    /// is handed back to be written and a relink's is kept, when it reads as asked; else the
    /// edit fails, and nothing is written. Any other version is the file as the owner found
    /// it, not the desk's: `given` is handed back.
    pub fn read_back(
        &mut self,
        path: &str,
        rev: &str,
        given: Result<Given, String>,
        s: &Settings,
    ) -> Result<Option<ToWrite>, Result<Given, String>> {
        let ours = |p: &Pending| matches!(&p.stage, Stage::Reading { rev: r, .. } if r == rev);
        let Some(p) = self.pending.get(path).filter(|p| ours(p)) else {
            return Err(given);
        };
        let Stage::Reading { text, .. } = &p.stage else {
            return Err(given);
        };
        let error = Error::new;
        let owner = p.owner.clone();
        let given = match given {
            Ok(g) => g,
            // It couldn't read the file before the change either, and gave nothing for it.
            Err(_) if p.expect.is_none() => Given::default(),
            Err(why) => {
                let message = format!("{owner} couldn't read its new text for {path}: {why}");
                self.failed(path, error(Reason::ReadBack, message));
                return Ok(None);
            }
        };
        let nothing = Given::default();
        let asked = given::read_back(path, p.expect.as_ref().unwrap_or(&nothing), s);
        let gave = given::read_back(path, &given, s);
        if let Err(diff) = given::same(&asked, &gave) {
            let message = format!(
                "{owner}'s new text for {path} doesn't read back as asked ({diff}); nothing was \
                 written"
            );
            self.failed(path, error(Reason::ReadBack, message));
            return Ok(None);
        }
        if let Goal::Relink(ticket) = p.goal {
            self.relink_read_back(ticket, path, text.clone(), given);
            return Ok(None);
        }
        Ok(self.to_write(path, owner, given))
    }

    /// The item edit of the file at `path`, whose new text read back as asked (`given`, what
    /// `owner` read in it), is handed to the core to write; it waits until the core says how
    /// that went (`wrote`).
    fn to_write(&mut self, path: &str, owner: String, given: Given) -> Option<ToWrite> {
        let p = self.pending.get_mut(path)?;
        let Goal::Item { edit, at, .. } = &p.goal else {
            return None;
        };
        let Stage::Reading { rev, text } = &mut p.stage else {
            return None;
        };
        let (rev, text) = (std::mem::take(rev), std::mem::take(text));
        p.stage = Stage::Writing;
        Some(ToWrite {
            path: path.to_string(),
            before: Seen::kept(at, p.text.as_str()),
            after: text,
            rev,
            owner,
            given,
            by: edit.by.clone(),
            item: edit.item.id.clone(),
            what: edit.what.clone(),
        })
    }

    /// The core wrote what it was handed for the file at `path` (`ToWrite`) and read the file
    /// again, which now holds `items`, or the write failed: whoever asked for the edit is
    /// answered (`take_edited`), and the file is free for another edit.
    pub fn wrote(&mut self, path: &str, written: Result<&[Item], WriteError>) {
        let writing = |p: &Pending| matches!(p.stage, Stage::Writing);
        if !self.pending.get(path).is_some_and(writing) {
            return;
        }
        let Some(p) = self.pending.remove(path) else {
            return;
        };
        let Goal::Item { ticket, edit, .. } = p.goal else {
            return;
        };
        let by = edit.by.clone();
        let answer = match written {
            Ok(items) => edit.answer(path, items),
            Err(e) => Err(Error::new(Reason::of(&e), format!("{path}: {e}"))),
        };
        self.done.push((by, ticket, answer));
    }

    /// The edit of the file at `path` waiting for its owner failed: tell whoever asked.
    fn failed(&mut self, path: &str, e: Error) {
        let Some(p) = self.pending.remove(path) else {
            return;
        };
        match p.goal {
            Goal::Item { ticket, edit, .. } => self.done.push((edit.by, ticket, Err(e))),
            Goal::Relink(ticket) => self.relink_failed(ticket, e.message),
        }
    }

    /// Answers to `items/edit` that were answered `Later`: who asked, under which ticket, and
    /// the answer.
    pub fn take_edited(&mut self) -> Vec<(String, Ticket, Result<Edited, Error>)> {
        std::mem::take(&mut self.done)
    }

    /// `file/edit` requests for the owners, each with its owner's name.
    pub fn take_file_edits(&mut self) -> Vec<(String, FileEdit)> {
        std::mem::take(&mut self.asks)
    }

    /// `file/read` requests of the new texts owners made, each with its owner's name.
    pub fn take_reads(&mut self) -> Vec<(String, FileRead)> {
        std::mem::take(&mut self.reads)
    }

    /// Before a rename, merge or recipe writes anything: ask the owner of each file among
    /// `edits` that another owner reads for its new text (`file/edit`), and have the owner read
    /// it back. `None` when `edits` are ready: there was nothing to ask, or every owner answered
    /// inside the call (the engine's own do) and `edits` have their new texts. Else the answer
    /// comes from `take_relinked` under the ticket: every owned file's edit with its new text,
    /// or the first owner's refusal, after which nothing else is kept. Refused at once when one
    /// of them has an edit waiting, changed since it was planned (`planned` says whether a file
    /// still holds the text it was planned on), its change would leave out something its owner
    /// gave (`given::check_change`: a link target past its bound), or an owner in the engine
    /// refuses.
    pub fn relink(
        &mut self,
        edits: &mut [rename::FileEdit],
        planned: impl Fn(&rename::FileEdit) -> bool,
        s: &Settings,
    ) -> Result<Option<Ticket>, String> {
        let owned: Vec<_> = edits
            .iter()
            .filter(|e| e.owned.is_some())
            .cloned()
            .collect();
        if owned.is_empty() {
            return Ok(None);
        }
        // What each owner should give once its file is changed, asked of the one rule.
        let mut expected: Vec<Option<Given>> = Vec::with_capacity(owned.len());
        for e in &owned {
            let o = e.owned.as_ref().expect("owned");
            if self.pending.contains_key(&e.file) {
                return Err(format!("{} has an edit waiting for {}", e.file, o.owner));
            }
            if !planned(e) {
                return Err(format!("{} changed since it was planned", e.file));
            }
            expected.push(match &o.given {
                Some(g) => Some(given::check_change(&e.file, g, &o.changes, s)?),
                None => None,
            });
        }
        let ticket = self.ticket();
        let asks: Vec<_> = (owned.iter().zip(expected))
            .map(|(e, expect)| {
                let o = e.owned.as_ref().expect("owned");
                let pending = Pending {
                    owner: o.owner.clone(),
                    rev: read::rev(e.old_text.as_bytes()),
                    text: e.old_text.clone(),
                    expect,
                    goal: Goal::Relink(ticket),
                    stage: Stage::Editing,
                };
                (e.file.clone(), o.changes.clone(), pending)
            })
            .collect();
        self.relinks.insert(ticket, Relink { edits: owned });
        for (file, changes, pending) in asks {
            // An owner in the engine answers inside `ask`, and its refusal ends the wait. It
            // hands back nothing to write: a relink's new text is kept.
            if self.relinks.contains_key(&ticket) {
                self.ask(file, changes, pending, s);
            }
        }
        let answered = |(t, _): &(Ticket, _)| *t == ticket;
        let Some(at) = self.relinked.iter().position(answered) else {
            // The answers that are in go into `edits`, so they say whose are still to come.
            let relink = self.relinks.get(&ticket);
            let so_far = relink.map(|r| r.answered()).unwrap_or_default();
            rename::answered(edits, so_far);
            return Ok(Some(ticket));
        };
        let (_, answers) = self.relinked.remove(at);
        rename::answered(edits, answers?);
        Ok(None)
    }

    /// Renames and merges whose owners have all answered (`relink`), or one refused.
    pub fn take_relinked(&mut self) -> Vec<(Ticket, Result<Vec<rename::FileEdit>, String>)> {
        std::mem::take(&mut self.relinked)
    }

    /// Stop waiting for the owners of the rename or merge `ticket`. Their answers are dropped.
    pub fn cancel_relink(&mut self, ticket: Ticket) {
        self.relinks.remove(&ticket);
        let ours = |p: &Pending| matches!(p.goal, Goal::Relink(t) if t == ticket);
        self.pending.retain(|_, p| !ours(p));
        let pending = &self.pending;
        (self.asks).retain(|(_, a)| pending.contains_key(&a.path));
        (self.reads).retain(|(_, r)| pending.contains_key(&r.path));
    }

    /// One owned file of the rename or merge `ticket` read back as asked: keep its new text.
    fn relink_read_back(&mut self, ticket: Ticket, path: &str, text: String, given: Given) {
        self.pending.remove(path);
        let Some(relink) = self.relinks.get_mut(&ticket) else {
            return;
        };
        if let Some(e) = relink.edits.iter_mut().find(|e| e.file == path) {
            e.new_text = text;
            if let Some(o) = e.owned.as_mut() {
                o.after = Some(given);
            }
        }
        if relink.done() {
            let relink = self.relinks.remove(&ticket).expect("waiting");
            self.relinked.push((ticket, Ok(relink.edits)));
        }
    }

    /// An owner refused, or didn't answer, one file of the rename or merge `ticket`: it is
    /// cancelled, with `message`.
    fn relink_failed(&mut self, ticket: Ticket, message: String) {
        if self.relinks.contains_key(&ticket) {
            self.cancel_relink(ticket);
            self.relinked.push((ticket, Err(message)));
        }
    }

    /// Whether there is something to send or to hand out.
    pub fn has_news(&self) -> bool {
        !(self.asks.is_empty()
            && self.reads.is_empty()
            && self.done.is_empty()
            && self.relinked.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const OWNER: &str = "csv-table";
    const FILE: &str = "Samples.csv";
    const OPEN: &str = "- [ ] sample\n";
    const DONE: &str = "- [x] sample\n";

    /// The settings of a workspace with nothing of its own.
    fn settings() -> Settings {
        Settings::load(Path::new("/nonexistent-cold-lava-workspace"))
    }

    /// What the owner gives for a file of one Markdown piece, at the place `1`.
    fn gives(text: &str) -> Given {
        serde_json::from_value(serde_json::json!({"items": [{"place": "1", "text": text}]}))
            .unwrap()
    }

    /// `FILE` as its owner read it, holding `OPEN`.
    fn owned() -> Owned {
        let mut file = Owned::new(FILE.into(), PathBuf::from(FILE), OWNER);
        file.rev = Some(read::rev(OPEN.as_bytes()));
        file
    }

    /// `FILE` as the core reads it now, holding `bytes`.
    fn seen(bytes: impl Into<Vec<u8>>) -> Seen {
        Seen::kept(Path::new(FILE), bytes)
    }

    /// Ticking the task in `FILE` as the core makes it: the change to its one Markdown piece,
    /// what its owner should give after it, and the item edit.
    fn tick(s: &Settings) -> (Vec<Change>, Given, ItemEdit) {
        let changes = vec![Change::Text {
            place: "1".into(),
            text: "- [x] sample".into(),
        }];
        let expect = given::check_change(FILE, &gives("- [ ] sample"), &changes, s).unwrap();
        let items = given::read(FILE, &expect, s).items;
        let item = Box::new(items.into_iter().next().expect("a task"));
        let edit = ItemEdit::new("agenda", item, None, "ticked".into(), None);
        (changes, expect, edit)
    }

    #[test]
    fn an_item_edit_waits_for_its_owner_then_is_handed_back_to_be_written() {
        let s = settings();
        let mut desk = Edits::default();
        let (changes, expect, edit) = tick(&s);
        let made = desk.edit_item(&owned(), &seen(OPEN), changes.clone(), expect, edit, &s);
        let (ticket, write) = made.unwrap();
        assert!(write.is_none(), "an extension answers later");
        assert_eq!(ticket, Ticket(1));
        assert_eq!(desk.waiting(FILE), Some(OWNER), "one at a time");
        assert!(desk.has_news());
        let asks = desk.take_file_edits();
        let [(owner, ask)] = &asks[..] else {
            panic!("{} asks", asks.len());
        };
        let asked = FileEdit {
            path: FILE.into(),
            rev: read::rev(OPEN.as_bytes()),
            text: OPEN.into(),
            changes,
        };
        assert_eq!((owner.as_str(), ask), (OWNER, &asked));
        let early = desk.read_back(FILE, &ask.rev, Ok(gives("- [ ] sample")), &s);
        assert!(early.is_err(), "not before file/edit is answered");

        assert!((desk.edited("other", FILE, &ask.rev, Ok(DONE.into()), &s)).is_none());
        assert!((desk.edited(OWNER, FILE, "older", Ok(DONE.into()), &s)).is_none());
        assert!(!desk.has_news(), "someone else's answer changes nothing");

        assert!((desk.edited(OWNER, FILE, &ask.rev, Ok(DONE.into()), &s)).is_none());
        let reads = desk.take_reads();
        let [(owner, back)] = &reads[..] else {
            panic!("{} reads", reads.len());
        };
        let new = FileRead {
            path: FILE.into(),
            rev: read::rev(DONE.as_bytes()),
            text: DONE.into(),
        };
        assert_eq!((owner.as_str(), back), (OWNER, &new));
        let old = desk.read_back(FILE, &ask.rev, Ok(gives("- [ ] sample")), &s);
        let Err(Ok(handed_back)) = old else {
            panic!("the old text is no read-back: what the owner read in it is handed back");
        };
        assert_eq!(handed_back, gives("- [ ] sample"));
        assert!((desk.edited(OWNER, FILE, &ask.rev, Ok(DONE.into()), &s)).is_none());
        assert!(desk.take_reads().is_empty(), "file/edit is answered once");

        let read = desk.read_back(FILE, &back.rev, Ok(gives("- [x] sample")), &s);
        let Ok(Some(w)) = read else {
            panic!("reads as asked");
        };
        assert_eq!(w.before, seen(OPEN), "over the file as it was read");
        assert_eq!((w.path.as_str(), w.after.as_str()), (FILE, DONE));
        let owner_read = (w.owner.as_str(), w.rev.as_str());
        assert_eq!(owner_read, (OWNER, back.rev.as_str()));
        assert_eq!((w.by.as_str(), w.what.as_str()), ("agenda", "ticked"));
        assert_eq!(desk.waiting(FILE), Some(OWNER), "until it is written");
        assert!(desk.take_edited().is_empty(), "answered once written");

        let items = given::read(FILE, &gives("- [x] sample"), &s).items;
        desk.wrote(FILE, Ok(items.as_slice()));
        assert_eq!(desk.waiting(FILE), None);
        let done = desk.take_edited();
        let [(by, t, Ok(edited))] = &done[..] else {
            panic!("{done:?}");
        };
        let found = (by.as_str(), *t, edited.item.id.as_str(), edited.changed);
        assert_eq!(found, ("agenda", ticket, w.item.as_str(), true));
    }

    /// Tick the task in `FILE`, and have its owner answer and read its new text back as asked:
    /// what the core is handed to write.
    fn ticked(desk: &mut Edits, s: &Settings) -> ToWrite {
        let (changes, expect, edit) = tick(s);
        let made = desk.edit_item(&owned(), &seen(OPEN), changes, expect, edit, s);
        assert!(made.unwrap().1.is_none());
        let (_, ask) = desk.take_file_edits().pop().expect("asked");
        assert!((desk.edited(OWNER, FILE, &ask.rev, Ok(DONE.into()), s)).is_none());
        let (_, back) = desk.take_reads().pop().expect("asked to read it back");
        let read = desk.read_back(FILE, &back.rev, Ok(gives("- [x] sample")), s);
        let Ok(Some(w)) = read else {
            panic!("reads as asked");
        };
        w
    }

    #[test]
    fn whoever_asked_hears_how_the_write_went() {
        let s = settings();
        let mut desk = Edits::default();
        let answer = |desk: &mut Edits| desk.take_edited().pop().expect("answered").2;
        ticked(&mut desk, &s);
        let at = PathBuf::from("/w/Samples.csv");
        desk.wrote(FILE, Err(WriteError::Changed(at)));
        let e = answer(&mut desk).unwrap_err();
        let changed = "Samples.csv: /w/Samples.csv changed since it was read";
        assert_eq!((e.reason, e.message.as_str()), (Reason::Changed, changed));
        assert_eq!(desk.waiting(FILE), None, "the file is free again");

        let w = ticked(&mut desk, &s);
        let nothing: &[Item] = &[];
        desk.wrote(FILE, Ok(nothing));
        let e = answer(&mut desk).unwrap_err();
        let gone = format!("Samples.csv was written, but {} isn't in it now", w.item);
        assert_eq!((e.reason, e.message), (Reason::ReadBack, gone));
        desk.wrote(FILE, Ok(nothing));
        assert!(desk.take_edited().is_empty(), "nothing is being written");
    }

    #[test]
    fn an_item_edit_is_refused_when_its_file_changed_or_isnt_text() {
        let s = settings();
        let mut desk = Edits::default();
        let (changes, expect, edit) = tick(&s);
        let theirs = seen("- [ ] sample, theirs\n");
        let Err(e) = desk.edit_item(&owned(), &theirs, changes, expect, edit, &s) else {
            panic!("an edit of a file that changed since its owner read it");
        };
        let changed = "Samples.csv changed since it was read: read it again";
        assert_eq!((e.reason, e.message.as_str()), (Reason::Changed, changed));
        let bytes = [b'-', 0xff];
        let mut file = owned();
        file.rev = Some(read::rev(&bytes));
        let (changes, expect, edit) = tick(&s);
        let Err(e) = desk.edit_item(&file, &seen(bytes), changes, expect, edit, &s) else {
            panic!("an edit of a file that isn't text");
        };
        let not_text = "Samples.csv isn't UTF-8 text";
        assert_eq!((e.reason, e.message.as_str()), (Reason::Refused, not_text));
        assert!(!desk.has_news(), "nothing asked");
        assert_eq!(desk.waiting(FILE), None);
        let (changes, expect, edit) = tick(&s);
        let made = desk.edit_item(&owned(), &seen(OPEN), changes, expect, edit, &s);
        assert_eq!(made.unwrap().0, Ticket(1), "a refusal takes no ticket");
    }

    /// Tick the task in `FILE` and have its owner answer `file/edit` with `text`, then read
    /// `reads` in its new text when it made one: the desk's answer, which is a refusal.
    fn refused(
        desk: &mut Edits,
        s: &Settings,
        text: Result<&str, &str>,
        reads: Option<Result<Given, String>>,
    ) -> Error {
        let (changes, expect, edit) = tick(s);
        let made = desk.edit_item(&owned(), &seen(OPEN), changes, expect, edit, s);
        assert!(made.unwrap().1.is_none());
        let (_, ask) = desk.take_file_edits().pop().expect("asked");
        let text = text.map(String::from).map_err(String::from);
        assert!(desk.edited(OWNER, FILE, &ask.rev, text, s).is_none());
        if let Some(reading) = reads {
            let (_, back) = desk.take_reads().pop().expect("asked to read it back");
            let taken = desk.read_back(FILE, &back.rev, reading, s);
            assert!(matches!(taken, Ok(None)), "taken, nothing to write");
        }
        assert_eq!(desk.waiting(FILE), None, "the file is free again");
        let done = desk.take_edited();
        let [(by, _, Err(e))] = &done[..] else {
            panic!("{done:?}");
        };
        assert_eq!(by, "agenda");
        e.clone()
    }

    #[test]
    fn an_owner_that_refuses_or_gives_back_something_else_fails_the_edit() {
        let s = settings();
        let mut desk = Edits::default();
        let said = |e: &Error| (e.reason, e.message.clone());
        let no = refused(&mut desk, &s, Err("it is read-only"), None);
        let refuses = "csv-table didn't edit Samples.csv: it is read-only";
        assert_eq!(said(&no), (Reason::Refused, refuses.to_string()));
        let same = refused(&mut desk, &s, Ok(OPEN), None);
        let unchanged = "csv-table's new text for Samples.csv is the same as before";
        assert_eq!(said(&same), (Reason::ReadBack, unchanged.to_string()));
        let stray = Some(Err("a stray quote".to_string()));
        let unread = refused(&mut desk, &s, Ok(DONE), stray);
        let cant = "csv-table couldn't read its new text for Samples.csv: a stray quote";
        assert_eq!(said(&unread), (Reason::ReadBack, cant.to_string()));
        let untouched = Some(Ok(gives("- [ ] sample")));
        let wrong = refused(&mut desk, &s, Ok(DONE), untouched);
        assert_eq!(wrong.reason, Reason::ReadBack, "{wrong}");
        let start = "csv-table's new text for Samples.csv doesn't read back as asked (";
        assert!(wrong.message.starts_with(start), "{wrong}");
        assert!(wrong.message.ends_with("); nothing was written"), "{wrong}");
    }

    /// A relink's edit of the owned file `rel`, planned on `text`, whose one Markdown piece
    /// reads `from`, to make it read `to`.
    fn relinking(rel: &str, text: &str, from: &str, to: &str) -> rename::FileEdit {
        rename::FileEdit {
            file: rel.into(),
            path: PathBuf::from(rel),
            old_text: text.into(),
            new_text: String::new(),
            links: 1,
            changes: Vec::new(),
            owned: Some(rename::Through {
                owner: OWNER.into(),
                changes: vec![Change::Text {
                    place: "1".into(),
                    text: to.into(),
                }],
                given: Some(gives(from)),
                after: None,
            }),
        }
    }

    /// The edits of a rename of Krakatoa to Krakatau in two owned files, `A.csv` and `B.csv`.
    fn krakatau() -> [rename::FileEdit; 2] {
        let (from, to) = ("see [[Krakatoa]]", "see [[Krakatau]]");
        [
            relinking("A.csv", "[[Krakatoa]]\n", "[[Krakatoa]]", "[[Krakatau]]"),
            relinking("B.csv", "see [[Krakatoa]]\n", from, to),
        ]
    }

    #[test]
    fn a_relink_waits_for_every_owner_then_hands_back_every_new_text() {
        let s = settings();
        let mut desk = Edits::default();
        let to = "[[Krakatau]]";
        let note = rename::FileEdit {
            owned: None,
            ..relinking("Note.md", "[[Krakatoa]]\n", "[[Krakatoa]]", to)
        };
        let [a, b] = krakatau();
        let mut edits = [note, a, b];
        let ticket = desk.relink(&mut edits, |_| true, &s).unwrap();
        let ticket = ticket.expect("waits for the owners");
        let asks = desk.take_file_edits();
        let asked: Vec<&str> = asks.iter().map(|(_, a)| a.path.as_str()).collect();
        assert_eq!(asked, ["A.csv", "B.csv"], "a note is no owner's to ask");
        let waiting = Err("A.csv has an edit waiting for csv-table".to_string());
        let again = desk.relink(&mut edits, |_| true, &s);
        assert_eq!(again, waiting, "one at a time");

        let answer = |desk: &mut Edits, rel: &str, new: &str, reads: &str| {
            let (_, ask) = asks.iter().find(|(_, a)| a.path == rel).unwrap();
            let read_back = desk.edited(OWNER, rel, &ask.rev, Ok(new.into()), &s);
            assert!(read_back.is_none());
            let (_, back) = desk.take_reads().pop().expect("asked to read it back");
            let kept = desk.read_back(rel, &back.rev, Ok(gives(reads)), &s);
            assert!(matches!(kept, Ok(None)), "kept, not written");
        };
        answer(&mut desk, "B.csv", "see [[Krakatau]]\n", "see [[Krakatau]]");
        let none = desk.take_relinked();
        assert!(none.is_empty(), "A.csv's owner is still to answer");
        answer(&mut desk, "A.csv", "[[Krakatau]]\n", "[[Krakatau]]");
        let relinked = desk.take_relinked();
        let [(t, Ok(answers))] = &relinked[..] else {
            panic!("{relinked:?}");
        };
        assert_eq!(*t, ticket);
        rename::answered(&mut edits, answers.clone());
        let new: Vec<(&str, &str)> = (edits.iter())
            .map(|e| (e.file.as_str(), e.new_text.as_str()))
            .collect();
        let expected = [
            ("Note.md", ""),
            ("A.csv", "[[Krakatau]]\n"),
            ("B.csv", "see [[Krakatau]]\n"),
        ];
        assert_eq!(new, expected, "each file's new text in the plan");
        let read = |e: &rename::FileEdit| e.owned.as_ref().is_none_or(|o| o.after.is_some());
        assert!(edits.iter().all(read), "with what each owner read in it");
        let planned = edits[1].old_text.as_str();
        assert_eq!(planned, "[[Krakatoa]]\n", "the rest of the plan stays");
        assert!(desk.take_edited().is_empty() && !desk.has_news());
    }

    #[test]
    fn a_relink_is_refused_at_its_first_file_that_waits_or_changed() {
        let s = settings();
        let mut desk = Edits::default();
        let mut edits = krakatau();
        let b_changed = |e: &rename::FileEdit| e.file != "B.csv";
        let changed = Err("B.csv changed since it was planned".to_string());
        assert_eq!(desk.relink(&mut edits, b_changed, &s), changed);
        assert!(!desk.has_news(), "no owner is asked");
        let ticket = desk.relink(&mut edits, |_| true, &s).unwrap();
        assert_eq!(ticket, Some(Ticket(1)), "a refusal takes no ticket");
        let waiting = Err("A.csv has an edit waiting for csv-table".to_string());
        let refused = desk.relink(&mut edits, |_| false, &s);
        assert_eq!(refused, waiting, "said before that it changed");
    }

    #[test]
    fn an_owner_refusing_one_file_cancels_the_whole_relink() {
        let s = settings();
        let mut desk = Edits::default();
        let mut edits = krakatau();
        let ticket = desk.relink(&mut edits, |_| true, &s).unwrap().unwrap();
        let rev = |e: &rename::FileEdit| read::rev(e.old_text.as_bytes());
        let no = Err("it is read-only".to_string());
        let answered = desk.edited(OWNER, "A.csv", &rev(&edits[0]), no, &s);
        assert!(answered.is_none());
        let relinked = desk.take_relinked();
        let refuses = "csv-table didn't edit A.csv: it is read-only";
        assert!(
            matches!(&relinked[..], [(t, Err(m))] if *t == ticket && m == refuses),
            "{relinked:?}"
        );
        let asks = desk.take_file_edits();
        assert!(asks.is_empty(), "B.csv's owner isn't asked after all");
        assert_eq!(desk.waiting("B.csv"), None);
        let late = desk.edited(OWNER, "B.csv", &rev(&edits[1]), Ok("x\n".into()), &s);
        assert!(late.is_none(), "a late answer is dropped");
        assert!(!desk.has_news());
    }

    /// The engine's canvas owner answers `file/edit` and reads its new text back inside the
    /// call, so a relink of a board waits for nothing.
    #[test]
    fn an_owner_in_the_engine_answers_inside_the_call() {
        let canvas = "{\"nodes\":[{\"id\":\"c\",\"type\":\"file\",\"file\":\"Lava.md\",\
                      \"x\":0,\"y\":0,\"width\":1,\"height\":1}],\"edges\":[]}\n";
        let name = format!("cold-lava-edit-desk-canvas-{}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Lava.md"), "# Lava\n").unwrap();
        std::fs::write(dir.join("Board.canvas"), canvas).unwrap();
        let index = crate::index::Index::build(crate::workspace::scan(&dir).unwrap());
        let note = index.by_rel("Lava").unwrap();
        let mut plan = rename::plan(&index, note, "Hot/Lava").unwrap();
        let mut desk = Edits::default();
        let s = index.workspace.item_settings();
        let relinked = desk.relink(&mut plan.edits, |_| true, s);
        assert_eq!(relinked, Ok(None), "answered at once");
        assert!(!desk.has_news(), "nothing to send or hand out");
        let board = (plan.edits.iter()).find(|e| e.file == "Board.canvas");
        let board = board.expect("the board links to the note");
        assert_eq!(board.new_text, canvas.replace("Lava.md", "Hot/Lava.md"));
        assert!(board.owned.as_ref().is_some_and(|o| o.after.is_some()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
