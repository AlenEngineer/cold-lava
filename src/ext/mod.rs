//! Extensions: what cold-lava does beyond the notes app itself, starting with the editor.
//!
//! This is the JSON-RPC protocol of docs/extensions.md, called in-process: each `Extension`
//! method is one host→extension method, each `Host` method one extension→host method, and every
//! type that crosses derives serde, so extensions in other processes will share this code path.
//! Extensions never see keystrokes and never draw: the core owns the panes they open. What an
//! extension asks for is queued as `Effect`s, which the menu applies after the call returns.

pub mod nvim;

use crate::core::Jump;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// A loaded extension. Only the host uses it: on the wire, the connection says who is talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtId(usize);

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
    pub roles: Vec<Role>,
    /// Keys its program understands, as (key, what it does), for the status line and the help.
    pub hints: Vec<(String, String)>,
}

/// A part of cold-lava an extension provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Opens notes for editing: gets the `editor/*` requests.
    Editor,
}

/// A program to run in a pane, in the vault's folder.
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

/// What the core asks of the editor (host → extension). Paths are absolute, inside the vault.
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
}

/// What an extension asks of the core (extension → host).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Effect {
    /// Show `message` in the status line.
    #[serde(rename = "ui/notify")]
    Notify { message: String },
    /// The editor shows `note` now, whoever asked for it.
    #[serde(rename = "editor/opened")]
    EditorOpened { note: PathBuf },
    /// Run a cold-lava command, such as `note.follow-link`.
    #[serde(rename = "command/run")]
    RunCommand { id: String, args: Value },
    /// `note` was just written: read it now instead of when the watcher notices.
    #[serde(rename = "vault/saved")]
    Saved { note: PathBuf },
    #[serde(rename = "pane/open")]
    OpenPane { pane: PaneId, spec: PaneSpec },
    #[serde(rename = "pane/input")]
    PaneInput { pane: PaneId, text: String },
}

/// An extension built into cold-lava. Its methods run on the UI thread: they must return at once
/// and never wait for a pane.
pub trait Extension {
    /// `initialize`: who it is.
    fn initialize(&mut self) -> Manifest;
    /// `pane/message`, `pane/exited`: news from a pane it opened.
    fn pane(&mut self, news: PaneNews, host: &mut Host<'_>);
    /// `editor/open`, `editor/save`, `editor/renamed`, for the extension in the editor role.
    fn editor(&mut self, request: EditorRequest, host: &mut Host<'_>);
}

/// What an extension can ask of the core during a call. Nothing happens until the call returns.
pub struct Host<'a> {
    effects: Vec<Effect>,
    next_pane: &'a mut u32,
}

impl Host<'_> {
    /// `ui/notify`
    pub fn notify(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.effects.push(Effect::Notify { message });
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

    /// `vault/saved`
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
}

pub struct Registry {
    loaded: Vec<(Box<dyn Extension>, Manifest)>,
    /// The one in the editor role, until its program fails to start.
    editor: Option<ExtId>,
    next_pane: u32,
}

impl Registry {
    /// The built-in extensions that can run here.
    pub fn builtin() -> Registry {
        let mut found: Vec<Box<dyn Extension>> = Vec::new();
        if let Some(nvim) = nvim::Nvim::detect() {
            found.push(Box::new(nvim));
        }
        Registry::new(found)
    }

    fn new(extensions: Vec<Box<dyn Extension>>) -> Registry {
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
        Registry {
            loaded,
            editor,
            next_pane: 0,
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

    /// Call one extension and return what it asked for, for the caller to apply.
    pub fn call(
        &mut self,
        ext: ExtId,
        f: impl FnOnce(&mut dyn Extension, &mut Host<'_>),
    ) -> Vec<Effect> {
        let mut host = Host {
            effects: Vec::new(),
            next_pane: &mut self.next_pane,
        };
        f(self.loaded[ext.0].0.as_mut(), &mut host);
        host.effects
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
            json!({"method": "vault/saved", "params": {"note": "/v/Lava.md"}}),
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
    fn manifest_is_json() {
        wire(
            Manifest {
                name: "nvim".into(),
                title: "Neovim".into(),
                roles: vec![Role::Editor],
                hints: vec![("Ctrl-o".into(), "back".into())],
            },
            json!({"name": "nvim", "title": "Neovim", "roles": ["editor"], "hints": [["Ctrl-o", "back"]]}),
        );
    }
}
