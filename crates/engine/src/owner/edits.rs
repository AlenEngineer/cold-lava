//! The edit desk: edits of files other owners read (`file/edit`). The core makes the change the
//! way it edits a note (a Markdown piece's new text through the Markdown owner, or a typed
//! item's field), and the desk asks the file's owner for the file's new text. Before it is
//! written, the desk asks the owner to read that text (`file/read`) and checks it reads as
//! asked: what the owner gave before, with the change made. Only then does it hand the text to
//! the core (`ToWrite`), which writes it with `write::replace`, refused if the file changed
//! since. An extension answers later, so each edit waits here, one per file, until its owner
//! has answered both; the engine's canvas owner answers both inside the call, through the same
//! steps. A rename, merge or recipe asks every owned file's owner this way before anything is
//! written, and keeps each new text that read back until all are in (`Relink`). The desk
//! writes nothing and tells no subscriber: the core does both.

use crate::bounded::{Message, Text};
use crate::gateway::{Edited, Error, Reason, Ticket};
use crate::items::{Item, Settings};
use crate::owner::cache::FileRead;
use crate::owner::given::{self, Change, Given};
use crate::read;
use crate::rename;
use crate::workspace::Owned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub by: String,
    pub item: Box<Item>,
    pub next: Option<Box<Item>>,
    pub what: String,
    pub warning: Option<String>,
}

/// An item edit whose new text read back as asked, for the core to write: `after` over
/// `before`, the text the edit was made on, at the workspace-relative `path`. What `owner` read
/// in it (`given`, at version `rev`) is then the file's, and `edit` is answered under `ticket`
/// (`Edits::answer`), whether the write went ahead or not.
pub(crate) struct ToWrite {
    pub path: String,
    pub before: String,
    pub after: String,
    pub rev: String,
    pub owner: String,
    pub given: Given,
    pub ticket: Ticket,
    pub edit: ItemEdit,
}

/// An edit of one owned file, from asking its owner to handing back what it answered.
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
}

/// What the edit is for.
enum Goal {
    /// An `items/edit`, answered under the ticket.
    Item(Ticket, ItemEdit),
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
    /// a time.
    pub fn waiting(&self, path: &str) -> Option<&str> {
        self.pending.get(path).map(|p| p.owner.as_str())
    }

    /// `edit` of an item in the owned `file`, made into `changes`, after which its owner should
    /// give `expect`: refused when the file changed since its owner read it, or isn't UTF-8
    /// text. Else its owner is asked for the file's new text (`file/edit`). The ticket the
    /// answer comes under (`take_edited`), and what to write when an owner in the engine
    /// answered inside the call. No other edit of the file may be waiting (`waiting`).
    pub fn edit_item(
        &mut self,
        file: &Owned,
        changes: Vec<Change>,
        expect: Given,
        edit: ItemEdit,
        s: &Settings,
    ) -> Result<(Ticket, Option<ToWrite>), Error> {
        let error = Error::new;
        let text = std::fs::read(&file.path)
            .map_err(|e| error(Reason::Io, format!("{}: {e}", file.rel)))?;
        if file.rev.as_deref() != Some(read::rev(&text).as_str()) {
            let message = format!("{} changed since it was read: read it again", file.rel);
            return Err(error(Reason::Changed, message));
        }
        let text = String::from_utf8(text)
            .map_err(|_| error(Reason::Refused, format!("{} isn't UTF-8 text", file.rel)))?;
        let ticket = self.ticket();
        let pending = Pending {
            owner: file.owner.clone(),
            rev: read::rev(text.as_bytes()),
            text,
            expect: Some(expect),
            goal: Goal::Item(ticket, edit),
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
            Some(given) => self.read_back(&read.path, given, s),
            None => {
                self.reads.push((owner.to_string(), read));
                None
            }
        }
    }

    /// Whether what the owner gives for the file at `path` at version `rev` is its reading of
    /// an edit's new text (`read_back`), not of the file as it found it.
    pub fn reads_back(&self, path: &str, rev: &str) -> bool {
        let reading = |p: &Pending| matches!(&p.stage, Stage::Reading { rev: r, .. } if r == rev);
        self.pending.get(path).is_some_and(reading)
    }

    /// The owner read the new text of the file at `path` (`given`): when it reads as asked, an
    /// item edit's new text is handed back to be written, and a relink's is kept. Else the edit
    /// fails, and nothing is written.
    pub fn read_back(
        &mut self,
        path: &str,
        given: Result<Given, String>,
        s: &Settings,
    ) -> Option<ToWrite> {
        let p = self.pending.get(path)?;
        let Stage::Reading { text, .. } = &p.stage else {
            return None;
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
                return None;
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
            return None;
        }
        if let Goal::Relink(ticket) = p.goal {
            self.relink_read_back(ticket, path, text.clone(), given);
            return None;
        }
        let p = self.pending.remove(path)?;
        let (Stage::Reading { rev, text }, Goal::Item(ticket, edit)) = (p.stage, p.goal) else {
            return None;
        };
        Some(ToWrite {
            path: path.to_string(),
            before: p.text,
            after: text,
            rev,
            owner,
            given,
            ticket,
            edit,
        })
    }

    /// The edit of the file at `path` waiting for its owner failed: tell whoever asked.
    fn failed(&mut self, path: &str, e: Error) {
        let Some(p) = self.pending.remove(path) else {
            return;
        };
        match p.goal {
            Goal::Item(ticket, edit) => self.done.push((edit.by, ticket, Err(e))),
            Goal::Relink(ticket) => self.relink_failed(ticket, e.message),
        }
    }

    /// The answer to the item edit `by` asked for under `ticket`, once the core wrote what
    /// `read_back` handed it, or couldn't: handed out with the rest (`take_edited`).
    pub fn answer(&mut self, by: String, ticket: Ticket, answer: Result<Edited, Error>) {
        self.done.push((by, ticket, answer));
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
    /// of them changed since it was planned, has an edit waiting, its change would leave out
    /// something its owner gave (`given::check_change`: a link target past its bound), or an
    /// owner in the engine refuses.
    pub fn relink(
        &mut self,
        edits: &mut [rename::FileEdit],
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
            if std::fs::read_to_string(&e.path).ok().as_ref() != Some(&e.old_text) {
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
