//! The messages between the host and an extension, as data: what the host tells it
//! (`PaneNews`, `EditorRequest`, `FileNews`, `FilePreview`, `CommandRequest`) and what it
//! asks of the core (`Effect`). Every one derives serde, so an extension in a process of its
//! own gets the same messages.

use crate::view::{Content, ViewId};
use cold_lava_engine::core::Jump;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// A pane, named by the extension that opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// A program to run in a pane, in the workspace's folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// News from a pane the extension opened (host → extension).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum PaneNews {
    /// Its program wrote `ESC ] 5379 ; kind ; payload BEL`.
    #[serde(rename = "pane/message")]
    Message {
        pane: PaneId,
        kind: String,
        payload: String,
    },
    /// Its program ended, or never started (`error`).
    #[serde(rename = "pane/exited")]
    Exited { pane: PaneId, error: Option<String> },
}

/// What the core asks of the editor (host → extension). Paths are absolute, inside the workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum EditorRequest {
    #[serde(rename = "editor/open")]
    Open { note: PathBuf, jump: Option<Jump> },
    /// Write every changed note.
    #[serde(rename = "editor/save")]
    Save,
    /// The note at `from` is now at `to`, and the notes linking to it changed on disk. Until
    /// extensions can follow the core's events, a rename tells the editor this way.
    #[serde(rename = "editor/renamed")]
    Renamed { from: PathBuf, to: PathBuf },
    /// The note was moved to the trash: forget it without writing it back.
    #[serde(rename = "editor/deleted")]
    Deleted { note: PathBuf },
}

/// News about a file of a kind it owns (host → extension). Paths are workspace-relative.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum FileNews {
    /// Show it: the user chose it in the tree or the quick switcher, or followed a link to it;
    /// at `place`, the owner's place for an item or a link in it (`file/read`), when one was
    /// opened.
    #[serde(rename = "file/open")]
    Open {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<String>,
    },
    /// It changed on disk, whoever changed it (its owner too): its version now, `None` when it
    /// is gone.
    #[serde(rename = "file/changed")]
    Changed { path: String, rev: Option<String> },
}

/// `file/preview` (host → owner, a request): a small view of a file of a kind it owns, for a
/// note that embeds it (`![[Eruptions.csv]]`). `width` and `height` are the room the reader
/// has, in columns and lines; the answer is a view's content (`view::Content`), drawn there
/// and cut to `height` lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilePreview {
    pub path: String,
    pub rev: String,
    pub width: u16,
    pub height: u16,
}

/// `command/run` (host → extension): run one of the commands it registered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum CommandRequest {
    #[serde(rename = "command/run")]
    Run { id: String, args: Value },
}

/// What an extension asks of the core (extension → host).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Effect {
    /// Show `message` in the status line.
    #[serde(rename = "ui/notify")]
    Notify { message: String },
    /// The extension's own part of the status line, kept until it sends another; `None` clears.
    #[serde(rename = "ui/status")]
    Status { text: Option<String> },
    /// The editor shows `note` now, whoever asked for it.
    #[serde(rename = "editor/opened")]
    EditorOpened { note: PathBuf },
    /// Run a command by id, as a key would: the core's, such as `note.follow-link`, or an
    /// extension's.
    #[serde(rename = "command/run")]
    RunCommand { id: String, args: Value },
    /// `note` was just written: read it now instead of when the watcher notices.
    #[serde(rename = "workspace/saved")]
    Saved { note: PathBuf },
    #[serde(rename = "pane/open")]
    OpenPane { pane: PaneId, spec: PaneSpec },
    #[serde(rename = "pane/input")]
    PaneInput { pane: PaneId, text: String },
    /// Show a view, at the row `select` if it names one; the host named it `view`. `file` is
    /// the workspace file it shows, whose backlinks the menu shows under it.
    #[serde(rename = "view/open")]
    OpenView {
        view: ViewId,
        content: Content,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
    },
    /// What one of its views shows now; `select` is a row to select, now or when it shows up.
    #[serde(rename = "view/update")]
    UpdateView {
        view: ViewId,
        content: Content,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    #[serde(rename = "view/close")]
    CloseView { view: ViewId },
    /// Bring one of its open views to the front.
    #[serde(rename = "view/show")]
    ShowView { view: ViewId },
    /// The answer to `file/preview` for `path` at version `rev`, from its owner. Not a message
    /// an extension sends: its owner answers the request, and the menu keeps the view.
    #[serde(skip)]
    Previewed {
        path: String,
        rev: String,
        content: Result<Content, String>,
    },
    /// Open a note, at a line: a path inside the workspace, absolute or relative to its root
    /// as items name their notes.
    #[serde(rename = "workspace/open")]
    OpenNote {
        note: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
        /// An item's id: a file another owner reads opens at the item's place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
}
