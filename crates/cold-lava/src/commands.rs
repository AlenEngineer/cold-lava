//! Named commands: every action has an id like `graph.open`, and keys, the command palette,
//! `[keys]` in config.toml and extensions reach it through that table. The menu's table also
//! holds the commands extensions register (`menu`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ext::{ExtId, Manifest};

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
    Read,
    Hints,
    History(HistoryAction),
}

/// Back and forward through the places you went to. They are menu commands that the graph and
/// the whiteboard also run, on the keys their own commands leave free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HistoryAction {
    Back,
    Forward,
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
        "in the menu: back to the editor",
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
        &["p"],
    ),
    cmd(
        MenuAction::Search,
        "search.open",
        "search the text of every note",
        &["s", "ctrl+f"],
    ),
    cmd(
        MenuAction::History(HistoryAction::Back),
        "history.back",
        "back to where you were: a note, a search, the graph, a board",
        &["ctrl+o", "alt+left", "backspace"],
    ),
    cmd(
        MenuAction::History(HistoryAction::Forward),
        "history.forward",
        "forward again, after going back",
        &["alt+right"],
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
        MenuAction::Read,
        "note.read",
        "read the note: switch the pane between the reader and the editor",
        &["v"],
    ),
    cmd(
        MenuAction::Hints,
        "note.hints",
        "link hints: label the links on screen, type a label to follow it",
        &["f"],
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoardAction {
    NewCard,
    NewLinkedCard,
    AddNote,
    NewGroup,
    Connect,
    Edit,
    EditExternal,
    /// Obsidian's colour presets 1 to 6.
    Colour(u8),
    ColourClear,
    Delete,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    Fit,
    OpenInObsidian,
    Reload,
    Escape,
    Help,
    Quit,
}

/// The whiteboard's commands, in the order the help screen lists them. Selecting, moving and
/// resizing cards are plain keys, not commands.
pub const BOARD: &[Command<BoardAction>] = &[
    cmd(
        BoardAction::NewCard,
        "board.new-card",
        "new text card (double-click empty space works too)",
        &["n"],
    ),
    cmd(
        BoardAction::NewLinkedCard,
        "board.new-linked-card",
        "new card linked from the selected one",
        &["a"],
    ),
    cmd(
        BoardAction::AddNote,
        "board.add-note",
        "add a note from the vault as a card",
        &["f"],
    ),
    cmd(
        BoardAction::NewGroup,
        "board.new-group",
        "new group (around the selected card)",
        &["G"],
    ),
    cmd(
        BoardAction::Connect,
        "board.connect",
        "draw an arrow: pick a target, ⏎ (again removes it)",
        &["c"],
    ),
    cmd(
        BoardAction::Edit,
        "board.edit",
        "edit text · open note cards in $EDITOR",
        &["enter", "e"],
    ),
    cmd(
        BoardAction::EditExternal,
        "board.edit-external",
        "edit a text card in $EDITOR",
        &["E"],
    ),
    cmd(
        BoardAction::Colour(1),
        "board.colour-1",
        "colour red (Obsidian preset 1)",
        &["1"],
    ),
    cmd(
        BoardAction::Colour(2),
        "board.colour-2",
        "colour orange (Obsidian preset 2)",
        &["2"],
    ),
    cmd(
        BoardAction::Colour(3),
        "board.colour-3",
        "colour yellow (Obsidian preset 3)",
        &["3"],
    ),
    cmd(
        BoardAction::Colour(4),
        "board.colour-4",
        "colour green (Obsidian preset 4)",
        &["4"],
    ),
    cmd(
        BoardAction::Colour(5),
        "board.colour-5",
        "colour cyan (Obsidian preset 5)",
        &["5"],
    ),
    cmd(
        BoardAction::Colour(6),
        "board.colour-6",
        "colour purple (Obsidian preset 6)",
        &["6"],
    ),
    cmd(
        BoardAction::ColourClear,
        "board.colour-clear",
        "clear colour",
        &["0"],
    ),
    cmd(
        BoardAction::Delete,
        "board.delete",
        "delete the card and its arrows",
        &["d", "delete"],
    ),
    cmd(BoardAction::Undo, "board.undo", "undo", &["u", "ctrl+z"]),
    cmd(
        BoardAction::Redo,
        "board.redo",
        "redo",
        &["U", "ctrl+y", "ctrl+r"],
    ),
    cmd(BoardAction::ZoomIn, "board.zoom-in", "zoom in", &["+", "="]),
    cmd(
        BoardAction::ZoomOut,
        "board.zoom-out",
        "zoom out",
        &["-", "_"],
    ),
    cmd(BoardAction::Fit, "board.fit", "fit everything", &["z"]),
    cmd(
        BoardAction::OpenInObsidian,
        "board.open-in-obsidian",
        "open the note (or this board) in Obsidian",
        &["o"],
    ),
    cmd(
        BoardAction::Reload,
        "board.reload",
        "reload the file from disk",
        &["R"],
    ),
    cmd(BoardAction::Escape, "board.escape", "deselect", &["esc"]),
    cmd(BoardAction::Help, "board.help", "this help", &["?"]),
    cmd(
        BoardAction::Quit,
        "board.quit",
        "leave the whiteboard",
        &["q", "ctrl+c"],
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReaderAction {
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    NextLink,
    PreviousLink,
    Follow,
    Edit,
    Back,
}

/// The reader's commands, when it has focus. Keys it doesn't bind go on to the menu's.
pub const READER: &[Command<ReaderAction>] = &[
    cmd(
        ReaderAction::Down,
        "reader.down",
        "scroll down",
        &["j", "down"],
    ),
    cmd(ReaderAction::Up, "reader.up", "scroll up", &["k", "up"]),
    cmd(
        ReaderAction::PageDown,
        "reader.page-down",
        "page down",
        &["pagedown", "space", "ctrl+d"],
    ),
    cmd(
        ReaderAction::PageUp,
        "reader.page-up",
        "page up",
        &["pageup", "ctrl+u"],
    ),
    cmd(
        ReaderAction::Top,
        "reader.top",
        "to the top",
        &["g", "home"],
    ),
    cmd(
        ReaderAction::Bottom,
        "reader.bottom",
        "to the bottom",
        &["G", "end"],
    ),
    cmd(
        ReaderAction::NextLink,
        "reader.next-link",
        "select the next link",
        &["tab"],
    ),
    cmd(
        ReaderAction::PreviousLink,
        "reader.previous-link",
        "select the previous link",
        &["shift+tab"],
    ),
    cmd(
        ReaderAction::Follow,
        "reader.follow",
        "follow the selected link (creates missing notes)",
        &["enter"],
    ),
    cmd(
        ReaderAction::Edit,
        "reader.edit",
        "edit the note in $EDITOR, full screen",
        &["e"],
    ),
    cmd(
        ReaderAction::Back,
        "reader.back",
        "back to the notes",
        &["esc", "q"],
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneAction {
    Leave,
}

/// The keys the core keeps while a program in a pane has focus; every other key goes to the
/// program. config.toml can move `pane.leave` but not unbind it, so every pane has a way out.
pub const PANE: &[Command<PaneAction>] = &[cmd(
    PaneAction::Leave,
    "pane.leave",
    "leave the pane, back to the menu",
    &["ctrl+q"],
)];

/// Every command id of every screen.
pub fn ids() -> impl Iterator<Item = &'static str> {
    MENU.iter()
        .map(|c| c.id)
        .chain(GRAPH.iter().map(|c| c.id))
        .chain(BOARD.iter().map(|c| c.id))
        .chain(READER.iter().map(|c| c.id))
        .chain(PANE.iter().map(|c| c.id))
}

/// The history commands' keys for a screen of its own (the graph, the whiteboard): the
/// menu's, with config.toml's overrides, less those `screen` binds.
pub fn history_keys<A: Copy>(screen: &Keymap<A>) -> Keymap<HistoryAction> {
    let table: Vec<Entry<HistoryAction>> = entries(MENU)
        .into_iter()
        .filter_map(|e| match e.action {
            MenuAction::History(action) => Some(Entry {
                action,
                id: e.id,
                title: e.title,
                keys: e.keys,
            }),
            _ => None,
        })
        .collect();
    let mut keys = Keymap::configured(&table);
    keys.by_key.retain(|k, _| !screen.by_key.contains_key(k));
    for ks in keys.by_id.values_mut() {
        ks.retain(|k| !screen.by_key.contains_key(k));
    }
    keys
}

/// The history commands as a screen's help lists them: (keys, title).
pub fn history_help(keys: &Keymap<HistoryAction>) -> Vec<(String, &'static str)> {
    MENU.iter()
        .filter(|c| matches!(c.action, MenuAction::History(_)))
        .map(|c| (keys.label(c.id), c.title))
        .collect()
}

/// What a menu command runs: one of the core's, or the `i`th command extension `ext` registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuCommand {
    Core(MenuAction),
    Ext(ExtId, usize),
}

/// A command as keymaps, the palette and the help read it. Unlike `Command`, an extension can
/// make one at runtime.
pub struct Entry<A> {
    pub action: A,
    pub id: String,
    pub title: String,
    /// Its default keys.
    pub keys: Vec<Key>,
}

pub fn entries<A: Copy>(table: &[Command<A>]) -> Vec<Entry<A>> {
    table
        .iter()
        .map(|c| Entry {
            action: c.action,
            id: c.id.to_string(),
            title: c.title.to_string(),
            keys: c.keys.iter().map(|s| default_key(c.id, s)).collect(),
        })
        .collect()
}

/// The menu's commands: `MENU`, then those of each extension, in order. A command is left out
/// when its id isn't `<extension name>.<something>` or is already a command; a default key that
/// doesn't parse or that an earlier command has is dropped. Each is reported.
pub fn menu<'a>(
    extensions: impl IntoIterator<Item = (ExtId, &'a Manifest)>,
) -> (Vec<Entry<MenuCommand>>, Vec<String>) {
    let mut table: Vec<Entry<MenuCommand>> = entries(MENU)
        .into_iter()
        .map(|e| Entry {
            action: MenuCommand::Core(e.action),
            id: e.id,
            title: e.title,
            keys: e.keys,
        })
        .collect();
    let mut ids: HashSet<String> = self::ids().map(String::from).collect();
    let mut refused = Vec::new();
    for (ext, m) in extensions {
        for (i, c) in m.commands.iter().enumerate() {
            let (name, id) = (&m.name, &c.id);
            let namespaced = id
                .strip_prefix(name.as_str())
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some_and(|rest| !rest.is_empty());
            if !namespaced {
                refused.push(format!(
                    "{name}: command {id:?} was left out: its id must start with \"{name}.\""
                ));
                continue;
            }
            if !ids.insert(id.clone()) {
                refused.push(format!(
                    "{name}: command {id:?} was left out: that id is taken"
                ));
                continue;
            }
            let mut keys = Vec::new();
            for s in &c.keys {
                let Some(k) = Key::parse(s) else {
                    refused.push(format!("{id}: {s:?} is not a key; it was dropped"));
                    continue;
                };
                match table.iter().find(|e| e.keys.contains(&k)) {
                    Some(owner) => refused.push(format!(
                        "{id}: \"{k}\" is already {}'s key; it was dropped",
                        owner.id
                    )),
                    None => keys.push(k),
                }
            }
            table.push(Entry {
                action: MenuCommand::Ext(ext, i),
                id: id.clone(),
                title: c.title.clone(),
                keys,
            });
        }
    }
    (table, refused)
}

/// A palette row: the command and the matched char positions in its title or its id.
pub struct Match<'a> {
    pub command: &'a Entry<MenuCommand>,
    pub title_hits: Vec<usize>,
    pub id_hits: Vec<usize>,
}

/// The palette's list for `query` from the menu's commands, best first, scored on the better of
/// title and id. An empty query lists every command in table order. The palette itself is left
/// out.
pub fn matching<'a>(table: &'a [Entry<MenuCommand>], query: &str) -> Vec<Match<'a>> {
    let mut scored: Vec<(i64, Match)> = table
        .iter()
        .filter(|c| c.action != MenuCommand::Core(MenuAction::Palette))
        .filter_map(|c| {
            let title = crate::search::fuzzy(query, &c.title);
            let id = crate::search::fuzzy(query, &c.id);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    /// Parses "g", "R", "ctrl+o", "shift+tab", "esc", "enter", "space", "backspace", "delete",
    /// "up", "down", "left", "right", "pageup", "pagedown", "home", "end".
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
                "delete" => KeyCode::Delete,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "pageup" => KeyCode::PageUp,
                "pagedown" => KeyCode::PageDown,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                _ => return None,
            },
        };
        Some(Key { code, mods })
    }

    /// What a terminal sends for this key. `app_cursor`: the program asked for application
    /// cursor keys (arrows as `ESC O A`).
    pub fn bytes(&self, app_cursor: bool) -> Vec<u8> {
        crate::embed::encode_key(KeyEvent::new(self.code, self.mods), app_cursor)
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
            KeyCode::Delete => f.write_str("Delete"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            c => write!(f, "{c:?}"),
        }
    }
}

