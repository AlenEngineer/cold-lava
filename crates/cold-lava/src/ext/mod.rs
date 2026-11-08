//! Extensions: what cold-lava does beyond the notes app itself, starting with the editor.
//!
//! This is the JSON-RPC protocol of docs/extensions.md, called in-process: each `Extension`
//! method is one host→extension method, each `Host` method one extension→host method, and every
//! type that crosses derives serde, so extensions in other processes will share this code path.
//! Extensions never see keystrokes and never draw: the core owns the panes they open. What an
//! extension asks for is queued as `Effect`s, which the menu applies after the call returns.
//! Notes as data are the exception: `items/*` and `types/list` are answered during the call by
//! the core (`crate::gateway`), within the permissions the extension's manifest asks for.
//! Extensions installed on disk (`installed`) run in processes of their own (`process`), over a
//! `transport` that frames JSON-RPC messages (`frame`, `rpc`); they reach the same `Host`.

pub mod frame;
pub mod installed;
pub mod nvim;
pub mod process;
pub mod rpc;
pub mod transport;
pub mod view;

use crate::commands::{self, Entry, MenuCommand};
use crate::core::{Core, Jump};
use crate::gateway::{self, Edit, Edited, Found, News, Query, Reason, Select, SubId};
use crate::types::{Definitions, Type};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use view::{Content, ViewId, ViewNews};

/// A loaded extension. Only the host uses it: on the wire, the connection says who is talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtId(pub(crate) usize);

/// A pane, named by the extension that opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneId(u32);

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
    /// Types it defines, named `<extension name>.<type>` once loaded.
    #[serde(default, skip_serializing_if = "Definitions::is_empty")]
    pub types: Definitions,
    /// What it may ask of the workspace gateway. Without the field, read only.
    #[serde(default = "read_only")]
    pub permissions: Vec<Permission>,
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
}

impl Permission {
    fn name(self) -> &'static str {
        match self {
            Permission::ItemsRead => "items.read",
            Permission::ItemsEdit => "items.edit",
        }
    }
}

pub(crate) fn read_only() -> Vec<Permission> {
    vec![Permission::ItemsRead]
}

/// The menu's screens built the way their extensions will be: they reach notes as data only
/// through `Host`, under these names (`Registry::screen`), which edits are attributed to.
pub const TABLE: &str = "table";
pub const AGENDA: &str = "agenda";
pub const READER: &str = "reader";
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
    /// Show a view; the host named it `view`.
    #[serde(rename = "view/open")]
    OpenView { view: ViewId, content: Content },
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
    /// Open a note, at a line: a path inside the workspace, absolute or relative to its root
    /// as items name their notes.
    #[serde(rename = "workspace/open")]
    OpenNote {
        note: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
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
    /// returns to. Refused when it is over the limits (`view::MAX_ROWS`, …).
    pub fn open_view(&mut self, content: Content) -> Result<ViewId, gateway::Error> {
        content
            .check()
            .map_err(|why| gateway::Error::new(Reason::Refused, why))?;
        *self.next_view += 1;
        let view = ViewId(*self.next_view);
        self.effects.push(Effect::OpenView { view, content });
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

    /// `workspace/open`: show a note, at a line.
    pub fn open_note(&mut self, note: PathBuf, line: Option<usize>) {
        self.effects.push(Effect::OpenNote { note, line });
    }

    /// The core, when the caller's manifest asks for `needs`.
    fn core(&mut self, needs: Permission) -> Result<&mut Core, gateway::Error> {
        if !self.permissions.contains(&needs) {
            let message = format!(
                "{} may not do this: its manifest's permissions don't include {}",
                self.name,
                needs.name()
            );
            return Err(gateway::Error::new(Reason::Forbidden, message));
        }
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

    /// `items/edit`, made at once through the core's safe edits and attributed to the caller.
    pub fn edit_item(&mut self, edit: &Edit) -> Result<Edited, gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsEdit)?.edit_item(name, edit)
    }

    /// `types/list`
    pub fn types(&mut self) -> Result<Vec<Type>, gateway::Error> {
        let core = self.core(Permission::ItemsRead)?;
        Ok(core.index.workspace.item_settings().types.types.clone())
    }
}

