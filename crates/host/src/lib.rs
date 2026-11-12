//! The extension host: what runs cold-lava's extensions and guards the workspace gateway, the
//! same for every frontend (the terminal UI, the GUI, a language server). It has no user
//! interface and must not depend on ratatui, crossterm, portable-pty or vt100; frontends depend
//! on it, and it depends on the engine, never the other way.
//!
//! This is the JSON-RPC protocol of docs/extensions.md, called in-process: each `Extension`
//! method is one host→extension method, each `Host` method one extension→host method, and every
//! type that crosses derives serde, so extensions in other processes share this code path.
//! Extensions never see keystrokes and never draw: the frontend owns the panes they open. What an
//! extension asks for is queued as `Effect`s, which the frontend applies after the call returns.
//! Notes as data are the exception: `items/*`, `types/list` and the file messages
//! (`workspace/read`, …) are answered during the call by the core (the engine's `gateway` and
//! `files`), within the permissions the extension's manifest asks for. An extension can
//! own a kind of file (`FileType`): the frontend hands it those files (`file/open`) and their
//! outside changes (`file/changed`), and asks it what each says as items and links (`file/read`).
//! Extensions installed on disk (`installed`) run in processes of their own (`process`), over a
//! `transport` that frames JSON-RPC messages (`frame`, `rpc`); they reach the same `Host`.
//!
//! What a frontend keeps for itself: its built-in extensions (the terminal's Neovim pane, its
//! agenda, table and graph), applying `Effect`s, its command table and keys, and drawing views
//! (the view gateway's messages are `cold-lava-views`' `view`, named here as `view`).

pub mod frame;
pub mod installed;
pub mod process;
pub mod rpc;
pub mod transport;

pub use cold_lava_views::view;

use cold_lava_engine::act::{self, Ask, Reply};
use cold_lava_engine::core::{Core, Jump};
use cold_lava_engine::files::{
    self, Create, Created, Deleted, Folder, Listed, Restore, Restored, Stat, Text, Written,
};
use cold_lava_engine::gateway::{
    self, Answer, Edit, Edited, Found, News, Query, Reason, Select, SubId, Ticket,
};
use cold_lava_engine::owner::cache::{FileRead, Kind};
use cold_lava_engine::owner::edits::FileEdit;
use cold_lava_engine::owner::given::Given;
use cold_lava_engine::types::Type;
#[cfg(test)]
use cold_lava_engine::{bind, shapes, tasks};
use cold_lava_engine::{links, matcher, owner, paths, recipe, style, types};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use view::{Content, ViewId, ViewNews};

/// A loaded extension. Only the host uses it: on the wire, the connection says who is talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtId(pub usize);

/// A pane, named by the extension that opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// Who an extension is, from `initialize`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Its id, for messages about it: "nvim".
    pub name: String,
    /// Its name for people: "Neovim".
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// One line on what it does.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub roles: Vec<Role>,
    /// Keys its program understands, as (key, what it does), for the status line and the help.
    pub hints: Vec<(String, String)>,
    /// What it adds to the menu's commands, for keys, the palette, the help and config.toml.
    pub commands: Vec<CommandSpec>,
    /// Types it defines, each with its id: `<extension name>.<type>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<Spec>,
    /// Shapes it defines, each with its id: `<extension name>.<shape>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Spec>,
    /// What it may ask of the workspace gateway. Without the field, read only.
    #[serde(default = "read_only")]
    pub permissions: Vec<Permission>,
    /// Kinds of file it opens, by extension.
    #[serde(default, rename = "file-types", skip_serializing_if = "Vec::is_empty")]
    pub file_types: Vec<FileType>,
    /// Notes it shows instead of the reader.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shows: Vec<Shows>,
}

/// A kind of file an extension owns: the menu hands it every file whose name ends in
/// `.<extension>` (`file/open`) and tells it when one changes (`file/changed`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileType {
    /// Lowercase, without the dot: "csv".
    pub extension: String,
    /// For people: "CSV table".
    pub title: String,
    /// One of its commands, which makes a new file of this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

/// Notes an extension shows instead of the reader (`[[shows]]`): those that meet `where` as a
/// note shape's `where` matches a note (frontmatter properties, tags, folder). Opening one hands
/// it to the extension (`file/open`); Markdown still reads its items and links and writes its
/// edits, so the extension shows the note, it doesn't own it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shows {
    /// For people: "Kanban board".
    pub title: String,
    #[serde(rename = "where")]
    pub conditions: crate::types::Where,
}

impl Shows {
    /// Why it can't be a claim: it must name a note's conditions, and only those.
    pub fn problem(&self) -> Option<&'static str> {
        let w = &self.conditions;
        let block = !(w.header.is_empty()
            && w.section.is_empty()
            && w.language.is_empty()
            && w.inside.is_empty()
            && w.file_type.is_empty());
        if block {
            Some("its where may only name property, tag and folder")
        } else if w.is_empty() {
            Some("its where must name a property, a tag or a folder: it can't show every note")
        } else {
            None
        }
    }

    /// Its conditions as people read them: `kanban-plugin: basic, #board, in Projects`.
    pub fn describe(&self) -> String {
        let w = &self.conditions;
        let props = w.property.iter().map(|(k, v)| format!("{k}: {v}"));
        let tags = (w.tag.iter()).map(|t| format!("#{}", t.trim_start_matches('#')));
        let folders = w.folder.iter().map(|f| format!("in {f}"));
        props
            .chain(tags)
            .chain(folders)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Whether an extension may claim files with extension `x`: not notes, the engine's own, nor a
/// kind one of the engine's built-in owners reads (`owner::BUILT_IN`: canvases).
fn claimable(x: &str) -> bool {
    x != "md" && !crate::owner::BUILT_IN.iter().any(|(b, _)| *b == x)
}

/// A part of the workspace gateway an extension may use, named in its manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// `items/query`, `items/subscribe`, `items/unsubscribe`, `types/list`.
    #[serde(rename = "items.read")]
    ItemsRead,
    /// `items/edit`.
    #[serde(rename = "items.edit")]
    ItemsEdit,
    /// `workspace/read` and `workspace/files` for kinds of file it doesn't own.
    #[serde(rename = "files.read")]
    FilesRead,
    /// `workspace/write` and `workspace/create` for kinds of file it doesn't own, and
    /// `workspace/create-folder`.
    #[serde(rename = "files.write")]
    FilesWrite,
    /// `workspace/delete` and `workspace/restore`, of any file or folder. `files.write` doesn't
    /// include it, and owning a kind of file doesn't either.
    #[serde(rename = "files.delete")]
    FilesDelete,
}

impl Permission {
    fn name(self) -> &'static str {
        match self {
            Permission::ItemsRead => "items.read",
            Permission::ItemsEdit => "items.edit",
            Permission::FilesRead => "files.read",
            Permission::FilesWrite => "files.write",
            Permission::FilesDelete => "files.delete",
        }
    }
}

pub(crate) fn read_only() -> Vec<Permission> {
    vec![Permission::ItemsRead]
}

/// The reader, a screen of the menu's built the way an extension will be: it reaches notes as
/// data only through `Host`, under this name (`Registry::screen`), which edits are attributed to.
pub const READER: &str = "reader";
/// The menu itself, when it deletes, restores or makes a folder: it asks the core as this
/// client, with no permission to check, since it is the user.
pub const MENU: &str = "menu";
/// The built-in extensions' names, in the frontends that have them: the agenda, the table,
/// the graph and Neovim as the editor. An installed extension can't take one
/// (`installed::load`).
pub const AGENDA: &str = "agenda";
pub const TABLE: &str = "table";
pub const GRAPH: &str = "graph";
pub const NVIM: &str = "nvim";
const SCREEN_PERMISSIONS: &[Permission] = &[Permission::ItemsRead, Permission::ItemsEdit];

/// A command an extension registers. Its id is `<extension name>.<something>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    /// Its default keys, written as in config.toml: "g", "ctrl+o".
    #[serde(default)]
    pub keys: Vec<String>,
}

