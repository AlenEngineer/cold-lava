//! The menu's one-line prompts: a new folder's name, a folder to add as a workspace, a note's
//! or folder's new name, a folder to move to. Each holds what is typed and what it asks for,
//! with the target it was opened on and the workspace it is in, so what changes under it (the
//! tree's selection, the marks, the workspace open) never changes what Enter does. The new note's prompt and the tag prompt hold a list as well:
//! `new_note.rs` and `tag_prompt.rs`.

use crate::line_input::{self, CtrlU};
use crate::popup;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};

/// Where a prompt's Enter acts, as the prompt holds it: the workspace it was opened in, what
/// Enter does there in a refusal's words (`can't {doing}: …`), and what in it must still be
/// there, relative to `root` (a folder's ends in `/`). The menu checks it before Enter acts
/// (`Home::still_here`).
#[derive(Debug, PartialEq, Eq)]
pub struct At {
    pub root: PathBuf,
    pub doing: String,
    pub needs: Option<String>,
}

impl At {
    /// In the workspace at `root`, needing nothing in particular in it.
    pub fn workspace(root: &Path, doing: &str) -> At {
        At {
            root: root.to_path_buf(),
            doing: doing.to_string(),
            needs: None,
        }
    }

    /// What it needs that is no longer there: a folder that isn't a folder now, a note that
    /// isn't a file.
    pub fn missing(&self) -> Option<&str> {
        let rel = self.needs.as_deref()?;
        let path = self.root.join(rel.trim_end_matches('/'));
        let there = match rel.ends_with('/') {
            true => path.is_dir(),
            false => path.is_file(),
        };
        (!there).then_some(rel)
    }
}

/// The one way a refusal says that `rel`, a note or a folder another program deleted, is gone:
/// by its path in the workspace, as a move's plan names it.
pub fn no_longer_there(rel: &str) -> String {
    format!("{rel} is no longer there")
}

/// How many of `marked`, relative to `root`, are gone since they were marked, as a title says
/// it after their count (` (1 gone)`), or nothing. Enter names them and leaves them alone.
pub fn gone_of(root: &Path, marked: &[String]) -> String {
    match marked.iter().filter(|rel| !root.join(rel).exists()).count() {
        0 => String::new(),
        n => format!(" ({n} gone)"),
    }
}

/// The folder a new note or a new folder goes in, as it was when its prompt opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// The workspace it is in.
    pub root: PathBuf,
    /// Relative to `root`, empty for the root itself.
    pub rel: String,
}

impl Folder {
    /// `name` in it, workspace-relative.
    pub fn join(&self, name: &str) -> String {
        match self.rel.as_str() {
            "" => name.to_string(),
            rel => format!("{rel}/{name}"),
        }
    }

    /// A new folder named `name` in it, workspace-relative.
    pub fn new_folder(&self, name: &str) -> String {
        self.join(name.trim_matches('/'))
    }

    /// A new note named `name` in it, workspace-relative.
    pub fn new_note(&self, name: &str) -> String {
        match name.ends_with(".md") {
            true => self.join(name),
            false => self.join(&format!("{name}.md")),
        }
    }

    /// Where a prompt opened on it acts, `doing` what: in its workspace, the folder still there.
    pub fn at(&self, doing: String) -> At {
        At {
            root: self.root.clone(),
            doing,
            needs: Some(format!("{}/", self.rel)),
        }
    }

    /// How a prompt's title names it.
    pub fn shown(&self) -> String {
        match self.rel.as_str() {
            "" => "the workspace root".to_string(),
            rel => format!("{rel}/"),
        }
    }
}

/// What a prompt asks for, and what it was opened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asks {
    /// A new folder's name, to make in `in_folder`.
    NewFolder { in_folder: Folder },
    /// A folder to add as a workspace.
    AddWorkspace,
    /// `path`'s new name, or with `moving` the folder to move it into, in the workspace at
    /// `root`.
    Rename {
        root: PathBuf,
        path: PathBuf,
        moving: bool,
    },
    /// The folder to move `marked` into: the notes and folders marked when it opened, relative
    /// to `root`.
    MoveMarked { root: PathBuf, marked: Vec<String> },
}

impl Asks {
    /// Where Enter acts with `answer`; adding a workspace acts in none.
    pub fn at(&self, answer: &str) -> Option<At> {
        match self {
            Asks::NewFolder { in_folder } => {
                Some(in_folder.at(format!("create {}/", in_folder.new_folder(answer))))
            }
            Asks::AddWorkspace => None,
            Asks::Rename { root, .. } => Some(At::workspace(root, "rename")),
            Asks::MoveMarked { root, .. } => Some(At::workspace(root, "move")),
        }
    }
}

