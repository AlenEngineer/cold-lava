//! Extensions in the terminal UI. The extension host is `cold-lava-host` (the protocol, the
//! permission check, the registry, extensions in their own process), shared by every frontend
//! and named here as before: `ext::Host`, `ext::Extension`, `ext::installed`. What only the
//! terminal has is here: Neovim in a pane (`nvim`), and the menu's `Registry`, the host's with
//! the menu's command table and who the editor is. `home.rs` applies the `Effect`s.

pub mod nvim;

pub use cold_lava_host::*;

use crate::commands::{self, Entry, MenuCommand};
use std::ops::{Deref, DerefMut};

/// The menu's registry: the host's (`cold_lava_host::Registry`, which it reads as), the
/// menu's commands and the extension in the editor role.
pub struct Registry {
    host: cold_lava_host::Registry,
    /// The one in the editor role, until its program fails to start.
    editor: Option<ExtId>,
    /// The menu's commands: the core's, then the extensions'.
    commands: Vec<Entry<MenuCommand>>,
}

impl Deref for Registry {
    type Target = cold_lava_host::Registry;

    fn deref(&self) -> &cold_lava_host::Registry {
        &self.host
    }
}

impl DerefMut for Registry {
    fn deref_mut(&mut self) -> &mut cold_lava_host::Registry {
        &mut self.host
    }
}

impl Registry {
    /// The built-in extensions that can run here, then the user's (`installed::user_dir`).
    pub fn load() -> Registry {
        let mut found: Vec<Box<dyn Extension>> = Vec::new();
        if let Some(nvim) = nvim::Nvim::detect() {
            found.push(Box::new(nvim));
        }
        found.push(Box::new(crate::agenda::Agenda::default()));
        found.push(Box::new(crate::type_table::Tables::default()));
        found.push(Box::new(crate::graph_ext::Graph::default()));
        let dir = installed::user_dir();
        Registry::with(cold_lava_host::Registry::load(found, dir.as_deref()))
    }

    #[cfg(test)]
    pub fn empty() -> Registry {
        Registry::new(Vec::new())
    }

    #[cfg(test)]
    pub(crate) fn new(extensions: Vec<Box<dyn Extension>>) -> Registry {
        Registry::with(cold_lava_host::Registry::new(extensions))
    }

    /// The menu's registry on the host's: its command table built from the manifests, and
    /// what that left out told to the host.
    fn with(mut host: cold_lava_host::Registry) -> Registry {
        let editor = (host.ids()).find(|e| host.manifest(*e).roles.contains(&Role::Editor));
        let manifests = host.ids().map(|e| (e, host.manifest(e)));
        let (commands, refused) = commands::menu(manifests);
        host.refuse_commands(refused);
        Registry {
            host,
            editor,
            commands,
        }
    }

    pub fn commands(&self) -> &[Entry<MenuCommand>] {
        &self.commands
    }

    pub fn editor(&self) -> Option<ExtId> {
        self.editor
    }

    /// No more editor requests: its program couldn't start.
    pub fn drop_editor(&mut self) {
        self.editor = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(id: &str, keys: &[&str]) -> CommandSpec {
        CommandSpec {
            id: id.into(),
            title: format!("does {id}"),
            keys: keys.iter().map(|k| k.to_string()).collect(),
        }
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
        assert!(
            refused[0].one_line().contains("\"graph.open\"")
                && refused[0].one_line().contains("\"wc.\"")
        );
        assert!(refused[3].one_line().contains("taken"), "{refused:?}");
        assert!(refused[4].one_line().contains("note.rename"), "{refused:?}");
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
        assert!(refused[0].one_line().contains("graph.open"), "{refused:?}");
        assert!(refused[1].one_line().contains("hyper+x"), "{refused:?}");
        assert!(refused[2].one_line().contains("a.one"), "{refused:?}");
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
        assert!(
            config.errors[0].one_line().contains("wc.show")
                && config.errors[0].one_line().contains("graph.open")
        );
        let keys = commands::Keymap::with(r.commands(), &config.keys).0;
        assert_eq!(keys.label("wc.show"), "c");

        // A typo is still reported, and without the extension its ids are unknown.
        let config = crate::config::parse("[keys]\n\"wc.shwo\" = \"W\"\n", r.commands());
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        assert!(config.errors[0].one_line().contains("wc.shwo"));
        let none = registry(&[]);
        let config = crate::config::parse("[keys]\n\"wc.show\" = \"W\"\n", none.commands());
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
    }
}