/// A type or a shape an extension defines (`[[types]]`, `[[shapes]]`): its id,
/// `<extension name>.<name>`, and the rest of its table, a definition in the same language as
/// anyone's (docs/extensions.md, "Types"; docs/shapes.md, "Shape definitions"). The engine reads
/// the definition, so a broken one leaves out that type or shape, not the extension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    #[serde(flatten)]
    pub definition: toml::Table,
}

/// A part of cold-lava an extension provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Opens notes for editing: gets the `editor/*` requests.
    Editor,
}

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

/// An extension: built into cold-lava, or a `process::Process` speaking for one in its own
/// process. Its methods run on the UI thread: they must return at once and never wait for a pane
/// or a process.
pub trait Extension {
    /// `initialize`: who it is.
    fn initialize(&mut self) -> Manifest;
    /// `pane/message`, `pane/exited`: news from a pane it opened.
    fn pane(&mut self, news: PaneNews, host: &mut Host<'_>);
    /// `editor/open`, `editor/save`, `editor/renamed`, for the extension in the editor role.
    fn editor(&mut self, request: EditorRequest, host: &mut Host<'_>);
    /// `command/run`: one of its commands, from a key, the palette or an extension.
    fn command(&mut self, _request: CommandRequest, _host: &mut Host<'_>) {}
    /// `items/changed`, `items/reset`: news for a subscription it made.
    fn items(&mut self, _news: News, _host: &mut Host<'_>) {}
    /// `view/event`, `view/closed`: what the user did in a view it opened.
    fn view(&mut self, _news: ViewNews, _host: &mut Host<'_>) {}
    /// `file/open`, `file/changed`: a file of a kind it owns.
    fn file(&mut self, _news: FileNews, _host: &mut Host<'_>) {}
    /// `file/read`: the items and links of a file of a kind it owns, answered now or later
    /// with `Host::file_given`.
    fn read_file(&mut self, read: FileRead, host: &mut Host<'_>) {
        let why = "it doesn't read files".to_string();
        host.file_given(&read.path, &read.rev, Err(why));
    }
    /// `file/edit`: the new text of a file of a kind it owns with some changes made, answered
    /// now or later with `Host::file_edited`.
    fn edit_file(&mut self, edit: FileEdit, host: &mut Host<'_>) {
        let why = "it doesn't edit files".to_string();
        host.file_edited(&edit.path, &edit.rev, Err(why));
    }
    /// `file/preview`: a small view of a file of a kind it owns, answered now or later with
    /// `Host::file_previewed`.
    fn preview_file(&mut self, preview: FilePreview, host: &mut Host<'_>) {
        let why = "it doesn't preview files".to_string();
        host.file_previewed(&preview.path, &preview.rev, Err(why));
    }
    /// `shape/region`, `shape/gesture`: what a region of a shape it ships shows, or what a
    /// gesture on one does, answered now or later with `Host::shape_answered`.
    fn shape(&mut self, ask: Ask, host: &mut Host<'_>) {
        host.shape_answered(&ask, Err("it doesn't answer for shapes".into()));
    }
    /// The answer to an `items/edit` that `Host::edit_item` answered `Later`: an edit of a file
    /// another owner reads, now written, or why not.
    fn edited(
        &mut self,
        _ticket: Ticket,
        _answer: Result<Edited, gateway::Error>,
        _host: &mut Host<'_>,
    ) {
    }
    /// The menu opened a workspace (`None`: it has none open). An extension in a process starts
    /// here, with `initialize`, and stops when the workspace closes.
    fn workspace(&mut self, _root: Option<&Path>, _host: &mut Host<'_>) {}
    /// Once a frame: answer what it asked since the last one.
    fn poll(&mut self, _host: &mut Host<'_>) {}
    /// Whether its commands can run now.
    fn running(&self) -> bool {
        true
    }
    /// Why it stopped, when it did.
    fn stopped(&self) -> Option<&str> {
        None
    }
    /// cold-lava is quitting.
    fn shutdown(&mut self) {}
}

/// What an extension can ask of the core during a call. Effects happen once the call returns;
/// the workspace gateway's `items/*` and `types/list` are answered at once.
pub struct Host<'a> {
    effects: Vec<Effect>,
    next_pane: &'a mut u32,
    next_view: &'a mut u32,
    /// Who is asking: the name in its manifest, which edits are attributed to.
    name: &'a str,
    permissions: &'a [Permission],
    /// Which loaded extension is asking (`None`: a screen of the menu), and who owns which
    /// kind of file, for the file messages.
    me: Option<ExtId>,
    owners: &'a [(String, ExtId)],
    core: Option<&'a mut Core>,
}

impl Host<'_> {
    /// `ui/notify`
    pub fn notify(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.effects.push(Effect::Notify { message });
    }

    /// `ui/status`
    pub fn status(&mut self, text: Option<String>) {
        self.effects.push(Effect::Status { text });
    }

    /// `editor/opened`
    pub fn editor_opened(&mut self, note: PathBuf) {
        self.effects.push(Effect::EditorOpened { note });
    }

    /// `command/run`
    pub fn run_command(&mut self, id: &str, args: Value) {
        let id = id.to_string();
        self.effects.push(Effect::RunCommand { id, args });
    }

    /// `workspace/saved`
    pub fn saved(&mut self, note: PathBuf) {
        self.effects.push(Effect::Saved { note });
    }

    /// `pane/open`. The program starts once the call returns; if it can't, the extension gets
    /// `PaneNews::Exited` with the error.
    pub fn open_pane(&mut self, spec: PaneSpec) -> PaneId {
        *self.next_pane += 1;
        let pane = PaneId(*self.next_pane);
        self.effects.push(Effect::OpenPane { pane, spec });
        pane
    }

    /// `pane/input`
    pub fn pane_input(&mut self, pane: PaneId, text: impl Into<String>) {
        let text = text.into();
        self.effects.push(Effect::PaneInput { pane, text });
    }

    /// `view/open`: the view shows once the call returns, in place of what showed, which Back
    /// returns to, at the row `select` names, with the backlinks of `file` (workspace-relative)
    /// under it when it shows one. Refused when it is over the limits (`view::MAX_ROWS`, …).
    pub fn open_view(
        &mut self,
        content: Content,
        select: Option<String>,
        file: Option<String>,
    ) -> Result<ViewId, gateway::Error> {
        content
            .check()
            .map_err(|why| gateway::Error::new(Reason::Refused, why))?;
        *self.next_view += 1;
        let view = ViewId(*self.next_view);
        self.effects.push(Effect::OpenView {
            view,
            content,
            select,
            file,
        });
        Ok(view)
    }

    /// `view/update`: one of its views shows `content` now, keeping the selection (or
    /// selecting `select`), the folds, the sort and the filter by id. Updates in one frame
    /// draw once, the last.
    pub fn update_view(
        &mut self,
        view: ViewId,
        content: Content,
        select: Option<String>,
    ) -> Result<(), gateway::Error> {
        content
            .check()
            .map_err(|why| gateway::Error::new(Reason::Refused, why))?;
        (self.effects).push(Effect::UpdateView {
            view,
            content,
            select,
        });
        Ok(())
    }

    /// `view/close`
    pub fn close_view(&mut self, view: ViewId) {
        self.effects.push(Effect::CloseView { view });
    }

    /// `view/show`: one of its open views shows in place of what showed, which Back returns
    /// to, as when it opened. A view that isn't open, or is in front already, stays as it is.
    pub fn show_view(&mut self, view: ViewId) {
        self.effects.push(Effect::ShowView { view });
    }

    /// `workspace/open`: show a note, at a line; or the file of the item `item` at its place.
    pub fn open_note(&mut self, note: PathBuf, line: Option<usize>, item: Option<String>) {
        self.effects.push(Effect::OpenNote { note, line, item });
    }

    /// The answer to `file/read` for a file of a kind it owns: its items and links join the
    /// workspace, or why it couldn't read it. Only the file's owner may answer.
    pub fn file_given(&mut self, path: &str, rev: &str, given: Result<Given, String>) {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == files::extension_of(path).as_ref() && Some(*owner) == me);
        let name = self.name;
        if let (true, Some(core)) = (owns, self.core.as_deref_mut()) {
            core.file_given(name, path, rev, given);
        }
    }

    /// The answer to `file/preview` for a file of a kind it owns: what to draw where a note
    /// embeds it, or why not. Only the file's owner may answer; a view over the limits, or
    /// with more than `view::PREVIEW_ROWS` rows, is refused.
    pub fn file_previewed(&mut self, path: &str, rev: &str, content: Result<Content, String>) {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == files::extension_of(path).as_ref() && Some(*owner) == me);
        if !owns {
            return;
        }
        let content = content.and_then(|c| match c.check().and_then(|()| c.small()) {
            Ok(()) => Ok(c),
            Err(why) => Err(why),
        });
        let (path, rev) = (path.to_string(), rev.to_string());
        self.effects.push(Effect::Previewed { path, rev, content });
    }

    /// The answer to `file/edit` for a file of a kind it owns: the file's new text, or why not.
    /// Only the file's owner may answer.
    pub fn file_edited(&mut self, path: &str, rev: &str, text: Result<String, String>) {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == files::extension_of(path).as_ref() && Some(*owner) == me);
        let name = self.name;
        if let (true, Some(core)) = (owns, self.core.as_deref_mut()) {
            core.file_edited(name, path, rev, text);
        }
    }

    /// The answer to `shape/region` or `shape/gesture` about a shape it ships: the core keeps
    /// it for the frontend's bindings (`Core::take_shape_answers`).
    pub fn shape_answered(&mut self, ask: &Ask, answer: Result<Reply, String>) {
        let name = self.name;
        if let Some(core) = self.core.as_deref_mut() {
            core.shape_answered(name, ask, answer);
        }
    }

    /// A recipe on the whole workspace, for a step of a shape it ships: it rewrites files, so it
    /// needs `files.write`.
    pub fn run_recipe(
        &mut self,
        id: &str,
        options: crate::recipe::Options,
    ) -> Result<String, gateway::Error> {
        self.core(Permission::FilesWrite)?.run_recipe(id, options)
    }

    /// Nothing, when the caller's manifest asks for `needs`; else the refusal, naming it.
    fn may(&self, needs: Permission) -> Result<(), gateway::Error> {
        if self.permissions.contains(&needs) {
            return Ok(());
        }
        let message = format!(
            "{} may not do this: its manifest's permissions don't include {}",
            self.name,
            needs.name()
        );
        Err(gateway::Error::new(Reason::Forbidden, message))
    }

    /// The core, when the caller's manifest asks for `needs`.
    fn core(&mut self, needs: Permission) -> Result<&mut Core, gateway::Error> {
        self.may(needs)?;
        (self.core.as_deref_mut())
            .ok_or_else(|| gateway::Error::new(Reason::NoWorkspace, "no workspace is open"))
    }

    /// `items/query`
    pub fn query_items(&mut self, query: &Query) -> Result<Found, gateway::Error> {
        self.core(Permission::ItemsRead)?.query_items(query)
    }

    /// `items/subscribe`: the extension's `items` hears `items/changed` and `items/reset` for it
    /// from now on.
    pub fn subscribe_items(&mut self, select: Select) -> Result<SubId, gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsRead)?
            .subscribe_items(name, select)
    }

    /// `items/unsubscribe`, for one of its own subscriptions.
    pub fn unsubscribe_items(&mut self, sub: SubId) -> Result<(), gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsRead)?
            .unsubscribe_items(name, sub);
        Ok(())
    }

    /// `items/edit` through the core's safe edits, attributed to the caller: made at once in a
    /// note; in a file another owner reads, answered `Later`, through `Extension::edited`.
    pub fn edit_item(&mut self, edit: &Edit) -> Result<Answer, gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsEdit)?.edit_item(name, edit)
    }

    /// `links/query`
    pub fn query_links(&mut self) -> Result<crate::links::LinkGraph, gateway::Error> {
        Ok(self.core(Permission::ItemsRead)?.query_links())
    }

    /// `links/subscribe`: the extension's `items` hears `links/changed` for it from now on.
    pub fn subscribe_links(&mut self) -> Result<SubId, gateway::Error> {
        let name = self.name;
        Ok(self.core(Permission::ItemsRead)?.subscribe_links(name))
    }

    /// `links/unsubscribe`, for one of its own subscriptions.
    pub fn unsubscribe_links(&mut self, sub: SubId) -> Result<(), gateway::Error> {
        self.unsubscribe_items(sub)
    }

    /// `types/list`
    pub fn types(&mut self) -> Result<Vec<Type>, gateway::Error> {
        let core = self.core(Permission::ItemsRead)?;
        Ok(core.index.workspace.item_settings().types.types.clone())
    }

    /// `workspace/style`
    pub fn style(&mut self) -> Result<crate::style::Style, gateway::Error> {
        let core = self.core(Permission::ItemsRead)?;
        Ok(core.index.workspace.style())
    }

    /// The core, for a file message about files of `extension`: the owner of that kind may
    /// read and write them; any other file needs `files.read`, and `files.write` to change it.
    fn files_core(
        &mut self,
        extension: Option<String>,
        writes: bool,
    ) -> Result<&mut Core, gateway::Error> {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == extension.as_ref() && Some(*owner) == me);
        match (owns, writes) {
            (true, _) => self
                .core
                .as_deref_mut()
                .ok_or_else(|| gateway::Error::new(Reason::NoWorkspace, "no workspace is open")),
            (false, false) => self.core(Permission::FilesRead),
            (false, true) => self.core(Permission::FilesWrite),
        }
    }

    /// `workspace/read`
    pub fn read_file(&mut self, path: &str) -> Result<Text, gateway::Error> {
        self.files_core(files::extension_of(path), false)?
            .read_file(path)
    }

    /// `workspace/write`: refused unless the file is still the version `expect`.
    pub fn write_file(
        &mut self,
        path: &str,
        text: &str,
        expect: &str,
    ) -> Result<Written, gateway::Error> {
        self.files_core(files::extension_of(path), true)?
            .write_file(path, text, expect)
    }

    /// `workspace/create`
    pub fn create_file(&mut self, create: &Create) -> Result<Created, gateway::Error> {
        self.files_core(create.extension(), true)?
            .create_file(create)
    }

    /// `workspace/files`
    pub fn list_files(&mut self, extension: &str) -> Result<Listed, gateway::Error> {
        let kind = extension.trim_start_matches('.').to_ascii_lowercase();
        Ok(self.files_core(Some(kind), false)?.files(extension))
    }

    /// `workspace/stat`: a read, so the owner of the file's kind or `files.read`. A folder
    /// answers with what is in it, so it needs `files.read` whatever it is called.
    pub fn stat_file(&mut self, path: &str) -> Result<Stat, gateway::Error> {
        let core = self.files_core(files::extension_of(path), false)?;
        let stat = core.stat_file(path)?;
        if stat.inside.is_some() {
            self.may(Permission::FilesRead)?;
        }
        Ok(stat)
    }

    /// `workspace/delete`, to the trash and attributed to the caller: it needs `files.delete`,
    /// whatever kind of file it is.
    pub fn delete_file(&mut self, path: &str, expect: &str) -> Result<Deleted, gateway::Error> {
        let name = self.name;
        self.core(Permission::FilesDelete)?
            .delete_file(name, path, expect)
    }

    /// `workspace/restore`, under `files.delete` too: whoever may delete may put back what it
    /// deleted. Putting it somewhere else (`to`) makes a file there, so it needs `files.write`
    /// as well.
    pub fn restore_file(&mut self, restore: &Restore) -> Result<Restored, gateway::Error> {
        if restore.to.is_some() {
            self.may(Permission::FilesWrite)?;
        }
        let name = self.name;
        self.core(Permission::FilesDelete)?
            .restore_file(name, restore)
    }

    /// `workspace/create-folder`: a folder has no kind, so it needs `files.write`.
    pub fn create_folder(&mut self, path: &str) -> Result<Folder, gateway::Error> {
        let name = self.name;
        self.core(Permission::FilesWrite)?.create_folder(name, path)
    }
}

