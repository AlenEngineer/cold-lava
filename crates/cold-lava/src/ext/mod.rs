//! Extensions: what cold-lava does beyond the notes app itself, starting with the editor.
//!
//! This is the JSON-RPC protocol of docs/extensions.md, called in-process: each `Extension`
//! method is one host→extension method, each `Host` method one extension→host method, and every
//! type that crosses derives serde, so extensions in other processes will share this code path.
//! Extensions never see keystrokes and never draw: the core owns the panes they open. What an
//! extension asks for is queued as `Effect`s, which the menu applies after the call returns.

pub mod nvim;

use crate::commands::{self, Entry, MenuCommand};
use crate::core::Jump;
use crate::types::Definitions;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// A loaded extension. Only the host uses it: on the wire, the connection says who is talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// What it adds to the menu's commands, for keys, the palette, the help and config.toml.
    pub commands: Vec<CommandSpec>,
    /// Types it defines, named `<extension name>.<type>` once loaded.
    #[serde(default, skip_serializing_if = "Definitions::is_empty")]
    pub types: Definitions,
}

/// A command an extension registers. Its id is `<extension name>.<something>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    /// Its default keys, written as in config.toml: "g", "ctrl+o".
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
    /// `command/run`: one of its commands, from a key, the palette or an extension.
    fn command(&mut self, _request: CommandRequest, _host: &mut Host<'_>) {}
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
}

pub struct Registry {
    loaded: Vec<(Box<dyn Extension>, Manifest)>,
    /// The one in the editor role, until its program fails to start.
    editor: Option<ExtId>,
    next_pane: u32,
    /// The menu's commands: the core's, then the extensions'.
    commands: Vec<Entry<MenuCommand>>,
    /// Why commands or keys the extensions registered were left out.
    refused: Vec<String>,
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
        let manifests = loaded.iter().enumerate().map(|(i, (_, m))| (ExtId(i), m));
        let (commands, refused) = commands::menu(manifests);
        Registry {
            loaded,
            editor,
            next_pane: 0,
            commands,
            refused,
        }
    }

    pub fn commands(&self) -> &[Entry<MenuCommand>] {
        &self.commands
    }

    pub fn refused(&self) -> &[String] {
        &self.refused
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
                roles: vec![Role::Editor],
                hints: vec![("Ctrl-o".into(), "back".into())],
                commands: vec![command("nvim.restart", &["ctrl+r"])],
                types: Definitions::new(),
            },
            json!({
                "name": "nvim",
                "title": "Neovim",
                "roles": ["editor"],
                "hints": [["Ctrl-o", "back"]],
                "commands": [{"id": "nvim.restart", "title": "does nvim.restart", "keys": ["ctrl+r"]}],
            }),
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
                roles: Vec::new(),
                hints: Vec::new(),
                commands: self.commands.clone(),
                types: Definitions::new(),
            }
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
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
