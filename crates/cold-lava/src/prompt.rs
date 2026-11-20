//! The menu's one-line prompts: a new folder's name, a folder to add as a workspace, a note's
//! or folder's new name, a folder to move to. Each holds what is typed and what it asks for,
//! with the target it was opened on, so what changes under it (the tree's selection) never
//! changes what Enter does. The new note's prompt and the tag prompt hold a list as well:
//! `new_note.rs` and `tag_prompt.rs`.

use crate::line_input::{self, CtrlU};
use crate::popup;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use std::path::PathBuf;

/// What a prompt asks for, and what it was opened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asks {
    /// A new folder's name, to make in `dir` (workspace-relative, empty for the root).
    NewFolder { dir: String },
    /// A folder to add as a workspace.
    AddWorkspace,
    /// `path`'s new name, or with `moving` the folder to move it into.
    Rename { path: PathBuf, moving: bool },
    /// The folder to move the marked notes and folders into.
    MoveMarked,
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
            Asks::Rename { moving: true, .. } | Asks::MoveMarked => {
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
                Asks::Rename { .. } | Asks::MoveMarked => {
                    Action::Done(self.asks.clone(), self.line.clone())
                }
            },
            _ => {
                line_input::key(&mut self.line, k, CtrlU::Clears);
                Action::Stay
            }
        }
    }

    /// Draw it over `area`. `marked` is how many notes and folders are marked, which the
    /// marked move's title counts.
    pub fn draw(&self, f: &mut Frame, area: Rect, marked: usize) {
        let (title, label) = match self.asks {
            Asks::NewFolder { .. } => (
                " new folder · ⏎ create · Esc cancel ".to_string(),
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
            Asks::MoveMarked => (
                format!(" move {marked} marked into folder · ⏎ preview · Esc cancel "),
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

    fn folder() -> Asks {
        Asks::NewFolder {
            dir: "Volcanoes".into(),
        }
    }

    fn rename(moving: bool) -> Asks {
        Asks::Rename {
            path: PathBuf::from("/w/Volcanoes/Krakatoa.md"),
            moving,
        }
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
        let mut p = typed(Asks::MoveMarked, "");
        assert_eq!(
            press(&mut p, KeyCode::Enter),
            Action::Done(Asks::MoveMarked, String::new()),
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
        assert_eq!(
            words(Asks::MoveMarked),
            "a folder to move to is being typed"
        );
    }

    fn drawn(p: &Prompt, marked: usize) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| p.draw(f, f.area(), marked)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..24u16)
            .map(|y| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn it_draws_its_title_its_label_and_the_line() {
        let shown = |p: &Prompt, marked, title: &str, line: &str| {
            let text = drawn(p, marked);
            assert!(text.iter().any(|r| r.contains(title)), "{title}: {text:#?}");
            assert!(text.iter().any(|r| r.contains(line)), "{line}: {text:#?}");
        };
        let p = typed(folder(), "Calderas");
        shown(&p, 0, " new folder · ⏎ create", "folder name: Calderas▏");
        let p = typed(Asks::AddWorkspace, "~/notes");
        shown(&p, 0, " add a workspace · ⏎ add", "folder: ~/notes▏");
        let p = typed(rename(false), "Krakatau");
        shown(&p, 0, " rename · ⏎ preview", "new name: Krakatau▏");
        let p = typed(rename(true), "Archive/");
        shown(&p, 0, " move to folder · ⏎ preview", "folder: Archive/▏");
        let p = typed(Asks::MoveMarked, "Archive");
        shown(&p, 3, " move 3 marked into folder", "folder: Archive▏");
    }
}