/// A shape's steps run through the `Host` of the extension that ships it: its permissions, its
/// name on the edits.
impl act::Gateway for Host<'_> {
    fn edit_item(&mut self, edit: &Edit) -> Result<Answer, gateway::Error> {
        Host::edit_item(self, edit)
    }

    fn create_file(&mut self, create: &Create) -> Result<Created, gateway::Error> {
        Host::create_file(self, create)
    }

    fn run_recipe(
        &mut self,
        id: &str,
        options: crate::recipe::Options,
    ) -> Result<String, gateway::Error> {
        Host::run_recipe(self, id, options)
    }
}

/// The loaded extensions and their manifests, who owns which kind of file and who shows
/// which notes. A frontend keeps one, calls extensions through it (`call`) and applies the
/// `Effect`s that come back; its command table is its own (`refuse_commands`).
pub struct Registry {
    loaded: Vec<(Box<dyn Extension>, Manifest)>,
    next_pane: u32,
    next_view: u32,
    /// Why types, shapes, commands, keys, file types or claims the extensions registered were
    /// left out.
    refused: Vec<String>,
    /// Where in `refused` the frontend's word on commands goes: after the types and shapes,
    /// before the file types and the claims.
    commands_at: usize,
    /// Extensions on disk that didn't load, and why.
    mistakes: Vec<String>,
    /// Whether any extension runs in a process, so the menu polls more often.
    processes: bool,
    /// Who opens each kind of file, by extension: the first to claim it.
    owners: Vec<(String, ExtId)>,
    /// The claims to show notes that hold, in load order: the extension and its claim's index.
    shows: Vec<(ExtId, usize)>,
}