/// What the menu should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    /// Close, then do what `Asks` asked with this answer: a folder's or a workspace's name
    /// trimmed (never empty), a new name or a folder as typed.
    Done(Asks, String),
}

pub struct Prompt {
    asks: Asks,
    line: String,
}

impl Prompt {
    /// A prompt for `asks`, starting with `line` typed.
    pub fn new(asks: Asks, line: String) -> Prompt {
        Prompt { asks, line }
    }

    /// What closing it would lose, in words for the status line. A prompt holds work with
    /// nothing typed yet: the next keys are meant for it.
    pub fn unsaved(&self) -> &'static str {
        match self.asks {
            Asks::NewFolder { .. } => "a new folder's name is being typed",
            Asks::AddWorkspace => "a workspace's folder is being typed",
            Asks::Rename { moving: false, .. } => "a new name is being typed",
            Asks::Rename { moving: true, .. } | Asks::MoveMarked { .. } => {
                "a folder to move to is being typed"
            }
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Enter => match self.asks {
                Asks::NewFolder { .. } | Asks::AddWorkspace => match self.line.trim() {
                    "" => Action::Close,
                    name => Action::Done(self.asks.clone(), name.to_string()),
                },
                Asks::Rename { .. } | Asks::MoveMarked { .. } => {
                    Action::Done(self.asks.clone(), self.line.clone())
                }
            },
            _ => {
                line_input::key(&mut self.line, k, CtrlU::Clears);
                Action::Stay
            }
        }
    }

    /// Draw it over `area`, its title naming where Enter acts.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let (title, label) = match &self.asks {
            Asks::NewFolder { in_folder } => (
                format!(
                    " new folder in {} · ⏎ create · Esc cancel ",
                    in_folder.shown()
                ),
                "folder name: ",
            ),
            Asks::AddWorkspace => (
                " add a workspace · ⏎ add · Esc cancel ".to_string(),
                "folder: ",
            ),
            Asks::Rename { moving: false, .. } => (
                " rename · ⏎ preview · Esc cancel ".to_string(),
                "new name: ",
            ),
            Asks::Rename { moving: true, .. } => (
                " move to folder · ⏎ preview · Esc cancel ".to_string(),
                "folder: ",
            ),
            Asks::MoveMarked { root, marked } => (
                format!(
                    " move {} marked{} into folder · ⏎ preview · Esc cancel ",
                    marked.len(),
                    gone_of(root, marked)
                ),
                "folder: ",
            ),
        };
        popup::prompt(f, area, &title, label, &self.line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn press(p: &mut Prompt, code: KeyCode) -> Action {
        p.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(asks: Asks, text: &str) -> Prompt {
        let mut p = Prompt::new(asks, String::new());
        for c in text.chars() {
            assert_eq!(press(&mut p, KeyCode::Char(c)), Action::Stay);
        }
        p
    }

    fn in_folder(rel: &str) -> Folder {
        Folder {
            root: PathBuf::from("/w"),
            rel: rel.into(),
        }
    }

    fn folder() -> Asks {
        Asks::NewFolder {
            in_folder: in_folder("Volcanoes"),
        }
    }

    fn move_marked() -> Asks {
        Asks::MoveMarked {
            root: PathBuf::from("/w"),
            marked: vec!["Volcanoes/Krakatoa.md".into(), "People".into()],
        }
    }

    fn rename(moving: bool) -> Asks {
        Asks::Rename {
            root: PathBuf::from("/w"),
            path: PathBuf::from("/w/Volcanoes/Krakatoa.md"),
            moving,
        }
    }

    #[test]
    fn enter_acts_where_it_was_opened() {
        assert_eq!(
            folder().at("/Ash/"),
            Some(At {
                root: PathBuf::from("/w"),
                doing: "create Volcanoes/Ash/".into(),
                needs: Some("Volcanoes/".into()),
            })
        );
        let at_root = Asks::NewFolder {
            in_folder: in_folder(""),
        };
        assert_eq!(at_root.at("Ash").unwrap().needs.as_deref(), Some("/"));
        assert_eq!(Asks::AddWorkspace.at("~/notes"), None);
        assert_eq!(
            rename(true).at("Archive"),
            Some(At::workspace(Path::new("/w"), "rename"))
        );
        assert_eq!(
            move_marked().at("Archive"),
            Some(At::workspace(Path::new("/w"), "move"))
        );
    }

    #[test]
    fn what_it_needs_is_missing_once_it_is_not_what_it_was() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let needs = |rel: &str| At {
            root: here.to_path_buf(),
            doing: "create".into(),
            needs: Some(rel.into()),
        };
        assert_eq!(needs("src/").missing(), None);
        assert_eq!(needs("/").missing(), None, "the workspace's root");
        assert_eq!(needs("Cargo.toml").missing(), None);
        assert_eq!(
            needs("Cargo.toml/").missing(),
            Some("Cargo.toml/"),
            "not a folder"
        );
        assert_eq!(needs("src").missing(), Some("src"), "not a note");
        assert_eq!(needs("gone/").missing(), Some("gone/"));
        assert_eq!(At::workspace(here, "move").missing(), None);
    }

    #[test]
    fn a_title_counts_the_marks_gone_since() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let marked = ["Cargo.toml", "src", "gone.md"].map(String::from);
        assert_eq!(gone_of(here, &marked[..2]), "");
        assert_eq!(gone_of(here, &marked), " (1 gone)");
    }

    #[test]
    fn enter_answers_with_what_it_was_opened_on() {
        let mut p = typed(folder(), "  Calderas ");
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Done(folder(), "Calderas".into()),
            "a folder's name is trimmed"
        );
        let mut p = typed(Asks::AddWorkspace, " ~/notes");
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Done(Asks::AddWorkspace, "~/notes".into())
        );
        let mut p = typed(rename(false), " Krakatau");
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Done(rename(false), " Krakatau".into()),
            "a new name goes as typed"
        );
        let mut p = typed(move_marked(), "");
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Done(move_marked(), String::new()),
            "the move says itself that a folder is wanted"
        );
    }

    #[test]
    fn a_name_left_empty_only_closes() {
        for asks in [folder(), Asks::AddWorkspace] {
            let mut p = typed(asks, "   ");
            assert_eq!(press(&mut p, KeyCode::Enter), Action::Close);
        }
    }

    #[test]
    fn it_starts_with_what_it_was_given_and_takes_the_line_keys() {
        let mut p = Prompt::new(rename(true), "Volcanoes/".into());
        assert_eq!(press(&mut p, KeyCode::Backspace), Action::Stay);
        press(&mut p, KeyCode::Char('!'));
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Done(rename(true), "Volcanoes!".into())
        );
        let mut p = typed(folder(), "Calderas");
        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(p.key(ctrl_u), Action::Stay);
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Close,
            "Ctrl-U cleared it"
        );
        assert_eq!(
            press(&mut typed(folder(), "x"), KeyCode::Esc),
            Action::Close
        );
    }

    #[test]
    fn it_holds_work_with_nothing_typed() {
        let words = |asks| Prompt::new(asks, String::new()).unsaved();
        assert_eq!(words(folder()), "a new folder's name is being typed");
        assert_eq!(
            words(Asks::AddWorkspace),
            "a workspace's folder is being typed"
        );
        assert_eq!(words(rename(false)), "a new name is being typed");
        assert_eq!(words(rename(true)), "a folder to move to is being typed");
        assert_eq!(words(move_marked()), "a folder to move to is being typed");
    }

    fn drawn(p: &Prompt) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| p.draw(f, f.area())).unwrap();
        let buffer = terminal.backend().buffer();
        (0..24u16)
            .map(|y| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn it_draws_its_title_its_label_and_the_line() {
        let shown = |p: &Prompt, title: &str, line: &str| {
            let text = drawn(p);
            assert!(text.iter().any(|r| r.contains(title)), "{title}: {text:#?}");
            assert!(text.iter().any(|r| r.contains(line)), "{line}: {text:#?}");
        };
        let p = typed(folder(), "Calderas");
        shown(
            &p,
            " new folder in Volcanoes/ · ⏎ create",
            "folder name: Calderas▏",
        );
        let p = Prompt::new(
            Asks::NewFolder {
                in_folder: in_folder(""),
            },
            String::new(),
        );
        shown(
            &p,
            " new folder in the workspace root · ⏎ create · Esc cancel",
            "folder name: ▏",
        );
        let p = typed(Asks::AddWorkspace, "~/notes");
        shown(&p, " add a workspace · ⏎ add", "folder: ~/notes▏");
        let p = typed(rename(false), "Krakatau");
        shown(&p, " rename · ⏎ preview", "new name: Krakatau▏");
        let p = typed(rename(true), "Archive/");
        shown(&p, " move to folder · ⏎ preview", "folder: Archive/▏");
        let p = typed(move_marked(), "Archive");
        shown(
            &p,
            " move 2 marked (2 gone) into folder",
            "folder: Archive▏",
        );
    }
}
