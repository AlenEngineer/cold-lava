//! Edits of files other owners read (`file/edit`). The engine makes the change the way it edits
//! a note (a Markdown piece's new text through the Markdown owner, or a typed item's field) and
//! asks the file's owner for the file's new text. Before writing it, the engine asks the owner to
//! read that text (`file/read`) and checks it reads as asked: what the owner gave before, with the
//! change made. Only then is it written, with `write::replace`, refused if the file changed since.
//! An extension answers later, so each edit waits here, one per file, until its owner has
//! answered both; the engine's canvas owner answers both inside the call, through the same
//! steps. A rename, merge or recipe asks every owned file's owner this way before anything is
//! written, and keeps each new text that read back until all are in (`Relink`).

use crate::gateway::{Edited, Error, Ticket};
use crate::items::Item;
use crate::owner::cache::FileRead;
use crate::owner::given::{Change, Given};
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
    pub text: String,
}

/// An edit of one owned file, from asking its owner to writing what it answered.
pub(crate) struct Pending {
    pub owner: String,
    /// The file's version and text the change was made on.
    pub rev: String,
    pub text: String,
    /// What the owner should give for the file once it is written.
    pub expect: Given,
    pub goal: Goal,
    pub stage: Stage,
}

pub(crate) enum Stage {
    /// `file/edit` was asked.
    Editing,
    /// The owner answered with the file's new text, at version `rev`, and was asked to read it.
    Reading { rev: String, text: String },
}

/// What the edit is for.
pub(crate) enum Goal {
    /// An `items/edit` by `by`, answered under `ticket`.
    Item {
        by: String,
        ticket: Ticket,
        /// The item as it should read once written, and a recurring task's next occurrence.
        item: Box<Item>,
        next: Option<Box<Item>>,
        what: String,
        warning: Option<String>,
    },
    /// Links in one of the files of the rename or merge `Ticket`, whose new text is kept, not
    /// written.
    Relink(Ticket),
}

/// A rename or merge waiting for the owners of its owned files: their edits, each given its
/// new text (and what the owner read in it) as it reads back.
pub(crate) struct Relink {
    pub edits: Vec<crate::rename::FileEdit>,
}

impl Relink {
    pub fn done(&self) -> bool {
        self.edits.iter().all(is_answered)
    }

    /// The edits whose owners have answered so far.
    pub fn answered(&self) -> Vec<crate::rename::FileEdit> {
        let answered = self.edits.iter().filter(|e| is_answered(e));
        answered.cloned().collect()
    }
}

fn is_answered(e: &crate::rename::FileEdit) -> bool {
    e.owned.as_ref().is_some_and(|o| o.after.is_some())
}

/// The edits waiting for owners, and what is to be sent and handed out.
#[derive(Default)]
pub(crate) struct Edits {
    next: u64,
    /// By the file's workspace-relative path.
    pub pending: HashMap<String, Pending>,
    /// `file/edit` to send, each with its owner.
    pub asks: Vec<(String, FileEdit)>,
    /// `file/read` of an owner's new text, to send.
    pub reads: Vec<(String, FileRead)>,
    /// Answers to `items/edit`, by who asked.
    pub done: Vec<(String, Ticket, Result<Edited, Error>)>,
    /// Renames and merges waiting for owners.
    pub relinks: HashMap<Ticket, Relink>,
    /// Their answers: the owned files' edits with their new texts, or why the owner didn't.
    pub relinked: Vec<(Ticket, Result<Vec<crate::rename::FileEdit>, String>)>,
}

impl Edits {
    pub fn ticket(&mut self) -> Ticket {
        self.next += 1;
        Ticket(self.next)
    }

    /// Whether there is something to send or to hand out.
    pub fn has_news(&self) -> bool {
        !(self.asks.is_empty()
            && self.reads.is_empty()
            && self.done.is_empty()
            && self.relinked.is_empty())
    }
}