/// Whether `id` is in the extension `name`'s namespace: `<name>.<something>`. Its commands'
/// and its types' ids must be.
pub fn namespaced(name: &str, id: &str) -> bool {
    (id.strip_prefix(name))
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|rest| !rest.is_empty())
}

/// Leaves out of `m` the types and shapes it can't define, as commands are: an id outside its
/// namespace (`<name>.<type>`), or one it gave already. Returns why, one line each.
fn leave_out_definitions(m: &mut Manifest) -> Vec<String> {
    let name = m.name.clone();
    let mut why = leave_out(&name, "type", &mut m.types);
    why.extend(leave_out(&name, "shape", &mut m.shapes));
    why
}

fn leave_out(name: &str, what: &str, specs: &mut Vec<Spec>) -> Vec<String> {
    let mut why = Vec::new();
    let mut ids = HashSet::new();
    specs.retain(|t| {
        let id = &t.id;
        let problem = if !namespaced(name, id) {
            format!("its id must start with \"{name}.\"")
        } else if !ids.insert(id.clone()) {
            "that id is taken".into()
        } else {
            return true;
        };
        why.push(format!("{name}: {what} {id:?} was left out: {problem}"));
        false
    });
    why
}

impl Registry {
    /// The frontend's built-in extensions, then the ones installed in `dir`
    /// (`installed::user_dir` for the user's), each in a process of its own.
    pub fn load(built_in: Vec<Box<dyn Extension>>, dir: Option<&Path>) -> Registry {
        let mut found = built_in;
        let (on_disk, mistakes) = dir.map(installed::load).unwrap_or_default();
        let processes = !on_disk.is_empty();
        for ext in on_disk {
            found.extend(process::Process::new(ext).map(|p| Box::new(p) as Box<dyn Extension>));
        }
        let mut r = Registry::new(found);
        r.mistakes = mistakes;
        r.processes = processes;
        r
    }

    pub fn new(extensions: Vec<Box<dyn Extension>>) -> Registry {
        let mut loaded: Vec<_> = extensions
            .into_iter()
            .map(|mut e| {
                let manifest = e.initialize();
                (e, manifest)
            })
            .collect();
        let mut refused = Vec::new();
        for (_, m) in &mut loaded {
            refused.extend(leave_out_definitions(m));
        }
        let commands_at = refused.len();
        let mut owners: Vec<(String, ExtId)> = Vec::new();
        for (i, (_, m)) in loaded.iter().enumerate() {
            for t in &m.file_types {
                let x = &t.extension;
                if !claimable(x) {
                    let why = format!("{} can't open .{x} files: cold-lava does", m.name);
                    refused.push(why);
                } else if let Some((_, first)) = owners.iter().find(|(o, _)| o == x) {
                    let first = &loaded[first.0].1.name;
                    let why = format!("{} and {first} both open .{x} files: {first} does", m.name);
                    refused.push(why);
                } else {
                    owners.push((x.clone(), ExtId(i)));
                }
            }
        }
        let mut shows: Vec<(ExtId, usize)> = Vec::new();
        for (i, (_, m)) in loaded.iter().enumerate() {
            for (n, claim) in m.shows.iter().enumerate() {
                let what = claim.describe();
                let same = |&&(e, k): &&(ExtId, usize)| {
                    loaded[e.0].1.shows[k].conditions == claim.conditions
                };
                if let Some(problem) = claim.problem() {
                    refused.push(format!(
                        "{}: shows {what:?} was left out: {problem}",
                        m.name
                    ));
                } else if let Some((first, _)) = shows.iter().find(same) {
                    let first = &loaded[first.0].1.name;
                    let why = format!(
                        "{} and {first} both show notes with {what}: {first} does",
                        m.name
                    );
                    refused.push(why);
                } else {
                    shows.push((ExtId(i), n));
                }
            }
        }
        Registry {
            loaded,
            next_pane: 0,
            next_view: 0,
            refused,
            commands_at,
            mistakes: Vec::new(),
            processes: false,
            owners,
            shows,
        }
    }

    /// The extension that shows the note at `path` (workspace-relative, with `.md`, its text
    /// and its tags) instead of the reader, and its claim: of the claims the note meets, the
    /// most specific (the most conditions, as types' shapes are ranked); of equals, the first
    /// loaded.
    pub fn shower_of(&self, path: &str, text: &str, tags: &[String]) -> Option<(ExtId, &Shows)> {
        let mut best: Option<(ExtId, &Shows)> = None;
        for &(ext, n) in &self.shows {
            let claim = &self.manifest(ext).shows[n];
            let more =
                best.is_none_or(|(_, b)| claim.conditions.conditions() > b.conditions.conditions());
            if more && crate::matcher::note_meets(&claim.conditions, path, text, tags) {
                best = Some((ext, claim));
            }
        }
        best
    }

    /// Whether the extension's claim `n` holds: it wasn't left out.
    pub fn shows(&self, ext: ExtId, n: usize) -> bool {
        self.shows.contains(&(ext, n))
    }

    /// The extension that opens files that end in `.<extension>` (lowercase).
    pub fn owner(&self, extension: &str) -> Option<ExtId> {
        (self.owners.iter())
            .find(|(x, _)| x == extension)
            .map(|(_, e)| *e)
    }

    /// Each kind of file an extension owns, with its owner's name and version, for the core.
    pub fn owned_kinds(&self) -> Vec<Kind> {
        (self.owners.iter())
            .map(|(x, ext)| {
                let m = self.manifest(*ext);
                Kind {
                    extension: x.clone(),
                    owner: m.name.clone(),
                    version: m.version.clone(),
                }
            })
            .collect()
    }

    /// The extension that opens the file at `path` (a path or a file name), and its kind.
    pub fn owner_of(&self, path: &str) -> Option<(ExtId, &FileType)> {
        let x = files::extension_of(path)?;
        let ext = self.owner(&x)?;
        let t = (self.manifest(ext).file_types.iter()).find(|t| t.extension == x)?;
        Some((ext, t))
    }

    pub fn refused(&self) -> &[String] {
        &self.refused
    }

    /// The frontend's command table left these out (a command whose id is outside its
    /// extension's namespace or taken, a key that is taken or isn't one): they are listed
    /// with the rest, where a manifest names its commands.
    pub fn refuse_commands(&mut self, why: Vec<String>) {
        let after = self.refused.split_off(self.commands_at);
        self.refused.extend(why);
        self.commands_at = self.refused.len();
        self.refused.extend(after);
    }