/// A screen's effective keys: its table's defaults with the overrides from config.toml.
pub struct Keymap<A> {
    by_key: HashMap<Key, A>,
    by_id: HashMap<String, Vec<Key>>,
}

impl<A: Copy> Keymap<A> {
    pub fn new(table: &[Command<A>]) -> Keymap<A> {
        Keymap::configured(&entries(table))
    }

    /// `table`'s keys with config.toml's overrides.
    pub fn configured(table: &[Entry<A>]) -> Keymap<A> {
        Keymap::with(table, &crate::config::get().keys).0
    }

    /// Applies `overrides` (keys by command id) to `table`. When a key would end up on two
    /// commands, neither override is applied: both commands keep their default keys, and the
    /// clash is reported. Repeats until nothing clashes, since a restored default can clash
    /// with a third override.
    pub fn with(
        table: &[Entry<A>],
        overrides: &BTreeMap<String, Vec<Key>>,
    ) -> (Keymap<A>, Vec<String>) {
        let mut applied: HashSet<&str> = table
            .iter()
            .map(|c| c.id.as_str())
            .filter(|id| overrides.contains_key(*id))
            .collect();
        let mut errors = Vec::new();
        loop {
            let keys: Vec<Vec<Key>> = table
                .iter()
                .map(|c| match overrides.get(&c.id) {
                    Some(keys) if applied.contains(c.id.as_str()) => keys.clone(),
                    _ => c.keys.clone(),
                })
                .collect();
            let Some((a, b, key)) = first_clash(&keys) else {
                let mut by_key = HashMap::new();
                let mut by_id = HashMap::new();
                for (c, keys) in table.iter().zip(keys) {
                    for &k in &keys {
                        by_key.insert(k, c.action);
                    }
                    by_id.insert(c.id.clone(), keys);
                }
                return (Keymap { by_key, by_id }, errors);
            };
            let (a, b) = (table[a].id.as_str(), table[b].id.as_str());
            errors.push(format!(
                "[keys] \"{key}\" is bound to both {a} and {b}; both keep their default keys"
            ));
            let removed = applied.remove(a) | applied.remove(b);
            assert!(removed, "default keys of {a} and {b} clash");
        }
    }

