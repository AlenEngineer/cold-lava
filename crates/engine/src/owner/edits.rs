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
//! new text that read back until all are in (`Relink`), then until the core takes them under the
//! ticket (`answered_into`): a frontend hears only that the ticket is ready. The desk reads and
//! writes no file and tells no subscriber: the core does.

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
/// version `rev`) is the file's, and `by`'s edit of `item` did `what`. The core hands it back
/// with how the write went (`Edits::wrote`), and the desk answers `by`. Until then the file
/// takes no other edit, so every one must reach `wrote`.
#[must_use = "the file takes no other edit until this reaches `Edits::wrote`"]
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

/// An edit of one owned file, from asking its owner until it fails, a relink's new text reads
/// back, or an item edit is written.
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

/// A rename or merge waiting for the owners of its owned files: the files still to answer, and
/// the answers in so far.
struct Relink {
    waiting: Vec<String>,
    answered: Vec<Relinked>,
}

/// One owned file's answer to a rename, merge or recipe, once it read back as asked: its new
/// text, the version that is (`rev`, the one `owner` read back), and what `owner` read in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Relinked {
    pub file: String,
    pub owner: String,
    pub rev: String,
    pub text: String,
    pub after: Given,
}

/// How a rename, merge or recipe stands once its owners were asked (`Edits::relink`).
#[derive(Debug, PartialEq)]
pub(crate) enum Answers {
    /// Every owner answered, inside the call as the engine's own do, or none had to be asked:
    /// each owned file's answer, put into the edits too.
    Now(Vec<Relinked>),
    /// An extension is still to answer, under the ticket (`Edits::take_relinked`,
    /// `Edits::answered_into`). The answers in so far are put into the edits, so they say
    /// whose are still to come.
    Later(Ticket),
}