    pub fn has_processes(&self) -> bool {
        self.processes
    }

    pub fn mistakes(&self) -> &[String] {
        &self.mistakes
    }

    pub fn ids(&self) -> impl Iterator<Item = ExtId> + use<> {
        (0..self.loaded.len()).map(ExtId)
    }

    pub fn running(&self, ext: ExtId) -> bool {
        self.loaded[ext.0].0.running()
    }

    pub fn stopped(&self, ext: ExtId) -> Option<&str> {
        self.loaded[ext.0].0.stopped()
    }

    /// cold-lava is quitting: let every extension go.
    pub fn shutdown(&mut self) {
        for (e, _) in &mut self.loaded {
            e.shutdown();
        }
    }

    pub fn manifest(&self, ext: ExtId) -> &Manifest {
        &self.loaded[ext.0].1
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.loaded.iter().map(|(_, m)| m)
    }

    /// The loaded extension with this name.
    pub fn by_name(&self, name: &str) -> Option<ExtId> {
        (self.loaded.iter())
            .position(|(_, m)| m.name == name)
            .map(ExtId)
    }

    /// Call one extension and return what it asked for, for the caller to apply.
    pub fn call(
        &mut self,
        ext: ExtId,
        core: Option<&mut Core>,
        f: impl FnOnce(&mut dyn Extension, &mut Host<'_>),
    ) -> Vec<Effect> {
        let (extension, manifest) = &mut self.loaded[ext.0];
        let mut host = Host {
            effects: Vec::new(),
            next_pane: &mut self.next_pane,
            next_view: &mut self.next_view,
            name: &manifest.name,
            permissions: &manifest.permissions,
            me: Some(ext),
            owners: &self.owners,
            core,
        };
        f(extension.as_mut(), &mut host);
        host.effects
    }

    /// Asks the extension `ext` about a shape it ships (`Binding::region_asks`, `Acted::Ask`).
    /// The ask carries the shape's inputs, so it needs `items.read`; without it the answer is
    /// that, at once.
    pub fn ask_shape(&mut self, ext: ExtId, mut core: Option<&mut Core>, ask: Ask) -> Vec<Effect> {
        let manifest = &self.loaded[ext.0].1;
        if !manifest.permissions.contains(&Permission::ItemsRead) {
            let why = format!(
                "{} may not be told a shape's inputs: its manifest's permissions don't include items.read",
                manifest.name
            );
            if let Some(core) = core.as_deref_mut() {
                core.shape_answered(&manifest.name, &ask, Err(why));
            }
            return Vec::new();
        }
        self.call(ext, core, |e, host| e.shape(ask, host))
    }

    /// The workspace gateway for one of the frontend's screens (`READER`), which may read and
    /// edit items. Screens ask for no effects.
    pub fn screen<'a>(&'a mut self, name: &'static str, core: &'a mut Core) -> Host<'a> {
        Host {
            effects: Vec::new(),
            next_pane: &mut self.next_pane,
            next_view: &mut self.next_view,
            name,
            permissions: SCREEN_PERMISSIONS,
            me: None,
            owners: &self.owners,
            core: Some(core),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::fmt::Debug;

    /// `value` is exactly `json` on the wire, and reads back from it.
    fn wire<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: T, json: Value) {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        assert_eq!(serde_json::from_value::<T>(json).unwrap(), value);
    }

    #[test]
    fn effects_are_json_rpc_messages() {
        let pane = PaneId(3);
        wire(
            Effect::Notify {
                message: "Neovim closed".into(),
            },
            json!({"method": "ui/notify", "params": {"message": "Neovim closed"}}),
        );
        wire(
            Effect::EditorOpened {
                note: "/v/Lava.md".into(),
            },
            json!({"method": "editor/opened", "params": {"note": "/v/Lava.md"}}),
        );
        wire(
            Effect::RunCommand {
                id: "search.open".into(),
                args: json!({"query": "ash"}),
            },
            json!({"method": "command/run", "params": {"id": "search.open", "args": {"query": "ash"}}}),
        );
        wire(
            Effect::Saved {
                note: "/v/Lava.md".into(),
            },
            json!({"method": "workspace/saved", "params": {"note": "/v/Lava.md"}}),
        );
        wire(
            Effect::OpenPane {
                pane,
                spec: PaneSpec {
                    program: "nvim".into(),
                    args: vec!["/v/Lava.md".into()],
                },
            },
            json!({"method": "pane/open", "params": {
                "pane": 3,
                "spec": {"program": "nvim", "args": ["/v/Lava.md"]},
            }}),
        );
        wire(
            Effect::PaneInput {
                pane,
                text: ":w\r".into(),
            },
            json!({"method": "pane/input", "params": {"pane": 3, "text": ":w\r"}}),
        );
        let view = ViewId(4);
        let content = || {
            Content::List(cold_lava_views::list::List {
                title: "Open".into(),
                groups: Vec::new(),
                keys: Vec::new(),
                empty: None,
                filter: Default::default(),
            })
        };
        let list = json!({"kind": "list", "title": "Open", "groups": []});
        wire(
            Effect::OpenView {
                view,
                content: content(),
                select: None,
                file: None,
            },
            json!({"method": "view/open", "params": {"view": 4, "content": list}}),
        );
        wire(
            Effect::OpenView {
                view,
                content: content(),
                select: None,
                file: Some("Volcanoes/Eruptions.csv".into()),
            },
            json!({"method": "view/open", "params": {"view": 4, "content": list, "file": "Volcanoes/Eruptions.csv"}}),
        );
        wire(
            Effect::UpdateView {
                view,
                content: content(),
                select: Some("r1".into()),
            },
            json!({"method": "view/update", "params": {"view": 4, "content": list, "select": "r1"}}),
        );
        wire(
            Effect::CloseView { view },
            json!({"method": "view/close", "params": {"view": 4}}),
        );
        wire(
            Effect::ShowView { view },
            json!({"method": "view/show", "params": {"view": 4}}),
        );
        wire(
            Effect::OpenNote {
                note: "Plan.md".into(),
                line: Some(3),
                item: None,
            },
            json!({"method": "workspace/open", "params": {"note": "Plan.md", "line": 3}}),
        );
        wire(
            Effect::OpenNote {
                note: "Volcanoes/Eruptions.csv".into(),
                line: None,
                item: Some("6f3e".into()),
            },
            json!({"method": "workspace/open",
                   "params": {"note": "Volcanoes/Eruptions.csv", "item": "6f3e"}}),
        );
    }

    #[test]
    fn host_calls_are_json_rpc_messages() {
        let pane = PaneId(3);
        let open = |jump| EditorRequest::Open {
            note: "/v/Lava.md".into(),
            jump,
        };
        wire(
            open(Some(Jump::Heading("Flows".into()))),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": {"heading": "Flows"}}}),
        );
        wire(
            open(Some(Jump::Line(12))),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": {"line": 12}}}),
        );
        wire(
            open(Some(Jump::Block("ab12".into()))),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": {"block": "ab12"}}}),
        );
        wire(
            open(None),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": null}}),
        );
        wire(EditorRequest::Save, json!({"method": "editor/save"}));
        wire(
            EditorRequest::Renamed {
                from: "/v/Lava.md".into(),
                to: "/v/Rock/Lava.md".into(),
            },
            json!({"method": "editor/renamed", "params": {"from": "/v/Lava.md", "to": "/v/Rock/Lava.md"}}),
        );
        wire(
            EditorRequest::Deleted {
                note: "/v/Lava.md".into(),
            },
            json!({"method": "editor/deleted", "params": {"note": "/v/Lava.md"}}),
        );
        wire(
            PaneNews::Message {
                pane,
                kind: "open".into(),
                payload: "/v/Lava.md".into(),
            },
            json!({"method": "pane/message", "params": {"pane": 3, "kind": "open", "payload": "/v/Lava.md"}}),
        );
        wire(
            PaneNews::Exited {
                pane,
                error: Some("not found".into()),
            },
            json!({"method": "pane/exited", "params": {"pane": 3, "error": "not found"}}),
        );
    }

