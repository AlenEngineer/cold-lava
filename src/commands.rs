//! Named commands: every action has an id like `graph.open`, and keys and the command palette
//! (later config and extensions) reach it through that table.

use std::collections::HashMap;
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuAction {
    Quit,
    Help,
    FocusNext,
    FocusPrevious,
    FocusEditor,
    Palette,
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

pub struct Command<A> {
    pub action: A,
    pub id: &'static str,
    pub title: &'static str,
    pub keys: &'static [&'static str],
}

const fn cmd<A>(
    action: A,
    id: &'static str,
    title: &'static str,
    keys: &'static [&'static str],
) -> Command<A> {
    Command {
        action,
        id,
        title,
        keys,
    }
}

/// The main menu's commands, in the order the help screen lists them.
pub const MENU: &[Command<MenuAction>] = &[
    cmd(
        MenuAction::FocusNext,
        "focus.next",
        "switch between vaults, notes, editor and backlinks",
        &["tab"],
    ),
    cmd(
        MenuAction::FocusPrevious,
        "focus.previous",
        "back to the notes, or from the notes to the vaults",
        &["shift+tab"],
    ),
    cmd(
        MenuAction::Palette,
        "app.palette",
        "command palette: run any command by name",
        &[":"],
    ),
    cmd(
        MenuAction::FocusEditor,
        "editor.focus",
        "in the menu: back to Neovim",
        &["ctrl+q"],
    ),
    cmd(
        MenuAction::ToggleBacklinks,
        "backlinks.toggle",
        "show / hide backlinks under the editor",
        &["b"],
    ),
    cmd(
        MenuAction::Switcher,
        "note.switch",
        "quick switcher: jump to any note by name",
        &["p", "ctrl+o"],
    ),
    cmd(
        MenuAction::Search,
        "search.open",
        "search the text of every note",
        &["s", "ctrl+f"],
    ),
    cmd(
        MenuAction::Rename,
        "note.rename",
        "rename the note (every link to it is updated)",
        &["R"],
    ),
    cmd(
        MenuAction::Move,
        "note.move",
        "move the note to another folder (links updated)",
        &["m"],
    ),
    cmd(
        MenuAction::EditFullscreen,
        "note.edit",
        "edit the note in $EDITOR, full screen",
        &["e"],
    ),
    cmd(
        MenuAction::Filter,
        "filter.open",
        "filter notes by name or path",
        &["/"],
    ),
    cmd(
        MenuAction::ClearFilter,
        "filter.clear",
        "clear the filter",
        &["esc"],
    ),
    cmd(
        MenuAction::Graph,
        "graph.open",
        "graph view of this vault",
        &["g"],
    ),
    cmd(
        MenuAction::Boards,
        "boards.open",
        "whiteboards of this vault",
        &["w"],
    ),
    cmd(
        MenuAction::NewNote,
        "note.new",
        "new note in the selected folder",
        &["n"],
    ),
    cmd(
        MenuAction::Today,
        "daily.today",
        "today's daily note (created from your template)",
        &["t"],
    ),
    cmd(
        MenuAction::AddVault,
        "vault.add",
        "add a vault (any folder of Markdown files)",
        &["a"],
    ),
    cmd(
        MenuAction::OpenInObsidian,
        "note.open-in-obsidian",
        "open the selected note in Obsidian",
        &["o"],
    ),
    cmd(
        MenuAction::Zoom,
        "editor.zoom",
        "zoom the editor to full width",
        &["z"],
    ),
    cmd(
        MenuAction::Rescan,
        "vault.rescan",
        "rescan the vault",
        &["r"],
    ),
    cmd(MenuAction::Help, "app.help", "this help", &["?"]),
    cmd(
        MenuAction::Quit,
        "app.quit",
        "quit (notes are saved)",
        &["q", "ctrl+c"],
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphAction {
    CycleLinks,
    CycleLinksBack,
    Back,
    Search,
    NextMatch,
    PreviousMatch,
    OpenNote,
    OpenInObsidian,
    Boards,
    Local,
    DepthMore,
    DepthLess,
    Groups,
    Pixels,
    ZoomIn,
    ZoomOut,
    Fit,
    Center,
    ToggleTags,
    ToggleUnresolved,
    ToggleOrphans,
    Labels,
    Pause,
    Rescan,
    Escape,
    Help,
    Quit,
}

/// The graph view's commands, in the order the help screen lists them. Moving the selection
/// and panning are plain keys, not commands.
pub const GRAPH: &[Command<GraphAction>] = &[
    cmd(
        GraphAction::CycleLinks,
        "graph.cycle-links",
        "cycle through the selected note's links",
        &["tab"],
    ),
    cmd(
        GraphAction::CycleLinksBack,
        "graph.cycle-links-back",
        "cycle back through the selected note's links",
        &["shift+tab"],
    ),
    cmd(
        GraphAction::Back,
        "graph.back",
        "go back to the previous selection",
        &["b", "backspace"],
    ),
    cmd(GraphAction::Search, "graph.search", "search notes", &["/"]),
    cmd(
        GraphAction::NextMatch,
        "graph.next-match",
        "jump to the next match",
        &["n"],
    ),
    cmd(
        GraphAction::PreviousMatch,
        "graph.previous-match",
        "jump to the previous match",
        &["N"],
    ),
    cmd(
        GraphAction::OpenNote,
        "graph.open-note",
        "open in $EDITOR (creates unresolved notes)",
        &["enter", "e"],
    ),
    cmd(
        GraphAction::OpenInObsidian,
        "graph.open-in-obsidian",
        "open the note in the Obsidian app",
        &["o"],
    ),
    cmd(
        GraphAction::Boards,
        "graph.boards",
        "whiteboards (Obsidian .canvas files)",
        &["w"],
    ),
    cmd(
        GraphAction::Local,
        "graph.local",
        "local graph around the selection",
        &["f"],
    ),
    cmd(
        GraphAction::DepthMore,
        "graph.depth-more",
        "local graph one link deeper",
        &["]"],
    ),
    cmd(
        GraphAction::DepthLess,
        "graph.depth-less",
        "local graph one link shallower",
        &["["],
    ),
    cmd(
        GraphAction::Groups,
        "graph.groups",
        "colour groups: obsidian → folder → tag → off",
        &["g"],
    ),
    cmd(
        GraphAction::Pixels,
        "graph.pixels",
        "pixel rendering (Kitty graphics) ↔ braille",
        &["p"],
    ),
    cmd(GraphAction::ZoomIn, "graph.zoom-in", "zoom in", &["+", "="]),
    cmd(
        GraphAction::ZoomOut,
        "graph.zoom-out",
        "zoom out",
        &["-", "_"],
    ),
    cmd(GraphAction::Fit, "graph.fit", "fit graph to screen", &["0"]),
    cmd(
        GraphAction::Center,
        "graph.center",
        "centre on selection",
        &["c"],
    ),
    cmd(
        GraphAction::ToggleTags,
        "graph.toggle-tags",
        "show / hide tags",
        &["t"],
    ),
    cmd(
        GraphAction::ToggleUnresolved,
        "graph.toggle-unresolved",
        "show / hide unresolved links",
        &["u"],
    ),
    cmd(
        GraphAction::ToggleOrphans,
        "graph.toggle-orphans",
        "show / hide orphans",
        &["O"],
    ),
    cmd(
        GraphAction::Labels,
        "graph.labels",
        "labels: auto → all → selection only",
        &["a"],
    ),
    cmd(
        GraphAction::Pause,
        "graph.pause",
        "pause / resume the layout",
        &["space"],
    ),
    cmd(
        GraphAction::Rescan,
        "graph.rescan",
        "rescan the vault",
        &["r"],
    ),
    cmd(
        GraphAction::Escape,
        "graph.escape",
        "leave local graph, clear selection",
        &["esc"],
    ),
    cmd(GraphAction::Help, "graph.help", "this help", &["?"]),
    cmd(GraphAction::Quit, "graph.quit", "quit", &["q", "ctrl+c"]),
];

/// A palette row: the command and the matched char positions in its title or its id.
pub struct Match {
    pub command: &'static Command<MenuAction>,
    pub title_hits: Vec<usize>,
    pub id_hits: Vec<usize>,
}

/// The palette's list for `query`, best first, scored on the better of title and id. An empty
/// query lists every command in `MENU` order. The palette itself is left out.
pub fn matching(query: &str) -> Vec<Match> {
    let mut scored: Vec<(i64, Match)> = MENU
        .iter()
        .filter(|c| c.action != MenuAction::Palette)
        .filter_map(|c| {
            let title = crate::search::fuzzy(query, c.title);
            let id = crate::search::fuzzy(query, c.id);
            let (score, title_hits, id_hits) = match (title, id) {
                (Some(t), Some(i)) if i.score > t.score => (i.score, Vec::new(), i.positions),
                (Some(t), _) => (t.score, t.positions, Vec::new()),
                (None, Some(i)) => (i.score, Vec::new(), i.positions),
                (None, None) => return None,
            };
            Some((
                score,
                Match {
                    command: c,
                    title_hits,
                    id_hits,
                },
            ))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, m)| m).collect()
}

/// A command's keys as shown to the user: "p / Ctrl-o".
pub fn keys_label<A>(c: &Command<A>) -> String {
    let keys: Vec<String> = c
        .keys
        .iter()
        .filter_map(|s| Key::parse(s))
        .map(|k| k.to_string())
        .collect();
    keys.join(" / ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    /// Parses "g", "R", "ctrl+o", "shift+tab", "esc", "enter", "space", "backspace".
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
                "space" => KeyCode::Char(' '),
                "backspace" => KeyCode::Backspace,
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
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::BackTab => f.write_str("Shift-Tab"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Backspace => f.write_str("Backspace"),
            c => write!(f, "{c:?}"),
        }
    }
}

pub struct Keymap<A>(HashMap<Key, A>);

impl<A: Copy> Keymap<A> {
    pub fn new(commands: &[Command<A>]) -> Keymap<A> {
        let mut map = HashMap::new();
        for c in commands {
            for s in c.keys {
                let key = Key::parse(s).unwrap_or_else(|| panic!("bad default key {s:?}"));
                map.insert(key, c.action);
            }
        }
        Keymap(map)
    }

    pub fn get(&self, k: KeyEvent) -> Option<A> {
        self.0.get(&Key::from(k)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_across_screens() {
        let ids = MENU.iter().map(|c| c.id).chain(GRAPH.iter().map(|c| c.id));
        let mut seen = HashSet::new();
        for id in ids {
            assert!(seen.insert(id), "duplicate id {id}");
        }
    }

    fn assert_keys_parse_and_are_bound_once<A>(table: &[Command<A>]) {
        let mut seen = HashSet::new();
        for c in table {
            for s in c.keys {
                let k = Key::parse(s).unwrap_or_else(|| panic!("{} key {s:?}", c.id));
                assert!(seen.insert(k), "{s:?} is bound twice");
            }
        }
    }

    #[test]
    fn default_keys_parse_and_are_bound_once() {
        assert_keys_parse_and_are_bound_once(MENU);
        assert_keys_parse_and_are_bound_once(GRAPH);
    }

    #[test]
    fn graph_keys_run_their_commands() {
        let keys = Keymap::new(GRAPH);
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(
            keys.get(press(KeyCode::Char('f'))),
            Some(GraphAction::Local)
        );
        assert_eq!(
            keys.get(press(KeyCode::Char(' '))),
            Some(GraphAction::Pause)
        );
        assert_eq!(keys.get(press(KeyCode::Backspace)), Some(GraphAction::Back));
        let shift_o = KeyEvent::new(KeyCode::Char('O'), KeyModifiers::SHIFT);
        assert_eq!(keys.get(shift_o), Some(GraphAction::ToggleOrphans));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(keys.get(ctrl_c), Some(GraphAction::Quit));
        assert_eq!(
            keys.get(press(KeyCode::Char('c'))),
            Some(GraphAction::Center)
        );
        assert_eq!(keys.get(press(KeyCode::Char('h'))), None);
    }

    #[test]
    fn shifted_letter_runs_its_command() {
        let keys = Keymap::new(MENU);
        let shift_r = KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(keys.get(shift_r), Some(MenuAction::Rename));
        let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(keys.get(back_tab), Some(MenuAction::FocusPrevious));
        let plain_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(keys.get(plain_r), Some(MenuAction::Rescan));
    }

    #[test]
    fn ctrl_keys_parse_and_display() {
        let k = Key::parse("ctrl+o").unwrap();
        assert_eq!(k.to_string(), "Ctrl-o");
        let keys = Keymap::new(MENU);
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(keys.get(ctrl_o), Some(MenuAction::Switcher));
        let o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        assert_eq!(keys.get(o), Some(MenuAction::OpenInObsidian));
    }

    #[test]
    fn palette_finds_commands_by_name() {
        let ids = |q: &str| -> Vec<&str> { matching(q).iter().map(|m| m.command.id).collect() };
        assert_eq!(ids("graph")[0], "graph.open");
        assert_eq!(ids("rnm")[0], "note.rename");
        assert_eq!(ids("daily.today")[0], "daily.today");
        assert!(ids("zzzz").is_empty());
    }

    #[test]
    fn empty_palette_lists_every_other_command_in_order() {
        let ids: Vec<&str> = matching("").iter().map(|m| m.command.id).collect();
        let menu: Vec<&str> = MENU
            .iter()
            .map(|c| c.id)
            .filter(|&id| id != "app.palette")
            .collect();
        assert_eq!(ids, menu);
    }

    #[test]
    fn named_keys_parse() {
        assert_eq!(Key::parse("tab").unwrap().to_string(), "Tab");
        assert_eq!(Key::parse("shift+tab").unwrap().to_string(), "Shift-Tab");
        assert_eq!(Key::parse("esc").unwrap().to_string(), "Esc");
        assert_eq!(Key::parse("enter").unwrap().to_string(), "Enter");
        assert_eq!(Key::parse("?").unwrap().to_string(), "?");
        assert_eq!(Key::parse("space").unwrap().to_string(), "space");
        assert_eq!(Key::parse("backspace").unwrap().to_string(), "Backspace");
        assert_eq!(Key::parse("bogus"), None);
    }
}
