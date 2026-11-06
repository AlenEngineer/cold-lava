//! Named commands: every action has an id like `graph.open`, and keys (later the command
//! palette, config and extensions) reach it through that table.

use std::collections::HashMap;
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
    FocusNext,
    FocusPrevious,
    FocusEditor,
    Switcher,
    Search,
    Rename,
    Move,
    ToggleBacklinks,
    Filter,
    ClearFilter,
    Zoom,
    Graph,
    Boards,
    NewNote,
    Today,
    AddVault,
    Rescan,
    OpenInObsidian,
    EditFullscreen,
}

pub struct Command {
    pub action: Action,
    #[allow(dead_code, reason = "read by the command palette and config keys")]
    pub id: &'static str,
    pub title: &'static str,
    pub keys: &'static [&'static str],
}

const fn cmd(
    action: Action,
    id: &'static str,
    title: &'static str,
    keys: &'static [&'static str],
) -> Command {
    Command {
        action,
        id,
        title,
        keys,
    }
}

/// The main menu's commands, in the order the help screen lists them.
pub const MENU: &[Command] = &[
    cmd(
        Action::FocusNext,
        "focus.next",
        "switch between vaults, notes, editor and backlinks",
        &["tab"],
    ),
    cmd(
        Action::FocusPrevious,
        "focus.previous",
        "back to the notes, or from the notes to the vaults",
        &["shift+tab"],
    ),
    cmd(
        Action::FocusEditor,
        "editor.focus",
        "in the menu: back to Neovim",
        &["ctrl+q"],
    ),
    cmd(
        Action::ToggleBacklinks,
        "backlinks.toggle",
        "show / hide backlinks under the editor",
        &["b"],
    ),
    cmd(
        Action::Switcher,
        "note.switch",
        "quick switcher: jump to any note by name",
        &["p", "ctrl+o"],
    ),
    cmd(
        Action::Search,
        "search.open",
        "search the text of every note",
        &["s", "ctrl+f"],
    ),
    cmd(
        Action::Rename,
        "note.rename",
        "rename the note (every link to it is updated)",
        &["R"],
    ),
    cmd(
        Action::Move,
        "note.move",
        "move the note to another folder (links updated)",
        &["m"],
    ),
    cmd(
        Action::EditFullscreen,
        "note.edit",
        "edit the note in $EDITOR, full screen",
        &["e"],
    ),
    cmd(
        Action::Filter,
        "filter.open",
        "filter notes by name or path",
        &["/"],
    ),
    cmd(
        Action::ClearFilter,
        "filter.clear",
        "clear the filter",
        &["esc"],
    ),
    cmd(
        Action::Graph,
        "graph.open",
        "graph view of this vault",
        &["g"],
    ),
    cmd(
        Action::Boards,
        "boards.open",
        "whiteboards of this vault",
        &["w"],
    ),
    cmd(
        Action::NewNote,
        "note.new",
        "new note in the selected folder",
        &["n"],
    ),
    cmd(
        Action::Today,
        "daily.today",
        "today's daily note (created from your template)",
        &["t"],
    ),
    cmd(
        Action::AddVault,
        "vault.add",
        "add a vault (any folder of Markdown files)",
        &["a"],
    ),
    cmd(
        Action::OpenInObsidian,
        "note.open-in-obsidian",
        "open the selected note in Obsidian",
        &["o"],
    ),
    cmd(
        Action::Zoom,
        "editor.zoom",
        "zoom the editor to full width",
        &["z"],
    ),
    cmd(Action::Rescan, "vault.rescan", "rescan the vault", &["r"]),
    cmd(Action::Help, "app.help", "this help", &["?"]),
    cmd(
        Action::Quit,
        "app.quit",
        "quit (notes are saved)",
        &["q", "ctrl+c"],
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    /// Parses "g", "R", "ctrl+o", "shift+tab", "esc", "enter".
    pub fn parse(s: &str) -> Option<Key> {
        let mut mods = KeyModifiers::NONE;
        let mut shift = false;
        let mut rest = s;
        loop {
            if let Some(r) = rest.strip_prefix("ctrl+") {
                mods |= KeyModifiers::CONTROL;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("alt+") {
                mods |= KeyModifiers::ALT;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("shift+") {
                shift = true;
                rest = r;
            } else {
                break;
            }
        }
        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            (Some(c), None) if shift => KeyCode::Char(c.to_uppercase().next()?),
            (Some(c), None) => KeyCode::Char(c),
            _ => match rest.to_ascii_lowercase().as_str() {
                "tab" if shift => KeyCode::BackTab,
                "tab" => KeyCode::Tab,
                "backtab" => KeyCode::BackTab,
                "esc" => KeyCode::Esc,
                "enter" => KeyCode::Enter,
                _ => return None,
            },
        };
        Some(Key { code, mods })
    }
}

impl From<KeyEvent> for Key {
    /// Terminals disagree on whether 'R' or BackTab also carry SHIFT, so SHIFT is dropped:
    /// it is already part of the character or the key code.
    fn from(k: KeyEvent) -> Key {
        Key {
            code: k.code,
            mods: k.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("Ctrl-")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("Alt-")?;
        }
        match self.code {
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::BackTab => f.write_str("Shift-Tab"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Enter => f.write_str("Enter"),
            c => write!(f, "{c:?}"),
        }
    }
}

pub struct Keymap(HashMap<Key, Action>);

impl Keymap {
    pub fn menu() -> Keymap {
        let mut map = HashMap::new();
        for c in MENU {
            for s in c.keys {
                let key = Key::parse(s).unwrap_or_else(|| panic!("bad default key {s:?}"));
                map.insert(key, c.action);
            }
        }
        Keymap(map)
    }

    pub fn get(&self, k: KeyEvent) -> Option<Action> {
        self.0.get(&Key::from(k)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique() {
        let mut seen = HashSet::new();
        for c in MENU {
            assert!(seen.insert(c.id), "duplicate id {}", c.id);
        }
    }

    #[test]
    fn default_keys_parse_and_are_bound_once() {
        let mut seen = HashSet::new();
        for c in MENU {
            for s in c.keys {
                let k = Key::parse(s).unwrap_or_else(|| panic!("{} key {s:?}", c.id));
                assert!(seen.insert(k), "{s:?} is bound twice");
            }
        }
    }

    #[test]
    fn shifted_letter_runs_its_command() {
        let keys = Keymap::menu();
        let shift_r = KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(keys.get(shift_r), Some(Action::Rename));
        let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(keys.get(back_tab), Some(Action::FocusPrevious));
        let plain_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(keys.get(plain_r), Some(Action::Rescan));
    }

    #[test]
    fn ctrl_keys_parse_and_display() {
        let k = Key::parse("ctrl+o").unwrap();
        assert_eq!(k.to_string(), "Ctrl-o");
        let keys = Keymap::menu();
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(keys.get(ctrl_o), Some(Action::Switcher));
        let o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        assert_eq!(keys.get(o), Some(Action::OpenInObsidian));
    }

    #[test]
    fn named_keys_parse() {
        assert_eq!(Key::parse("tab").unwrap().to_string(), "Tab");
        assert_eq!(Key::parse("shift+tab").unwrap().to_string(), "Shift-Tab");
        assert_eq!(Key::parse("esc").unwrap().to_string(), "Esc");
        assert_eq!(Key::parse("enter").unwrap().to_string(), "Enter");
        assert_eq!(Key::parse("?").unwrap().to_string(), "?");
        assert_eq!(Key::parse("bogus"), None);
    }
}
