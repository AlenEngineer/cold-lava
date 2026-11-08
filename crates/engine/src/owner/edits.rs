//! Edits of files other owners read (`file/edit`). The engine makes the change the way it edits
//! a note (a Markdown piece's new text through the Markdown owner, or a typed item's field) and
//! asks the file's owner for the file's new text. Before writing it, the engine asks the owner to
//! read that text (`file/read`) and checks it reads as asked: what the owner gave before, with the
//! change made. Only then is it written, with `write::replace`, refused if the file changed since.
//! Owners answer later, so each edit waits here, one per file, until its owner has answered both.

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
    /// Links to files a rename moved.
    Relink,
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
}

impl Edits {
    pub fn ticket(&mut self) -> Ticket {
        self.next += 1;
        Ticket(self.next)
    }

    /// Whether there is something to send or to hand out.
    pub fn has_news(&self) -> bool {
        !(self.asks.is_empty() && self.reads.is_empty() && self.done.is_empty())
    }
}
