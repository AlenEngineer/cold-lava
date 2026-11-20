//! The new note's prompt: its name, and which of the workspace's templates it starts from. It
//! holds the folder it was opened on, and its title names it, so the note goes where the
//! prompt says whatever is selected by Enter.

use crate::app::{ACCENT, ACCENT_SOFT, MUTED, NOTE};
use crate::line_input::{self, CtrlU};
use crate::popup;
use crate::prompt::Folder;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

/// What the menu should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    /// Close, then create the note `name` (trimmed, never empty) in `in_folder`, from
    /// `template` (workspace-relative, without `.md`) when one was chosen.
    Create {
        in_folder: Folder,
        name: String,
        template: Option<String>,
    },
}

pub struct NewNote {
    /// Where the note goes.
    in_folder: Folder,
    name: String,
    /// The workspace's templates: workspace-relative without `.md`, and the name shown.
    templates: Vec<(String, String)>,
    /// The chosen one's place in `templates`, `None` for none.
    template: Option<usize>,
}

impl NewNote {
    pub fn new(in_folder: Folder, templates: Vec<(String, String)>) -> NewNote {
        NewNote {
            in_folder,
            name: String::new(),
            templates,
            template: None,
        }
    }

    /// What closing it would lose, in words for the status line. It holds work with nothing
    /// typed yet: the next keys are meant for it.
    pub fn unsaved(&self) -> &'static str {
        "a new note's name is being typed"
    }

    /// Tab and the arrows choose a template, and go round through "No template" at the ends.
    pub fn key(&mut self, k: KeyEvent) -> Action {
        let n = self.templates.len();
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Tab | KeyCode::Down if n > 0 => {
                self.template = match self.template {
                    None => Some(0),
                    Some(i) if i + 1 < n => Some(i + 1),
                    Some(_) => None,
                };
                Action::Stay
            }
            KeyCode::BackTab | KeyCode::Up if n > 0 => {
                self.template = match self.template {
                    None => Some(n - 1),
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                };
                Action::Stay
            }
            KeyCode::Enter => match self.name.trim() {
                "" => Action::Close,
                name => Action::Create {
                    in_folder: self.in_folder.clone(),
                    name: name.to_string(),
                    template: self.template.map(|i| self.templates[i].0.clone()),
                },
            },
            _ => {
                line_input::key(&mut self.name, k, CtrlU::Clears);
                Action::Stay
            }
        }
    }

    /// Draw it over `area`: the folder and the name, and under it the templates when the
    /// workspace has any.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let title = format!(
            " new note in {} · ⏎ create · Esc cancel ",
            self.in_folder.shown()
        );
        let r = popup::prompt(f, area, &title, "name: ", &self.name);
        if self.templates.is_empty() {
            return;
        }
        let room = area.bottom().saturating_sub(r.bottom()) as usize;
        let rows = (self.templates.len() + 1).min(room.saturating_sub(2));
        let list = Rect::new(r.x, r.bottom(), r.width, rows as u16 + 2);
        let shown = self.template.map_or(0, |i| i + 1);
        let start = shown.saturating_sub(rows.saturating_sub(1));
        let choices = std::iter::once("No template")
            .chain(self.templates.iter().map(|(_, name)| name.as_str()));
        let lines: Vec<Line> = choices
            .enumerate()
            .skip(start)
            .take(rows)
            .map(|(i, t)| {
                let style = if i == shown {
                    Style::new().fg(Color::Black).bg(ACCENT)
                } else if i == 0 {
                    Style::new().fg(MUTED)
                } else {
                    Style::new().fg(NOTE)
                };
                let width = list.width.saturating_sub(2) as usize;
                let mark = if i == shown { "▸" } else { " " };
                Line::styled(
                    format!("{mark} {t:<w$}", w = width.saturating_sub(2)),
                    style,
                )
            })
            .collect();
        f.render_widget(Clear, list);
        f.render_widget(
            Paragraph::new(lines).block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(ACCENT_SOFT))
                    .title(" template · Tab "),
            ),
            list,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn press(n: &mut NewNote, code: KeyCode) -> Action {
        n.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn templates() -> Vec<(String, String)> {
        vec![
            ("Templates/Eruption".into(), "Eruption".into()),
            ("Templates/Person".into(), "Person".into()),
        ]
    }

    fn in_folder(rel: &str) -> Folder {
        Folder {
            root: "/w".into(),
            rel: rel.into(),
        }
    }

    fn typed(dir: &str, templates: Vec<(String, String)>, text: &str) -> NewNote {
        let mut n = NewNote::new(in_folder(dir), templates);
        for c in text.chars() {
            assert_eq!(press(&mut n, KeyCode::Char(c)), Action::Stay);
        }
        n
    }

    #[test]
    fn enter_creates_the_trimmed_name_in_the_folder_it_was_opened_on() {
        let mut n = typed("Volcanoes", Vec::new(), " Pinatubo ");
        assert_eq!(
            press(&mut n, KeyCode::Enter),
            Action::Create {
                in_folder: in_folder("Volcanoes"),
                name: "Pinatubo".into(),
                template: None,
            }
        );
        let mut n = typed("", Vec::new(), "  ");
        assert_eq!(press(&mut n, KeyCode::Enter), Action::Close, "no name");
        assert_eq!(
            press(&mut typed("", Vec::new(), "x"), KeyCode::Esc),
            Action::Close
        );
    }

    #[test]
    fn tab_and_the_arrows_go_round_the_templates_through_none() {
        let chosen = |keys: &[KeyCode]| {
            let mut n = typed("", templates(), "Pinatubo");
            for &k in keys {
                assert_eq!(press(&mut n, k), Action::Stay);
            }
            match press(&mut n, KeyCode::Enter) {
                Action::Create { template, .. } => template,
                other => panic!("created nothing: {other:?}"),
            }
        };
        assert_eq!(chosen(&[]), None);
        assert_eq!(chosen(&[KeyCode::Tab]), Some("Templates/Eruption".into()));
        assert_eq!(
            chosen(&[KeyCode::Down, KeyCode::Down]),
            Some("Templates/Person".into())
        );
        assert_eq!(chosen(&[KeyCode::Tab, KeyCode::Tab, KeyCode::Tab]), None);
        assert_eq!(chosen(&[KeyCode::Up]), Some("Templates/Person".into()));
        assert_eq!(
            chosen(&[KeyCode::BackTab, KeyCode::BackTab]),
            Some("Templates/Eruption".into())
        );
        assert_eq!(chosen(&[KeyCode::Tab, KeyCode::BackTab]), None);
    }

    #[test]
    fn without_templates_tab_and_the_arrows_do_nothing() {
        let mut n = typed("", Vec::new(), "Pinatubo");
        for k in [KeyCode::Tab, KeyCode::Down, KeyCode::Up, KeyCode::BackTab] {
            assert_eq!(press(&mut n, k), Action::Stay);
        }
        assert_eq!(
            press(&mut n, KeyCode::Enter),
            Action::Create {
                in_folder: in_folder(""),
                name: "Pinatubo".into(),
                template: None,
            }
        );
    }

    #[test]
    fn it_holds_work_with_nothing_typed() {
        let n = NewNote::new(in_folder(""), Vec::new());
        assert_eq!(n.unsaved(), "a new note's name is being typed");
    }

    fn drawn(n: &NewNote) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| n.draw(f, f.area())).unwrap();
        let buffer = terminal.backend().buffer();
        (0..24u16)
            .map(|y| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn it_draws_the_name_and_the_templates_with_the_chosen_one_marked() {
        let text = drawn(&typed("", Vec::new(), "Pinatubo"));
        let title = " new note in the workspace root · ⏎ create · Esc cancel ";
        assert!(text.iter().any(|r| r.contains(title)), "{text:#?}");
        assert!(text.iter().any(|r| r.contains("name: Pinatubo▏")));
        assert!(!text.iter().any(|r| r.contains("template")), "{text:#?}");

        let mut n = typed("Volcanoes", templates(), "Pinatubo");
        press(&mut n, KeyCode::Tab);
        let text = drawn(&n);
        let title = " new note in Volcanoes/ · ⏎ create · Esc cancel ";
        assert!(text.iter().any(|r| r.contains(title)), "{text:#?}");
        assert!(text.iter().any(|r| r.contains(" template · Tab ")));
        let at = |s: &str| text.iter().position(|r| r.contains(s)).expect(s);
        assert!(at("  No template") < at("▸ Eruption"));
        assert!(at("▸ Eruption") < at("  Person"));
    }
}