pub struct Registry {
    loaded: Vec<(Box<dyn Extension>, Manifest)>,
    /// The one in the editor role, until its program fails to start.
    editor: Option<ExtId>,
    next_pane: u32,
    next_view: u32,
    /// The menu's commands: the core's, then the extensions'.
    commands: Vec<Entry<MenuCommand>>,
    /// Why commands or keys the extensions registered were left out.
    refused: Vec<String>,
    /// Extensions on disk that didn't load, and why.
    mistakes: Vec<String>,
    /// Whether any extension runs in a process, so the menu polls more often.
    processes: bool,
}

impl Registry {
    /// The built-in extensions that can run here, then the user's (`installed::user_dir`).
    pub fn load() -> Registry {
        let mut found: Vec<Box<dyn Extension>> = Vec::new();
        if let Some(nvim) = nvim::Nvim::detect() {
            found.push(Box::new(nvim));
        }
        found.push(Box::new(crate::agenda::Agenda::default()));
        let dir = installed::user_dir();
        let (on_disk, mistakes) = dir.as_deref().map(installed::load).unwrap_or_default();
        let processes = !on_disk.is_empty();
        for ext in on_disk {
            found.extend(process::Process::new(ext).map(|p| Box::new(p) as Box<dyn Extension>));
        }
        let mut r = Registry::new(found);
        r.mistakes = mistakes;
        r.processes = processes;
        r
    }

    #[cfg(test)]
    pub fn empty() -> Registry {
        Registry::new(Vec::new())
    }

    pub(crate) fn new(extensions: Vec<Box<dyn Extension>>) -> Registry {
        let loaded: Vec<_> = extensions
            .into_iter()
            .map(|mut e| {
                let manifest = e.initialize();
                (e, manifest)
            })
            .collect();
        let editor = loaded
            .iter()
            .position(|(_, m)| m.roles.contains(&Role::Editor))
            .map(ExtId);
        let manifests = loaded.iter().enumerate().map(|(i, (_, m))| (ExtId(i), m));
        let (commands, refused) = commands::menu(manifests);
        Registry {
            loaded,
            editor,
            next_pane: 0,
            next_view: 0,
            commands,
            refused,
            mistakes: Vec::new(),
            processes: false,
        }
    }

    pub fn commands(&self) -> &[Entry<MenuCommand>] {
        &self.commands
    }

    pub fn refused(&self) -> &[String] {
        &self.refused
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

    pub fn editor(&self) -> Option<ExtId> {
        self.editor
    }

    /// No more editor requests: its program couldn't start.
    pub fn drop_editor(&mut self) {
        self.editor = None;
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
            core,
        };
        f(extension.as_mut(), &mut host);
        host.effects
    }