    pub fn get(&self, k: KeyEvent) -> Option<A> {
        self.by_key.get(&Key::from(k)).copied()
    }

    pub fn keys(&self, id: &str) -> &[Key] {
        self.by_id.get(id).map_or(&[], Vec::as_slice)
    }

    /// A command's keys as shown to the user: "s / Ctrl-f".
    pub fn label(&self, id: &str) -> String {
        let keys: Vec<String> = self.keys(id).iter().map(Key::to_string).collect();
        keys.join(" / ")
    }
}

fn default_key(id: &str, s: &str) -> Key {
    Key::parse(s).unwrap_or_else(|| panic!("{id}: bad default key {s:?}"))
}

/// The first key held by two different commands, as (earlier command, later command, key).
fn first_clash(keys: &[Vec<Key>]) -> Option<(usize, usize, Key)> {
    let mut owner = HashMap::new();
    for (i, ks) in keys.iter().enumerate() {
        for &k in ks {
            match owner.insert(k, i) {
                Some(j) if j != i => return Some((j, i, k)),
                _ => {}
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_across_screens() {
        let mut seen = HashSet::new();
        for id in ids() {
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
        assert_keys_parse_and_are_bound_once(BOARD);
        assert_keys_parse_and_are_bound_once(READER);
        assert_keys_parse_and_are_bound_once(PANE);
    }

    #[test]
    fn pane_keeps_only_its_leave_key() {
        let keys = Keymap::new(PANE);
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(keys.get(ctrl_q), Some(PaneAction::Leave));
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(keys.get(q), None, "goes to the program");
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(keys.get(esc), None, "goes to the program");
    }

    #[test]
    fn reader_keys_run_their_commands() {
        let keys = Keymap::new(READER);
        let press = |code| keys.get(KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(press(KeyCode::Char('j')), Some(ReaderAction::Down));
        assert_eq!(press(KeyCode::Down), Some(ReaderAction::Down));
        assert_eq!(press(KeyCode::PageUp), Some(ReaderAction::PageUp));
        assert_eq!(press(KeyCode::End), Some(ReaderAction::Bottom));
        assert_eq!(press(KeyCode::Tab), Some(ReaderAction::NextLink));
        assert_eq!(press(KeyCode::Esc), Some(ReaderAction::Back));
        assert_eq!(press(KeyCode::Char('p')), None, "left to the menu");
    }

    #[test]
    fn board_keys_run_their_commands() {
        let keys = Keymap::new(BOARD);
        let press = |code, mods| keys.get(KeyEvent::new(code, mods));
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(press(KeyCode::Char('n'), none), Some(BoardAction::NewCard));
        let shift_g = press(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(shift_g, Some(BoardAction::NewGroup));
        assert_eq!(press(KeyCode::Char('z'), ctrl), Some(BoardAction::Undo));
        assert_eq!(press(KeyCode::Char('z'), none), Some(BoardAction::Fit));
        assert_eq!(press(KeyCode::Char('r'), ctrl), Some(BoardAction::Redo));
        assert_eq!(press(KeyCode::Char('r'), none), None);
        assert_eq!(
            press(KeyCode::Char('3'), none),
            Some(BoardAction::Colour(3))
        );
        assert_eq!(press(KeyCode::Delete, none), Some(BoardAction::Delete));
        assert_eq!(press(KeyCode::Esc, none), Some(BoardAction::Escape));
        assert_eq!(press(KeyCode::Char('q'), none), Some(BoardAction::Quit));
        assert_eq!(press(KeyCode::Char('h'), none), None);
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
        let back = MenuAction::History(HistoryAction::Back);
        assert_eq!(keys.get(ctrl_o), Some(back));
        let o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        assert_eq!(keys.get(o), Some(MenuAction::OpenInObsidian));
    }

    #[test]
    fn palette_finds_commands_by_name() {
        let (table, _) = menu([]);
        let ids = |q: &str| -> Vec<String> {
            let hits = matching(&table, q);
            hits.iter().map(|m| m.command.id.clone()).collect()
        };
        assert_eq!(ids("graph")[0], "graph.open");
        assert_eq!(ids("rnm")[0], "note.rename");
        assert_eq!(ids("daily.today")[0], "daily.today");
        assert!(ids("zzzz").is_empty());
    }

    #[test]
    fn empty_palette_lists_every_other_command_in_order() {
        let (table, _) = menu([]);
        let ids: Vec<&str> = matching(&table, "")
            .iter()
            .map(|m| m.command.id.as_str())
            .collect();
        let menu: Vec<&str> = MENU
            .iter()
            .map(|c| c.id)
            .filter(|&id| id != "app.palette")
            .collect();
        assert_eq!(ids, menu);
    }

    #[test]
    fn history_keys_leave_the_screens_own_keys_alone() {
        let graph = history_keys(&Keymap::new(GRAPH));
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(graph.get(backspace), None, "stays graph.back");
        let alt_left = KeyEvent::new(KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(graph.get(alt_left), Some(HistoryAction::Back));
        assert_eq!(graph.label("history.back"), "Ctrl-o / Alt-←");
        let board = history_keys(&Keymap::new(BOARD));
        assert_eq!(board.get(backspace), Some(HistoryAction::Back));
        let alt_right = KeyEvent::new(KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(board.get(alt_right), Some(HistoryAction::Forward));
        let reader = Keymap::new(READER);
        for k in [backspace, alt_left, alt_right] {
            assert_eq!(reader.get(k), None, "the reader leaves {k:?} to the menu");
        }
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
        assert_eq!(Key::parse("delete").unwrap().to_string(), "Delete");
        assert_eq!(Key::parse("bogus"), None);
    }
}