/// Put the owners' `answers` into `edits`, by file: each file's new text, and what its owner
/// read in it.
fn put(edits: &mut [rename::FileEdit], answers: &[Relinked]) {
    for a in answers {
        let Some(e) = edits.iter_mut().find(|e| e.file == a.file) else {
            continue;
        };
        e.new_text = a.text.clone();
        if let Some(o) = e.owned.as_mut() {
            o.after = Some(a.after.clone());
        }
    }
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
    /// Those whose owners have all answered, or one refused: the answers, or the refusal, until
    /// the core takes them (`answered_into`) or the relink is cancelled (`cancel_relink`).
    ready: Vec<(Ticket, Result<Vec<Relinked>, String>)>,
    /// Their tickets, each said once (`take_relinked`) unless the core took the answer first.
    /// One cancelled once it was ready is said too, so whoever waited hears of it.
    unsaid: Vec<Ticket>,
    /// The tickets the last `take_relinked` said. Each is good until the next one: an answer
    /// not taken by then is dropped, so a ticket a frontend drops leaves nothing behind.
    said: Vec<Ticket>,
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

    /// Refused while an edit of the file at `path` waits for its owner (`waiting`).
    pub fn free(&self, path: &str) -> Result<(), Error> {
        let Some(owner) = self.waiting(path) else {
            return Ok(());
        };
        let message = format!("{path} has an edit waiting for {owner}: try again");
        Err(Error::new(Reason::Changed, message))
    }

    /// `edit` of an item in the owned `file`, made into `changes` on the file as the core read
    /// it (`before`), after which its owner should give `expect`: refused when another edit of
    /// the file waits (`free`), the file changed since its owner read it, or it isn't UTF-8
    /// text. Else its owner is asked for the file's new text (`file/edit`). The ticket the
    /// answer comes under (`take_edited`), and what to write when an owner in the engine
    /// answered inside the call.
    pub fn edit_item(
        &mut self,
        file: &Owned,
        before: &Seen,
        changes: Vec<Change>,
        expect: Given,
        edit: ItemEdit,
        s: &Settings,
    ) -> Result<(Ticket, Option<ToWrite>), Error> {
        self.free(&file.rel)?;
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
    #[must_use]
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
                let read_back = self.read_back(owner, &read.path, &read.rev, given, s);
                read_back.ok().flatten()
            }
            None => {
                self.reads.push((owner.to_string(), read));
                None
            }
        }
    }

    /// `owner` read the file at `path` at version `rev` (`given`). When that is the new text an
    /// edit of the file waits for that owner to read back, the desk takes it: an item edit's new
    /// text is handed back to be written and a relink's is kept, when it reads as asked; else
    /// the edit fails, and nothing is written. Anything else isn't the desk's, and `given` is
    /// handed back: another owner's reading, or another version, such as a read-back that came
    /// after its edit was cancelled or failed, of a text that was never written.
    pub fn read_back(
        &mut self,
        owner: &str,
        path: &str,
        rev: &str,
        given: Result<Given, String>,
        s: &Settings,
    ) -> Result<Option<ToWrite>, Result<Given, String>> {
        let reading = |p: &Pending| matches!(&p.stage, Stage::Reading { rev: r, .. } if r == rev);
        let ours = |p: &Pending| p.owner == owner && reading(p);
        let Some(p) = self.pending.get(path).filter(|p| ours(p)) else {
            return Err(given);
        };
        let Stage::Reading { text, .. } = &p.stage else {
            return Err(given);
        };
        let error = Error::new;
        let owner = owner.to_string();
        let given = match given {
            Ok(g) => g,
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
            let answer = Relinked {
                file: path.to_string(),
                owner,
                rev: rev.to_string(),
                text: text.clone(),
                after: given,
            };
            self.relink_read_back(ticket, answer);
            return Ok(None);
        }
        Ok(self.hand_over(path, owner, given))
    }

    /// The item edit of the file at `path`, whose new text read back as asked (`given`, what
    /// `owner` read in it), is handed to the core to write; it waits until the core says how
    /// that went (`wrote`).
    fn hand_over(&mut self, path: &str, owner: String, given: Given) -> Option<ToWrite> {
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

    /// The core wrote what it was handed (`w`) and read the file again, which now holds
    /// `items`, or the write failed: whoever asked for the edit is answered (`take_edited`), and
    /// the file is free for another edit.
    pub fn wrote(&mut self, w: ToWrite, written: Result<&[Item], WriteError>) {
        let path = w.path.as_str();
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
    /// it back. `Answers::Now` when there was nothing to ask, or every owner answered inside the
    /// call (the engine's own do); else `Answers::Later`, and the answer, every owned file's
    /// new text or the first owner's refusal, comes under the ticket (`answered_into`). The
    /// answers in are put into `edits`. Refused at once when one of them has an edit waiting,
    /// changed since it was planned (`planned` says whether a file still holds the text it was
    /// planned on), its change would leave out something its owner gave (`given::check_change`,
    /// such as a link to a target past its bound), or an owner in the engine refuses.
    pub fn relink(
        &mut self,
        edits: &mut [rename::FileEdit],
        planned: impl Fn(&rename::FileEdit) -> bool,
        s: &Settings,
    ) -> Result<Answers, String> {
        let asks = self.check(edits, planned, s)?;
        if asks.is_empty() {
            return Ok(Answers::Now(Vec::new()));
        }
        let ticket = self.start(asks, s);
        let Some(relink) = self.relinks.get(&ticket) else {
            return self.answered_into(ticket, edits).map(Answers::Now);
        };
        put(edits, &relink.answered);
        Ok(Answers::Later(ticket))
    }

    /// The files among `edits` that other owners read, in order, each with what its owner
    /// should give once it is changed (`None` when the owner couldn't read it before), or the
    /// first refusal (`relink`).
    fn check<'a>(
        &self,
        edits: &'a [rename::FileEdit],
        planned: impl Fn(&rename::FileEdit) -> bool,
        s: &Settings,
    ) -> Result<Vec<(&'a rename::FileEdit, Option<Given>)>, String> {
        let mut asks = Vec::new();
        for e in edits {
            let Some(o) = &e.owned else {
                continue;
            };
            if self.waiting(&e.file).is_some() {
                return Err(format!("{} has an edit waiting for {}", e.file, o.owner));
            }
            if !planned(e) {
                return Err(format!("{} changed since it was planned", e.file));
            }
            let expect = match &o.given {
                Some(g) => Some(given::check_change(&e.file, g, &o.changes, s)?),
                None => None,
            };
            asks.push((e, expect));
        }
        Ok(asks)
    }

    /// Ask the owner of each file of `asks` for its new text, all under one ticket, and wait
    /// for them (`Relink`). An owner in the engine answers inside `ask`, and its refusal ends
    /// the wait: the files after it aren't asked.
    fn start(&mut self, asks: Vec<(&rename::FileEdit, Option<Given>)>, s: &Settings) -> Ticket {
        let ticket = self.ticket();
        let waiting = asks.iter().map(|(e, _)| e.file.clone()).collect();
        let answered = Vec::new();
        self.relinks.insert(ticket, Relink { waiting, answered });
        for (e, expect) in asks {
            if !self.relinks.contains_key(&ticket) {
                break;
            }
            let Some(o) = &e.owned else {
                continue;
            };
            let pending = Pending {
                owner: o.owner.clone(),
                rev: read::rev(e.old_text.as_bytes()),
                text: e.old_text.clone(),
                expect,
                goal: Goal::Relink(ticket),
                stage: Stage::Editing,
            };
            self.ask(e.file.clone(), o.changes.clone(), pending, s);
        }
        ticket
    }

    /// The answer to the rename, merge or recipe `ticket`, once every owner has answered or one
    /// refused: each owned file's answer, put into `edits` and handed to the core to keep once
    /// the plan is written, or the refusal. It is taken once, and is then no longer said
    /// (`take_relinked`). Refused too while an owner is still to answer, once the relink is
    /// cancelled, or when a file of `edits` another owner reads has no answer: one added to
    /// the plan after its owners were asked, which no owner was asked about.
    pub fn answered_into(
        &mut self,
        ticket: Ticket,
        edits: &mut [rename::FileEdit],
    ) -> Result<Vec<Relinked>, String> {
        let not_in = || "the owners haven't all answered".to_string();
        let Some(at) = self.ready.iter().position(|(t, _)| *t == ticket) else {
            return Err(not_in());
        };
        self.unsaid.retain(|t| *t != ticket);
        let answers = self.ready.remove(at).1?;
        let answered = |e: &rename::FileEdit| answers.iter().any(|a| a.file == e.file);
        if !edits.iter().filter(|e| e.owned.is_some()).all(answered) {
            return Err(not_in());
        }
        put(edits, &answers);
        Ok(answers)
    }

    /// Renames, merges and recipes whose owners have all answered (`relink`), or one refused,
    /// each said once: its answer is for `answered_into` until the next call, which drops it,
    /// as `cancel_relink` does.
    pub fn take_relinked(&mut self) -> Vec<Ticket> {
        let said = std::mem::take(&mut self.said);
        self.ready.retain(|(t, _)| !said.contains(t));
        self.said = std::mem::take(&mut self.unsaid);
        self.said.clone()
    }

    /// Stop waiting for the owners of the rename, merge or recipe `ticket`. Their answers are
    /// dropped, the ones in already too, and what is still to be sent to them isn't sent. A
    /// read-back that comes after is no longer the desk's (`read_back`).
    pub fn cancel_relink(&mut self, ticket: Ticket) {
        self.relinks.remove(&ticket);
        let ours = |p: &Pending| matches!(p.goal, Goal::Relink(t) if t == ticket);
        self.pending.retain(|_, p| !ours(p));
        let pending = &self.pending;
        (self.asks).retain(|(_, a)| pending.contains_key(&a.path));
        (self.reads).retain(|(_, r)| pending.contains_key(&r.path));
        self.ready.retain(|(t, _)| *t != ticket);
    }

    /// One owned file of the rename or merge `ticket` read back as asked (`answer`): keep it.
    fn relink_read_back(&mut self, ticket: Ticket, answer: Relinked) {
        self.pending.remove(&answer.file);
        let Some(relink) = self.relinks.get_mut(&ticket) else {
            return;
        };
        if let Some(at) = relink.waiting.iter().position(|f| *f == answer.file) {
            relink.waiting.remove(at);
            relink.answered.push(answer);
        }
        if relink.waiting.is_empty() {
            let answered = std::mem::take(&mut relink.answered);
            self.relinks.remove(&ticket);
            self.finished(ticket, Ok(answered));
        }
    }

    /// An owner refused, or didn't answer, one file of the rename or merge `ticket`: it is
    /// cancelled, with `message`.
    fn relink_failed(&mut self, ticket: Ticket, message: String) {
        if self.relinks.contains_key(&ticket) {
            self.cancel_relink(ticket);
            self.finished(ticket, Err(message));
        }
    }

    /// The relink `ticket` is over, with `answer`: it waits to be taken (`answered_into`), and
    /// its ticket to be said (`take_relinked`).
    fn finished(&mut self, ticket: Ticket, answer: Result<Vec<Relinked>, String>) {
        self.ready.push((ticket, answer));
        self.unsaid.push(ticket);
    }

    /// Whether there is something to send or to hand out.
    pub fn has_news(&self) -> bool {
        !(self.asks.is_empty()
            && self.reads.is_empty()
            && self.done.is_empty()
            && self.unsaid.is_empty())
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
        let (changes2, expect2, edit2) = tick(&s);
        let busy = desk.edit_item(&owned(), &seen(OPEN), changes2, expect2, edit2, &s);
        let Err(e) = busy else {
            panic!("a second edit of the file while the first waits");
        };
        let waits = "Samples.csv has an edit waiting for csv-table: try again";
        assert_eq!((e.reason, e.message.as_str()), (Reason::Changed, waits));
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
        let early = desk.read_back(OWNER, FILE, &ask.rev, Ok(gives("- [ ] sample")), &s);
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
        let old = desk.read_back(OWNER, FILE, &ask.rev, Ok(gives("- [ ] sample")), &s);
        let Err(Ok(handed_back)) = old else {
            panic!("the old text is no read-back: what the owner read in it is handed back");
        };
        assert_eq!(handed_back, gives("- [ ] sample"));
        assert!((desk.edited(OWNER, FILE, &ask.rev, Ok(DONE.into()), &s)).is_none());
        assert!(desk.take_reads().is_empty(), "file/edit is answered once");

        let theirs = desk.read_back("other", FILE, &back.rev, Ok(gives("- [x] sample")), &s);
        let Err(Ok(handed_back)) = theirs else {
            panic!("another owner's reading of the new text is no read-back");
        };
        assert_eq!(handed_back, gives("- [x] sample"));

        let read = desk.read_back(OWNER, FILE, &back.rev, Ok(gives("- [x] sample")), &s);
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
        let item = w.item.clone();
        desk.wrote(w, Ok(items.as_slice()));
        assert_eq!(desk.waiting(FILE), None);
        let done = desk.take_edited();
        let [(by, t, Ok(edited))] = &done[..] else {
            panic!("{done:?}");
        };
        let found = (by.as_str(), *t, edited.item.id.as_str(), edited.changed);
        assert_eq!(found, ("agenda", ticket, item.as_str(), true));
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
        let read = desk.read_back(OWNER, FILE, &back.rev, Ok(gives("- [x] sample")), s);
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
        let w = ticked(&mut desk, &s);
        let at = PathBuf::from("/w/Samples.csv");
        desk.wrote(w, Err(WriteError::Changed(at)));
        let e = answer(&mut desk).unwrap_err();
        let changed = "Samples.csv: /w/Samples.csv changed since it was read";
        assert_eq!((e.reason, e.message.as_str()), (Reason::Changed, changed));
        assert_eq!(desk.waiting(FILE), None, "the file is free again");

        let w = ticked(&mut desk, &s);
        let nothing: &[Item] = &[];
        let item = w.item.clone();
        desk.wrote(w, Ok(nothing));
        let e = answer(&mut desk).unwrap_err();
        let gone = format!("Samples.csv was written, but {item} isn't in it now");
        assert_eq!((e.reason, e.message), (Reason::ReadBack, gone));
        let stale = ToWrite {
            path: FILE.into(),
            before: seen(DONE),
            after: DONE.into(),
            rev: read::rev(DONE.as_bytes()),
            owner: OWNER.into(),
            given: gives("- [x] sample"),
            by: "agenda".into(),
            item,
            what: "ticked".into(),
        };
        desk.wrote(stale, Ok(nothing));
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
            let taken = desk.read_back(OWNER, FILE, &back.rev, reading, s);
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
        let Ok(Answers::Later(ticket)) = desk.relink(&mut edits, |_| true, &s) else {
            panic!("waits for the owners");
        };
        let asks = desk.take_file_edits();
        let asked: Vec<&str> = asks.iter().map(|(_, a)| a.path.as_str()).collect();
        assert_eq!(asked, ["A.csv", "B.csv"], "a note is no owner's to ask");
        let waiting = Err("A.csv has an edit waiting for csv-table".to_string());
        let again = desk.relink(&mut edits, |_| true, &s);
        assert_eq!(again, waiting, "one at a time");
        let not_in = Err("the owners haven't all answered".to_string());
        let early = desk.answered_into(ticket, &mut edits);
        assert_eq!(early, not_in, "nothing to take yet");

        let answer = |desk: &mut Edits, rel: &str, new: &str, reads: &str| {
            let (_, ask) = asks.iter().find(|(_, a)| a.path == rel).unwrap();
            let read_back = desk.edited(OWNER, rel, &ask.rev, Ok(new.into()), &s);
            assert!(read_back.is_none());
            let (_, back) = desk.take_reads().pop().expect("asked to read it back");
            let kept = desk.read_back(OWNER, rel, &back.rev, Ok(gives(reads)), &s);
            assert!(matches!(kept, Ok(None)), "kept, not written");
            back.rev
        };
        let b_rev = answer(&mut desk, "B.csv", "see [[Krakatau]]\n", "see [[Krakatau]]");
        let none = desk.take_relinked();
        assert!(none.is_empty(), "A.csv's owner is still to answer");
        let a_rev = answer(&mut desk, "A.csv", "[[Krakatau]]\n", "[[Krakatau]]");
        assert_eq!(desk.take_relinked(), [ticket]);
        let answers = desk.answered_into(ticket, &mut edits).unwrap();
        assert!(desk.take_relinked().is_empty(), "said once");
        let versions: Vec<(&str, &str, &str)> = (answers.iter())
            .map(|a| (a.file.as_str(), a.owner.as_str(), a.rev.as_str()))
            .collect();
        let read_back = [
            ("B.csv", OWNER, b_rev.as_str()),
            ("A.csv", OWNER, a_rev.as_str()),
        ];
        assert_eq!(
            versions, read_back,
            "each at the version its owner read back"
        );
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
        let twice = desk.answered_into(ticket, &mut edits);
        assert_eq!(twice, not_in, "taken once");
        assert!(desk.take_edited().is_empty() && !desk.has_news());
    }

    /// A relink of `edits` whose owner refuses the first file at once, so it is over.
    fn refused_relink(desk: &mut Edits, edits: &mut [rename::FileEdit], s: &Settings) -> Ticket {
        let Ok(Answers::Later(ticket)) = desk.relink(edits, |_| true, s) else {
            panic!("waits for the owner");
        };
        let rev = read::rev(edits[0].old_text.as_bytes());
        let no = Err("it is read-only".to_string());
        assert!(desk.edited(OWNER, &edits[0].file, &rev, no, s).is_none());
        ticket
    }

    #[test]
    fn a_ticket_is_good_until_the_next_take_says_the_next_ones() {
        let s = settings();
        let mut desk = Edits::default();
        let mut edits = krakatau();
        let dropped = refused_relink(&mut desk, &mut edits, &s);
        assert_eq!(desk.take_relinked(), [dropped]);
        let kept = refused_relink(&mut desk, &mut edits, &s);
        assert_eq!(desk.take_relinked(), [kept], "each said once");
        let not_in = Err("the owners haven't all answered".to_string());
        let late = desk.answered_into(dropped, &mut edits);
        assert_eq!(late, not_in, "dropped by the take after it was said");
        let refuses = Err("csv-table didn't edit A.csv: it is read-only".to_string());
        let in_time = desk.answered_into(kept, &mut edits);
        assert_eq!(in_time, refuses, "taken before the next take");
        assert!(desk.take_relinked().is_empty() && !desk.has_news());
    }

    #[test]
    fn a_file_another_owner_reads_that_its_owner_wasnt_asked_about_is_refused() {
        let s = settings();
        let mut desk = Edits::default();
        let [a, b] = krakatau();
        let mut asked = [a];
        let Ok(Answers::Later(ticket)) = desk.relink(&mut asked, |_| true, &s) else {
            panic!("waits for the owner");
        };
        let (_, ask) = desk.take_file_edits().pop().expect("asked for A.csv");
        let new = Ok("[[Krakatau]]\n".into());
        assert!(desk.edited(OWNER, "A.csv", &ask.rev, new, &s).is_none());
        let (_, back) = desk.take_reads().pop().expect("asked to read it back");
        let reads = Ok(gives("[[Krakatau]]"));
        let kept = desk.read_back(OWNER, "A.csv", &back.rev, reads, &s);
        assert!(matches!(kept, Ok(None)), "kept, not written");
        assert_eq!(desk.take_relinked(), [ticket]);

        let mut added = b;
        added.new_text = "see [[Krakatau]]\n".into();
        let through = added.owned.as_mut().unwrap();
        through.after = Some(gives("see [[Krakatau]]"));
        let mut edits = [asked[0].clone(), added];
        let not_in = Err("the owners haven't all answered".to_string());
        let made = desk.answered_into(ticket, &mut edits);
        assert_eq!(made, not_in, "B.csv's owner was never asked");
        assert_eq!(edits[0].new_text, "", "nothing is put into the plan");
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
        let ticket = desk.relink(&mut edits, |_| true, &s);
        let first = Ok(Answers::Later(Ticket(1)));
        assert_eq!(ticket, first, "a refusal takes no ticket");
        let waiting = Err("A.csv has an edit waiting for csv-table".to_string());
        let refused = desk.relink(&mut edits, |_| false, &s);
        assert_eq!(refused, waiting, "said before that it changed");
    }

    #[test]
    fn an_owner_refusing_one_file_cancels_the_whole_relink() {
        let s = settings();
        let mut desk = Edits::default();
        let mut edits = krakatau();
        let Ok(Answers::Later(ticket)) = desk.relink(&mut edits, |_| true, &s) else {
            panic!("waits for the owner");
        };
        let rev = |e: &rename::FileEdit| read::rev(e.old_text.as_bytes());
        let no = Err("it is read-only".to_string());
        let answered = desk.edited(OWNER, "A.csv", &rev(&edits[0]), no, &s);
        assert!(answered.is_none());
        let refuses = Err("csv-table didn't edit A.csv: it is read-only".to_string());
        let refused = desk.answered_into(ticket, &mut edits);
        assert_eq!(refused, refuses, "refused under its ticket");
        assert!(desk.take_relinked().is_empty(), "taken once");
        let asks = desk.take_file_edits();
        assert!(asks.is_empty(), "B.csv's owner isn't asked after all");
        assert_eq!(desk.waiting("B.csv"), None);
        let late = desk.edited(OWNER, "B.csv", &rev(&edits[1]), Ok("x\n".into()), &s);
        assert!(late.is_none(), "a late answer is dropped");
        assert!(!desk.has_news());
    }

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
        let Ok(Answers::Now(answers)) = desk.relink(&mut plan.edits, |_| true, s) else {
            panic!("answered at once");
        };
        assert!(!desk.has_news(), "nothing to send or hand out");
        let board = (plan.edits.iter()).find(|e| e.file == "Board.canvas");
        let board = board.expect("the board links to the note");
        assert_eq!(board.new_text, canvas.replace("Lava.md", "Hot/Lava.md"));
        assert!(board.owned.as_ref().is_some_and(|o| o.after.is_some()));
        let [answer] = &answers[..] else {
            panic!("{answers:?}");
        };
        let read_back = (
            answer.file.as_str(),
            answer.owner.as_str(),
            answer.rev.clone(),
        );
        let new = read::rev(board.new_text.as_bytes());
        assert_eq!(
            read_back,
            ("Board.canvas", "canvas", new),
            "the version it read back"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