    /// The workspace gateway for one of the menu's screens (`TABLE`, `AGENDA`, `READER`), which
    /// may read and edit items. Screens ask for no effects.
    pub fn screen<'a>(&'a mut self, name: &'static str, core: &'a mut Core) -> Host<'a> {
        Host {
            effects: Vec::new(),
            next_pane: &mut self.next_pane,
            next_view: &mut self.next_view,
            name,
            permissions: SCREEN_PERMISSIONS,
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
            Content::List(crate::list::List {
                title: "Open".into(),
                groups: Vec::new(),
                keys: Vec::new(),
                empty: None,
            })
        };
        let list = json!({"kind": "list", "title": "Open", "groups": []});
        wire(
            Effect::OpenView {
                view,
                content: content(),
            },
            json!({"method": "view/open", "params": {"view": 4, "content": list}}),
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
            Effect::OpenNote {
                note: "Plan.md".into(),
                line: Some(3),
            },
            json!({"method": "workspace/open", "params": {"note": "Plan.md", "line": 3}}),
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
                types: Definitions::new(),
                permissions: vec![Permission::ItemsRead, Permission::ItemsEdit],
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
                types: Definitions::new(),
                permissions: Vec::new(),
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
                    .map(|e| e.changed)
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
            crate::core::Event::ItemEdited { by, what, .. } => Some((by, what)),
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

    fn registry(extensions: &[(&'static str, Vec<CommandSpec>)]) -> Registry {
        let loaded = extensions.iter().map(|(name, commands)| {
            let commands = commands.clone();
            Box::new(Commands { name, commands }) as Box<dyn Extension>
        });
        Registry::new(loaded.collect())
    }

    fn ids(r: &Registry) -> Vec<&str> {
        let ext = r
            .commands()
            .iter()
            .filter(|c| matches!(c.action, MenuCommand::Ext(..)));
        ext.map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn commands_join_the_menu_after_the_cores() {
        let r = registry(&[("wc", vec![command("wc.show", &["c"])])]);
        assert!(r.refused().is_empty(), "{:?}", r.refused());
        let last = r.commands().last().unwrap();
        assert_eq!(last.id, "wc.show");
        assert_eq!(last.action, MenuCommand::Ext(ExtId(0), 0));
        assert_eq!(r.commands().len(), commands::MENU.len() + 1);
        let keys = commands::Keymap::with(r.commands(), &Default::default()).0;
        assert_eq!(keys.label("wc.show"), "c");
        let palette = commands::matching(r.commands(), "wc.show");
        assert_eq!(palette[0].command.id, "wc.show");
    }

    #[test]
    fn commands_outside_the_namespace_or_taken_are_refused() {
        let r = registry(&[
            (
                "wc",
                vec![
                    command("graph.open", &[]),
                    command("wcx.show", &[]),
                    command("wc.", &[]),
                    command("wc.show", &[]),
                    command("wc.show", &[]),
                ],
            ),
            // An extension named like a core namespace can't take a core command.
            ("note", vec![command("note.rename", &[])]),
        ]);
        assert_eq!(ids(&r), ["wc.show"]);
        let refused = r.refused();
        assert_eq!(refused.len(), 5, "{refused:?}");
        assert!(refused[0].contains("\"graph.open\"") && refused[0].contains("\"wc.\""));
        assert!(refused[3].contains("taken"), "{refused:?}");
        assert!(refused[4].contains("note.rename"), "{refused:?}");
        // What a refused command's id was meant to run is still the core's.
        let keys = commands::Keymap::with(r.commands(), &Default::default()).0;
        assert_eq!(keys.label("graph.open"), "g");
    }

    #[test]
    fn default_keys_that_are_taken_or_bad_are_dropped() {
        let r = registry(&[
            ("a", vec![command("a.one", &["g", "hyper+x", "x"])]),
            ("b", vec![command("b.two", &["x", "y"])]),
        ]);
        assert_eq!(ids(&r), ["a.one", "b.two"]);
        let refused = r.refused();
        assert_eq!(refused.len(), 3, "{refused:?}");
        assert!(refused[0].contains("graph.open"), "{refused:?}");
        assert!(refused[1].contains("hyper+x"), "{refused:?}");
        assert!(refused[2].contains("a.one"), "{refused:?}");
        let keys = commands::Keymap::with(r.commands(), &Default::default()).0;
        assert_eq!(keys.label("a.one"), "x");
        assert_eq!(keys.label("b.two"), "y");
    }

    #[test]
    fn config_rebinds_extension_commands_like_the_cores() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let r = registry(&[("wc", vec![command("wc.show", &["c"])])]);
        let press = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);

        let config = crate::config::parse("[keys]\n\"wc.show\" = \"W\"\n", r.commands());
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        let keys = commands::Keymap::with(r.commands(), &config.keys).0;
        assert_eq!(keys.get(press('W')), Some(MenuCommand::Ext(ExtId(0), 0)));
        assert_eq!(keys.get(press('c')), None);
        assert_eq!(keys.label("wc.show"), "W");

        // Clash rules are the core's: both keep their defaults.
        let config = crate::config::parse("[keys]\n\"wc.show\" = \"g\"\n", r.commands());
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        assert!(config.errors[0].contains("wc.show") && config.errors[0].contains("graph.open"));
        let keys = commands::Keymap::with(r.commands(), &config.keys).0;
        assert_eq!(keys.label("wc.show"), "c");

        // A typo is still reported, and without the extension its ids are unknown.
        let config = crate::config::parse("[keys]\n\"wc.shwo\" = \"W\"\n", r.commands());
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        assert!(config.errors[0].contains("wc.shwo"));
        let none = registry(&[]);
        let config = crate::config::parse("[keys]\n\"wc.show\" = \"W\"\n", none.commands());
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
    }
}
