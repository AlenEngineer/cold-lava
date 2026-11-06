//! `~/.config/cold-lava/config.toml`, read once at startup. A mistake in it never stops the
//! app: the entry falls back to its default and the problem is listed in `errors`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::commands::{BOARD, GRAPH, Key, Keymap, MENU};

#[derive(Default)]
pub struct Config {
    /// Keys by command id, only for ids that exist and keys that parse.
    pub keys: BTreeMap<String, Vec<Key>>,
    /// One human-readable line per problem in the file.
    pub errors: Vec<String>,
}

/// The file as written. Other top-level sections are ignored, for later use.
#[derive(Deserialize)]
struct File {
    #[serde(default)]
    keys: BTreeMap<String, toml::Value>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn path() -> Option<PathBuf> {
    crate::home::config_dir().map(|c| c.join("cold-lava/config.toml"))
}

pub fn load() {
    let _ = CONFIG.set(read(path()));
}

fn read(path: Option<PathBuf>) -> Config {
    match path.map(std::fs::read_to_string) {
        Some(Ok(text)) => parse(&text),
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Config {
            errors: vec![format!("can't read it: {e}")],
            ..Config::default()
        },
        _ => Config::default(),
    }
}

/// The loaded config, or the defaults when `load()` never ran (tests).
pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

pub fn parse(text: &str) -> Config {
    let file: File = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            return Config {
                errors: vec![e.to_string().trim_end().to_string()],
                ..Config::default()
            };
        }
    };
    let ids: HashSet<&str> = MENU
        .iter()
        .map(|c| c.id)
        .chain(GRAPH.iter().map(|c| c.id))
        .chain(BOARD.iter().map(|c| c.id))
        .collect();
    let mut config = Config::default();
    for (id, value) in file.keys {
        if !ids.contains(id.as_str()) {
            config
                .errors
                .push(format!("[keys] {id:?} is not a command"));
            continue;
        }
        match parse_keys(&value) {
            Ok(keys) => {
                config.keys.insert(id, keys);
            }
            Err(e) => config.errors.push(format!("[keys] {id:?}: {e}")),
        }
    }
    config.errors.extend(Keymap::with(MENU, &config.keys).1);
    config.errors.extend(Keymap::with(GRAPH, &config.keys).1);
    config.errors.extend(Keymap::with(BOARD, &config.keys).1);
    config
}

/// `"G"`, `["p", "ctrl+p"]` or `[]`.
fn parse_keys(value: &toml::Value) -> Result<Vec<Key>, String> {
    let strings: Vec<&toml::Value> = match value {
        toml::Value::Array(a) => a.iter().collect(),
        v => vec![v],
    };
    strings
        .into_iter()
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| format!("expected a key like \"g\" or \"ctrl+o\", got {v}"))?;
            Key::parse(s).ok_or_else(|| format!("{s:?} is not a key"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::MenuAction;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn menu(text: &str) -> (Keymap<MenuAction>, Vec<String>) {
        let c = parse(text);
        (Keymap::with(MENU, &c.keys).0, c.errors)
    }

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn override_replaces_the_default_keys() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"G\"\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(keys.get(press('G')), Some(MenuAction::Graph));
        assert_eq!(keys.get(press('g')), None);
        assert_eq!(keys.label("graph.open"), "G");
    }

    #[test]
    fn list_and_empty_list() {
        let (keys, errors) =
            menu("[keys]\n\"note.switch\" = [\"p\", \"ctrl+p\"]\n\"vault.add\" = []\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(keys.label("note.switch"), "p / Ctrl-p");
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(keys.get(ctrl_o), None);
        assert_eq!(keys.label("vault.add"), "");
        assert_eq!(keys.get(press('a')), None);
    }

    #[test]
    fn unknown_command_is_reported_and_the_rest_applies() {
        let (keys, errors) = menu("[keys]\n\"graph.opne\" = \"G\"\n\"vault.add\" = \"A\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("graph.opne"));
        assert_eq!(keys.label("graph.open"), "g");
        assert_eq!(keys.label("vault.add"), "A");
    }

    #[test]
    fn bad_key_is_reported_and_the_default_kept() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = [\"G\", \"hyper+g\"]\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("hyper+g"));
        assert_eq!(keys.label("graph.open"), "g");
        let (_, errors) = menu("[keys]\n\"graph.open\" = 7\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn clash_in_one_screen_keeps_both_defaults() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"x\"\n\"boards.open\" = \"x\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("graph.open") && errors[0].contains("boards.open"));
        assert_eq!(keys.label("graph.open"), "g");
        assert_eq!(keys.label("boards.open"), "w");

        // Taking another command's default key is a clash too.
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"n\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(keys.get(press('n')), Some(MenuAction::NewNote));
        assert_eq!(keys.get(press('g')), Some(MenuAction::Graph));
    }

    #[test]
    fn swapping_two_keys_is_not_a_clash() {
        let (keys, errors) = menu("[keys]\n\"graph.open\" = \"n\"\n\"note.new\" = \"g\"\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(keys.get(press('n')), Some(MenuAction::Graph));
        assert_eq!(keys.get(press('g')), Some(MenuAction::NewNote));
    }

    #[test]
    fn same_key_in_two_screens_is_fine() {
        let c = parse("[keys]\n\"graph.open\" = \"x\"\n\"board.fit\" = \"x\"\n");
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert_eq!(Keymap::with(BOARD, &c.keys).0.label("board.fit"), "x");
    }

    #[test]
    fn syntax_error_keeps_every_default() {
        let (keys, errors) = menu("[keys\n\"graph.open\" = \"G\"\n");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(keys.label("graph.open"), "g");
    }

    #[test]
    fn other_sections_are_allowed() {
        let (_, errors) = menu("[colours]\naccent = \"red\"\n");
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let c = read(Some(PathBuf::from("/nonexistent/cold-lava/config.toml")));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(c.keys.is_empty());
    }
}