    #[test]
    fn command_run_is_a_json_rpc_message() {
        wire(
            CommandRequest::Run {
                id: "nvim.restart".into(),
                args: Value::Null,
            },
            json!({"method": "command/run", "params": {"id": "nvim.restart", "args": null}}),
        );
    }

    fn command(id: &str, keys: &[&str]) -> CommandSpec {
        CommandSpec {
            id: id.into(),
            title: format!("does {id}"),
            keys: keys.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn manifest_is_json() {
        wire(
            Manifest {
                name: "nvim".into(),
                title: "Neovim".into(),
                version: String::new(),
                description: String::new(),
                roles: vec![Role::Editor],
                hints: vec![("Ctrl-o".into(), "back".into())],
                commands: vec![command("nvim.restart", &["ctrl+r"])],
                types: Vec::new(),
                shapes: Vec::new(),
                permissions: vec![Permission::ItemsRead, Permission::ItemsEdit],
                file_types: Vec::new(),
                shows: Vec::new(),
            },
            json!({
                "name": "nvim",
                "title": "Neovim",
                "roles": ["editor"],
                "hints": [["Ctrl-o", "back"]],
                "commands": [{"id": "nvim.restart", "title": "does nvim.restart", "keys": ["ctrl+r"]}],
                "permissions": ["items.read", "items.edit"],
            }),
        );
        let without =
            json!({"name": "wc", "title": "wc", "roles": [], "hints": [], "commands": []});
        let read = serde_json::from_value::<Manifest>(without).unwrap();
        assert_eq!(
            read.permissions,
            [Permission::ItemsRead],
            "read only by default"
        );
    }

    /// An extension that only registers commands.
    struct Commands {
        name: &'static str,
        commands: Vec<CommandSpec>,
    }

    impl Extension for Commands {
        fn initialize(&mut self) -> Manifest {
            Manifest {
                name: self.name.into(),
                title: self.name.into(),
                version: String::new(),
                description: String::new(),
                roles: Vec::new(),
                hints: Vec::new(),
                commands: self.commands.clone(),
                types: Vec::new(),
                shapes: Vec::new(),
                permissions: Vec::new(),
                file_types: Vec::new(),
                shows: Vec::new(),
            }
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
    }

    /// What a `Ticker` did and heard.
    #[derive(Default)]
    struct Log {
        subscribed: Option<Result<SubId, Reason>>,
        ticked: Option<Result<bool, Reason>>,
        heard: Vec<News>,
    }

    /// An extension that, on its command, follows the workspace's tasks and ticks the first.
    struct Ticker {
        name: &'static str,
        permissions: Vec<Permission>,
        log: std::rc::Rc<std::cell::RefCell<Log>>,
    }

    impl Extension for Ticker {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: self.name,
                commands: Vec::new(),
            })
            .initialize();
            manifest.permissions = self.permissions.clone();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
        fn command(&mut self, _: CommandRequest, host: &mut Host<'_>) {
            let mut log = self.log.borrow_mut();
            let tasks = Select {
                kind: "task".into(),
                ..Select::default()
            };
            log.subscribed = Some(host.subscribe_items(tasks.clone()).map_err(|e| e.reason));
            let query = Query {
                select: tasks,
                ..Query::default()
            };
            let Ok(found) = host.query_items(&query) else {
                return;
            };
            let first = &found.items[0];
            let tick = Edit {
                id: first.id.clone(),
                rev: first.rev.clone(),
                op: gateway::Op::Toggle,
            };
            log.ticked = Some(
                host.edit_item(&tick)
                    .map(|e| matches!(e, Answer::Now(e) if e.changed))
                    .map_err(|e| e.reason),
            );
        }
        fn items(&mut self, news: News, _: &mut Host<'_>) {
            self.log.borrow_mut().heard.push(news);
        }
    }

    #[test]
    fn the_gateway_answers_within_the_manifests_permissions() {
        use Permission::{ItemsEdit, ItemsRead};
        let dir = std::env::temp_dir().join(format!("cold-lava-ext-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(
            dir.join("Plan.md"),
            "- [ ] Pay rent\n- [ ] Call the plumber\n",
        )
        .unwrap();
        let mut core = Core::open(&dir).unwrap();
        let tickers = [
            ("blind", vec![]),
            ("look", vec![ItemsRead]),
            ("tick", vec![ItemsRead, ItemsEdit]),
        ];
        let logs: Vec<_> = tickers.iter().map(|_| Default::default()).collect();
        let loaded = (tickers.iter().zip(&logs)).map(|((name, permissions), log)| {
            let (name, permissions, log) = (*name, permissions.clone(), std::rc::Rc::clone(log));
            Box::new(Ticker {
                name,
                permissions,
                log,
            }) as Box<dyn Extension>
        });
        let mut r = Registry::new(loaded.collect());
        let run = |e: &mut dyn Extension, host: &mut Host<'_>| {
            e.command(
                CommandRequest::Run {
                    id: String::new(),
                    args: Value::Null,
                },
                host,
            )
        };
        for i in 0..3 {
            r.call(ExtId(i), Some(&mut core), run);
        }
        let log = |i: usize| logs[i].borrow();
        assert_eq!(log(0).subscribed, Some(Err(Reason::Forbidden)));
        assert_eq!(log(0).ticked, None, "it couldn't query either");
        assert!(matches!(log(1).subscribed, Some(Ok(_))));
        assert_eq!(log(1).ticked, Some(Err(Reason::Forbidden)));
        assert_eq!(log(2).ticked, Some(Ok(true)));
        let text = std::fs::read_to_string(dir.join("Plan.md")).unwrap();
        assert!(text.starts_with("- [x] Pay rent ✅ "), "{text}");
        let edited = core.poll().into_iter().find_map(|e| match e {
            cold_lava_engine::core::Event::ItemEdited { by, what, .. } => Some((by, what)),
            _ => None,
        });
        assert_eq!(edited, Some(("tick".into(), "ticked".into())), "attributed");

        // The menu hands each subscription's news to the extension that made it.
        for (owner, news) in core.take_item_news() {
            let ext = r.by_name(&owner).unwrap();
            r.call(ext, Some(&mut core), |e, host| e.items(news, host));
        }
        for i in [1, 2] {
            let heard = &log(i).heard;
            assert!(
                matches!(&heard[..], [News::Changed { updated, .. }] if updated.len() == 1),
                "{heard:?}"
            );
        }
        assert!(log(0).heard.is_empty());

        let no_workspace = r.call(ExtId(1), None, run);
        assert!(no_workspace.is_empty());
        assert_eq!(log(1).subscribed, Some(Err(Reason::NoWorkspace)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An extension that opens files of some kinds and does nothing itself: its host is used
    /// directly, through `Registry::call`.
    struct Owner {
        name: &'static str,
        opens: &'static [&'static str],
        permissions: Vec<Permission>,
    }

    impl Extension for Owner {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: self.name,
                commands: Vec::new(),
            })
            .initialize();
            manifest.permissions = self.permissions.clone();
            manifest.file_types = (self.opens.iter())
                .map(|x| FileType {
                    extension: x.to_string(),
                    title: format!("{x} file"),
                    new: None,
                })
                .collect();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
    }

    /// An extension that ships the shape `tally.list` and answers for it: its region says how
    /// many volcanoes it is bound to, and a click edits the clicked one with `set_field`.
    struct Tally {
        permissions: Vec<Permission>,
        set_field: serde_json::Value,
    }

    const TALLY: &str = r#"
        inputs = ["a"]
        parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
        regions = [{ id = "count", area = "box", extension = true }]
        [[on]]
        click = "count"
        extension = true
    "#;

    impl Extension for Tally {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: "tally",
                commands: Vec::new(),
            })
            .initialize();
            manifest.permissions = self.permissions.clone();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
        fn shape(&mut self, ask: Ask, host: &mut Host<'_>) {
            let answer = match &ask {
                Ask::Region { inputs, .. } => json!({"items": [
                    {"label": format!("{} volcanoes", inputs["a"].members.len())}
                ]}),
                Ask::Gesture { .. } => json!({"do": [{"edit": {"item": "clicked",
                    "set-field": self.set_field}}]}),
            };
            host.shape_answered(&ask, Ok(serde_json::from_value(answer).unwrap()));
        }
    }

    /// A workspace of two volcanoes whose `visited` an edit can set, and Tally's shape loaded.
    fn tally_workspace(name: &str) -> (PathBuf, crate::shapes::Shapes) {
        use crate::shapes::{Layer, load};
        let dir = std::env::temp_dir().join(format!("cold-lava-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        std::fs::write(
            dir.join(".cold-lava/types.toml"),
            "version = 1\n[types.volcano]\non = \"note\"\nwhere.tag = [\"volcano\"]\n\
             fields.name = { note = \"name\" }\nfields.visited = { property = \"visited\" }\n",
        )
        .unwrap();
        for name in ["Krakatoa", "Vesuvius"] {
            std::fs::write(
                dir.join(format!("{name}.md")),
                "---\ntags: [volcano]\n---\n",
            )
            .unwrap();
        }
        let table: toml::Table = toml::from_str(TALLY).unwrap();
        let shapes = load([
            Layer::built_in(),
            Layer::extension("tally", [("tally.list".to_string(), table)]),
        ]);
        assert!(shapes.errors.is_empty(), "{:?}", shapes.errors);
        (dir, shapes)
    }

    /// A click on the first volcano, in the region Tally answers for.
    fn click_on_a_volcano(core: &mut Core) -> crate::act::Gesture {
        let q = crate::gateway::Query {
            select: Select {
                kind: "volcano".into(),
                ..Default::default()
            },
            page: Default::default(),
        };
        crate::act::Gesture::Click {
            on: "count".into(),
            item: Some(core.query_items(&q).unwrap().items.remove(0).id),
        }
    }

    #[test]
    fn an_extension_answers_for_its_shape_within_its_permissions() {
        use crate::act::{Acted, Answered};
        use crate::bind::{Binding, Source};
        use ItemsEdit as E;
        use ItemsRead as R;
        use Permission::{ItemsEdit, ItemsRead};
        let (dir, shapes) = tally_workspace("ext-shape");

        for (permissions, region, clicked) in [
            (
                vec![],
                Err(
                    "tally may not be told a shape's inputs: its manifest's permissions don't include items.read",
                ),
                None,
            ),
            (vec![R], Ok("2 volcanoes"), Some(Err(Reason::Forbidden))),
            (vec![R, E], Ok("2 volcanoes"), Some(Ok(()))),
        ] {
            let mut core = Core::open(&dir).unwrap();
            let mut r = Registry::new(vec![Box::new(Tally {
                permissions: permissions.clone(),
                set_field: json!({"field": "visited", "value": "yes"}),
            })]);
            let source = Source::parse("type=volcano").unwrap();
            let mut b =
                Binding::new(&mut core, &shapes, "tally.list", [("a".into(), source)]).unwrap();
            assert_eq!(b.extension(), Some("tally"));
            for ask in b.region_asks(1) {
                r.ask_shape(ExtId(0), Some(&mut core), ask);
            }
            let answers = core.take_shape_answers();
            let got = match &answers[..] {
                [a] => a.answer.clone(),
                _ => panic!("{answers:?}"),
            };
            match (got, region) {
                (Ok(Answered::Region { items }), Ok(label)) => {
                    b.answered("count", items);
                    assert_eq!(b.shown().regions[0].items[0].label, label);
                }
                (Err(why), Err(expected)) => assert_eq!(why, expected),
                (got, expected) => panic!("{permissions:?}: {got:?}, not {expected:?}"),
            }
            let Some(clicked) = clicked else {
                continue;
            };
            let click = click_on_a_volcano(&mut core);
            let mut acted = None;
            r.call(ExtId(0), Some(&mut core), |_, host| {
                acted = Some(b.act(host, &click, 1))
            });
            let Some(Ok(Acted::Ask(ask))) = acted else {
                panic!("its extension decides: {acted:?}");
            };
            r.ask_shape(ExtId(0), Some(&mut core), ask);
            let steps = match core.take_shape_answers().remove(0).answer {
                Ok(Answered::Gesture { steps }) => steps,
                a => panic!("{a:?}"),
            };
            let mut ran = None;
            r.call(ExtId(0), Some(&mut core), |_, host| {
                ran = Some(b.run(host, &click, &steps))
            });
            let text = std::fs::read_to_string(dir.join("Krakatoa.md")).unwrap();
            match (ran.unwrap(), clicked) {
                (Err(e), Err(reason)) => {
                    assert_eq!(e.reason, reason);
                    assert!(!text.contains("visited"), "nothing written: {text}");
                }
                (Ok(_), Ok(())) => {
                    assert!(text.contains("visited: yes"), "{text}");
                    let by = core.poll().into_iter().find_map(|e| match e {
                        cold_lava_engine::core::Event::ItemEdited { by, .. } => Some(by),
                        _ => None,
                    });
                    assert_eq!(by.as_deref(), Some("tally"), "attributed to the extension");
                }
                (got, expected) => panic!("{got:?}, not {expected:?}"),
            }
            b.close(&mut core);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The steps of an answer get the checks a loaded shape's steps get (#268): the frontend
    /// is handed the reason, never steps to run.
    #[test]
    fn steps_an_extension_answers_that_cant_run_are_refused() {
        use crate::act::Acted;
        use crate::bind::{Binding, Source};
        let (dir, shapes) = tally_workspace("ext-shape-unchecked");
        let mut core = Core::open(&dir).unwrap();
        let mut r = Registry::new(vec![Box::new(Tally {
            permissions: vec![Permission::ItemsRead, Permission::ItemsEdit],
            set_field: json!({"field": "visited"}),
        })]);
        let source = Source::parse("type=volcano").unwrap();
        let mut b = Binding::new(&mut core, &shapes, "tally.list", [("a".into(), source)]).unwrap();
        let click = click_on_a_volcano(&mut core);
        let mut acted = None;
        r.call(ExtId(0), Some(&mut core), |_, host| {
            acted = Some(b.act(host, &click, 1))
        });
        let Some(Ok(Acted::Ask(ask))) = acted else {
            panic!("its extension decides: {acted:?}");
        };
        r.ask_shape(ExtId(0), Some(&mut core), ask);
        let answers = core.take_shape_answers();
        let [answer] = &answers[..] else {
            panic!("{answers:?}");
        };
        assert_eq!(answer.by, "tally");
        let refused = "it answered shape/gesture with steps that can't run: set-field takes \
                       exactly one of value, add and remove";
        assert_eq!(answer.answer, Err(refused.to_string()));
        for name in ["Krakatoa", "Vesuvius"] {
            let text = std::fs::read_to_string(dir.join(format!("{name}.md"))).unwrap();
            assert!(!text.contains("visited"), "nothing written: {text}");
        }
        b.close(&mut core);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn owners(extensions: Vec<Owner>) -> Registry {
        Registry::new(
            (extensions.into_iter())
                .map(|o| Box::new(o) as Box<dyn Extension>)
                .collect(),
        )
    }

    #[test]
    fn each_kind_of_file_has_one_owner_and_notes_have_none() {
        let owner = |name, opens| Owner {
            name,
            opens,
            permissions: Vec::new(),
        };
        let r = owners(vec![
            owner("sheets", &["csv", "tsv"]),
            owner("other", &["csv", "md", "kanban"]),
        ]);
        let ext = |path: &str| r.owner_of(path).map(|(e, t)| (e, t.title.clone()));
        assert_eq!(
            ext("Data/Eruptions.CSV"),
            Some((ExtId(0), "csv file".into()))
        );
        assert_eq!(ext("Board.kanban"), Some((ExtId(1), "kanban file".into())));
        assert_eq!(ext("Plan.md"), None);
        assert_eq!(ext("csv"), None, "a name without an extension");
        assert_eq!((r.owner("tsv"), r.owner("md")), (Some(ExtId(0)), None));
        assert_eq!(
            r.refused(),
            [
                "other and sheets both open .csv files: sheets does",
                "other can't open .md files: cold-lava does",
            ]
        );
    }

    struct Shower {
        name: &'static str,
        shows: &'static [&'static str],
    }

    impl Extension for Shower {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: self.name,
                commands: Vec::new(),
            })
            .initialize();
            manifest.shows = (self.shows.iter())
                .map(|w| Shows {
                    title: format!("{} view", self.name),
                    conditions: toml::from_str(w).unwrap(),
                })
                .collect();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
    }

    #[test]
    fn the_most_specific_claim_shows_a_note_and_equal_claims_conflict() {
        let r = Registry::new(vec![
            Box::new(Shower {
                name: "kanban",
                shows: &[r#"property = { kanban-plugin = "basic" }"#],
            }),
            Box::new(Shower {
                name: "boards",
                shows: &[
                    r#"property = { kanban-plugin = "basic" }"#,
                    r#"property = { kanban-plugin = "basic" }
                       folder = ["Work"]"#,
                    r#"tag = ["cards"]"#,
                    r#"header = ["Front"]"#,
                    "",
                ],
            }),
        ]);
        let board = "---\nkanban-plugin: basic\n---\n## Doing\n";
        let shower = |path: &str, text: &str, tags: &[&str]| {
            let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
            (r.shower_of(path, text, &tags)).map(|(e, s)| (e, s.describe()))
        };
        assert_eq!(
            shower("Board.md", board, &[]),
            Some((ExtId(0), "kanban-plugin: basic".into()))
        );
        assert_eq!(
            shower("Work/Board.md", board, &["cards"]),
            Some((ExtId(1), "kanban-plugin: basic, in Work".into())),
            "more conditions win"
        );
        assert_eq!(
            shower("Deck.md", "# Deck\n", &["cards/spanish"]),
            Some((ExtId(1), "#cards".into())),
            "a nested tag counts"
        );
        assert_eq!(shower("Plan.md", "# Plan\n", &[]), None);
        assert_eq!(
            r.refused(),
            [
                "boards and kanban both show notes with kanban-plugin: basic: kanban does",
                "boards: shows \"\" was left out: its where may only name property, tag and folder",
                "boards: shows \"\" was left out: its where must name a property, a tag or a \
                 folder: it can't show every note",
            ]
        );
        assert!(r.shows(ExtId(1), 1) && !r.shows(ExtId(1), 0));
    }

    #[test]
    fn owners_read_and_write_their_own_kinds_and_others_need_files_permissions() {
        use Permission::{FilesRead, FilesWrite};
        let dir = std::env::temp_dir().join(format!("cold-lava-ext-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent\n").unwrap();
        std::fs::write(dir.join("Eruptions.csv"), "name\nKrakatoa\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let owner = |name, opens, permissions| Owner {
            name,
            opens,
            permissions,
        };
        let mut r = owners(vec![
            owner("sheets", &["csv"], Vec::new()),
            owner("blind", &[], Vec::new()),
            owner("look", &[], vec![FilesRead]),
            owner("write", &[], vec![FilesRead, FilesWrite]),
        ]);
        let mut asked = |ext: usize, f: &dyn Fn(&mut Host<'_>) -> Result<(), gateway::Error>| {
            let mut answer = None;
            r.call(ExtId(ext), Some(&mut core), |_, host| {
                answer = Some(f(host))
            });
            answer.unwrap().map_err(|e| e.reason)
        };
        let read = |path: &'static str| move |h: &mut Host<'_>| h.read_file(path).map(drop);
        let write = |path: &'static str| {
            move |h: &mut Host<'_>| {
                let now = h.read_file(path)?;
                h.write_file(path, &format!("{}x\n", now.text), &now.rev)
                    .map(drop)
            }
        };
        let list = |x: &'static str| move |h: &mut Host<'_>| h.list_files(x).map(drop);
        let create = |path: &'static str| {
            move |h: &mut Host<'_>| {
                let at = files::NewFile::At { path: path.into() };
                let text = String::new();
                h.create_file(&Create { at, text }).map(drop)
            }
        };
        let forbidden = Err(Reason::Forbidden);
        assert_eq!(asked(0, &read("Eruptions.csv")), Ok(()), "its own kind");
        assert_eq!(asked(0, &write("Eruptions.csv")), Ok(()));
        assert_eq!(asked(0, &list("csv")), Ok(()));
        assert_eq!(asked(0, &create("New.csv")), Ok(()));
        assert_eq!(
            asked(0, &read("Plan.md")),
            forbidden,
            "a note isn't its kind"
        );
        assert_eq!(asked(0, &list("md")), forbidden);
        assert_eq!(
            asked(1, &read("Eruptions.csv")),
            forbidden,
            "another's kind"
        );
        assert_eq!(asked(2, &read("Eruptions.csv")), Ok(()));
        assert_eq!(asked(2, &list("csv")), Ok(()));
        assert_eq!(asked(2, &write("Plan.md")), forbidden);
        assert_eq!(asked(2, &create("Other.md")), forbidden);
        assert_eq!(asked(3, &write("Plan.md")), Ok(()));
        assert_eq!(asked(3, &create("Other.md")), Ok(()));
        assert_eq!(asked(3, &create("Other.md")), Err(Reason::Exists));
        let text = std::fs::read_to_string(dir.join("Eruptions.csv")).unwrap();
        assert_eq!(text, "name\nKrakatoa\nx\n");
        assert!(dir.join("New.csv").exists() && dir.join("Other.md").exists());
        let mut screen = r.screen(READER, &mut core);
        let screens = screen.read_file("Eruptions.csv").map_err(|e| e.reason);
        assert_eq!(
            screens.map(drop),
            forbidden,
            "screens read notes as items only"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_news_and_file_types_are_json() {
        wire(
            FileNews::Open {
                path: "Data/Eruptions.csv".into(),
                place: None,
            },
            json!({"method": "file/open", "params": {"path": "Data/Eruptions.csv"}}),
        );
        wire(
            FileNews::Open {
                path: "Data/Eruptions.csv".into(),
                place: Some("Krakatoa".into()),
            },
            json!({"method": "file/open",
                   "params": {"path": "Data/Eruptions.csv", "place": "Krakatoa"}}),
        );
        wire(
            FileNews::Changed {
                path: "Data/Eruptions.csv".into(),
                rev: None,
            },
            json!({"method": "file/changed", "params": {"path": "Data/Eruptions.csv", "rev": null}}),
        );
        wire(
            FileType {
                extension: "csv".into(),
                title: "CSV table".into(),
                new: Some("csv-table.new".into()),
            },
            json!({"extension": "csv", "title": "CSV table", "new": "csv-table.new"}),
        );
        wire(
            FilePreview {
                path: "Data/Eruptions.csv".into(),
                rev: "00ff00ff00ff00ff".into(),
                width: 60,
                height: 8,
            },
            json!({"path": "Data/Eruptions.csv", "rev": "00ff00ff00ff00ff",
                   "width": 60, "height": 8}),
        );
    }

    #[test]
    fn types_outside_the_namespace_or_taken_are_left_out() {
        let spec = |id: &str| Spec {
            id: id.into(),
            definition: toml::from_str("on = \"note\"").unwrap(),
        };
        let mut m = Manifest {
            name: "wc".into(),
            title: "wc".into(),
            version: String::new(),
            description: String::new(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: Vec::new(),
            types: ["doc", "wcx.doc", "wc.", "wc.doc", "wc.doc"]
                .map(spec)
                .to_vec(),
            shapes: ["lane", "wc.lane"].map(spec).to_vec(),
            permissions: read_only(),
            file_types: Vec::new(),
            shows: Vec::new(),
        };
        let refused = leave_out_definitions(&mut m);
        let ids: Vec<&str> = m.types.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["wc.doc"]);
        assert_eq!(m.shapes.len(), 1);
        assert_eq!(refused.len(), 5, "{refused:?}");
        assert_eq!(
            refused[4],
            "wc: shape \"lane\" was left out: its id must start with \"wc.\""
        );
        assert_eq!(
            refused[0],
            "wc: type \"doc\" was left out: its id must start with \"wc.\""
        );
        assert!(refused[3].ends_with("that id is taken"), "{refused:?}");
    }
}
