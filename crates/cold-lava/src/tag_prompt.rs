//! The tag prompt: a tag to add to the marked notes, or to the note it was opened on when none
//! is marked; `-tag` removes it. Tab and the arrows complete it from the workspace's tags.

use crate::app::{ACCENT, ACCENT_SOFT, NOTE};
use crate::line_input::{self, CtrlU};
use crate::popup;
use crate::words::plural;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::path::PathBuf;

/// What the menu should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    /// Close, then add `tag` as shown (or remove it, for `-tag`) on the marked notes, or on
    /// `note` when none is marked.
    Apply {
        tag: String,
        note: Option<PathBuf>,
    },
}

pub struct TagPrompt {
    /// The note selected when it opened, if one was: tagged when none is marked.
    note: Option<PathBuf>,
    typed: String,
    /// The workspace's tags.
    all: Vec<String>,
    /// The one Tab completed to, among those that match `typed`.
    sel: Option<usize>,
}

impl TagPrompt {
    pub fn new(all: Vec<String>, note: Option<PathBuf>) -> TagPrompt {
        TagPrompt {
            note,
            typed: String::new(),
            all,
            sel: None,
        }
    }

    /// What closing it would lose, in words for the status line. It holds work with nothing
    /// typed yet: the next keys are meant for it.
    pub fn unsaved(&self) -> &'static str {
        "a tag is being typed"
    }

    /// The workspace's tags that start with what was typed (after a `-` and a `#`).
    fn matches(&self) -> Vec<&String> {
        let q = self
            .typed
            .trim_start_matches('-')
            .trim_start_matches('#')
            .to_lowercase();
        self.all.iter().filter(|t| t.starts_with(&q)).collect()
    }

    /// What the prompt holds: what was typed, or the tag Tab completed it to (keeping a `-`).
    fn shown(&self) -> String {
        match self.sel.and_then(|i| self.matches().get(i).copied()) {
            Some(t) if self.typed.starts_with('-') => format!("-{t}"),
            Some(t) => t.clone(),
            None => self.typed.clone(),
        }
    }

    /// Tab and the arrows go round the matching tags. What is typed goes on the tag shown, the
    /// one completed to if one is.
    pub fn key(&mut self, k: KeyEvent) -> Action {
        let matches = self.matches().len();
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Tab | KeyCode::Down if matches > 0 => {
                self.sel = Some(self.sel.map_or(0, |i| (i + 1) % matches));
                Action::Stay
            }
            KeyCode::BackTab | KeyCode::Up if matches > 0 => {
                let last = matches - 1;
                self.sel = Some(self.sel.map_or(last, |i| (i + last) % matches));
                Action::Stay
            }
            KeyCode::Enter => Action::Apply {
                tag: self.shown(),
                note: self.note.clone(),
            },
            _ => {
                let mut line = self.shown();
                if line_input::key(&mut line, k, CtrlU::Clears).is_some() {
                    self.typed = line;
                    self.sel = None;
                }
                Action::Stay
            }
        }
    }

    /// Draw it over `area`, with the matching tags under it. `marked` is how many notes are
    /// marked, which its title counts.
    pub fn draw(&self, f: &mut Frame, area: Rect, marked: usize) {
        let whom = match marked {
            0 => "the note".to_string(),
            n => plural(n, "marked note"),
        };
        let title = format!(" tag {whom} · -tag removes · Tab · ⏎ ");
        let r = popup::prompt(f, area, &title, "tag: #", &self.shown());
        let hits = self.matches();
        let room = area.bottom().saturating_sub(r.bottom()) as usize;
        let rows = hits.len().min(8).min(room.saturating_sub(2));
        if rows == 0 {
            return;
        }
        let sel = self.sel;
        let start = sel.map_or(0, |i| (i + 1).saturating_sub(rows));
        let list = Rect::new(r.x, r.bottom(), r.width, rows as u16 + 2);
        let width = list.width.saturating_sub(2) as usize;
        let lines: Vec<Line> = (hits.iter().enumerate().skip(start).take(rows))
            .map(|(i, t)| {
                let on = Some(i) == sel;
                let style = match on {
                    true => Style::new().fg(Color::Black).bg(ACCENT),
                    false => Style::new().fg(NOTE),
                };
                let mark = if on { "▸" } else { " " };
                Line::styled(
                    format!("{mark} #{t:<w$}", w = width.saturating_sub(3)),
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
                    .title(format!(" {} ", plural(hits.len(), "tag"))),
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

    fn press(t: &mut TagPrompt, code: KeyCode) -> Action {
        t.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn note() -> Option<PathBuf> {
        Some(PathBuf::from("/w/Volcanoes/Krakatoa.md"))
    }

    fn typed(text: &str) -> TagPrompt {
        let all = ["ash", "ashfall", "lava", "lava/flow"].map(String::from);
        let mut t = TagPrompt::new(all.to_vec(), note());
        for c in text.chars() {
            assert_eq!(press(&mut t, KeyCode::Char(c)), Action::Stay);
        }
        t
    }

    fn applied(t: &mut TagPrompt) -> String {
        match press(t, KeyCode::Enter) {
            Action::Apply { tag, note: n } => {
                assert_eq!(n, note(), "the note it was opened on");
                tag
            }
            other => panic!("applied nothing: {other:?}"),
        }
    }

    #[test]
    fn enter_applies_what_is_typed_to_the_note_it_was_opened_on() {
        assert_eq!(applied(&mut typed("#ember")), "#ember");
        assert_eq!(applied(&mut typed("-lava")), "-lava");
        assert_eq!(press(&mut typed("ash"), KeyCode::Esc), Action::Close);
    }

    #[test]
    fn tab_and_the_arrows_go_round_the_matching_tags() {
        let mut t = typed("la");
        press(&mut t, KeyCode::Tab);
        assert_eq!(applied(&mut t), "lava");
        let mut t = typed("-#la");
        press(&mut t, KeyCode::Down);
        press(&mut t, KeyCode::Down);
        assert_eq!(applied(&mut t), "-lava/flow", "a - is kept");
        let mut t = typed("la");
        press(&mut t, KeyCode::Tab);
        press(&mut t, KeyCode::Tab);
        press(&mut t, KeyCode::Tab);
        assert_eq!(applied(&mut t), "lava", "round past the end");
        let mut t = typed("a");
        press(&mut t, KeyCode::Up);
        assert_eq!(applied(&mut t), "ashfall", "Up starts from the last");
        let mut t = typed("a");
        press(&mut t, KeyCode::BackTab);
        press(&mut t, KeyCode::BackTab);
        assert_eq!(applied(&mut t), "ash");
    }

    #[test]
    fn typing_goes_on_the_tag_completed_to() {
        let mut t = typed("la");
        press(&mut t, KeyCode::Tab);
        press(&mut t, KeyCode::Char('/'));
        assert_eq!(applied(&mut t), "lava/");
        let mut t = typed("la");
        press(&mut t, KeyCode::Tab);
        press(&mut t, KeyCode::Backspace);
        assert_eq!(applied(&mut t), "lav");
        let mut t = typed("ash");
        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(t.key(ctrl_u), Action::Stay);
        assert_eq!(applied(&mut t), "");
    }

    #[test]
    fn with_nothing_matching_tab_does_nothing() {
        let mut t = typed("zz");
        assert_eq!(press(&mut t, KeyCode::Tab), Action::Stay);
        assert_eq!(applied(&mut t), "zz");
    }

    #[test]
    fn it_holds_work_with_nothing_typed() {
        assert_eq!(
            TagPrompt::new(Vec::new(), None).unsaved(),
            "a tag is being typed"
        );
    }

    fn drawn(t: &TagPrompt, marked: usize) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), marked)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..30u16)
            .map(|y| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn it_draws_whom_it_tags_and_the_matching_tags() {
        let mut t = typed("la");
        press(&mut t, KeyCode::Tab);
        let text = drawn(&t, 0);
        let at = |s: &str| text.iter().position(|r| r.contains(s)).expect(s);
        assert!(at(" tag the note · -tag removes") < at("tag: #lava▏"));
        assert!(at("tag: #lava▏") < at(" 2 tags "));
        assert!(at("▸ #lava ") < at("  #lava/flow"));
        assert!(
            drawn(&t, 3)
                .iter()
                .any(|r| r.contains(" tag 3 marked notes "))
        );
        let none = drawn(&typed("zz"), 0);
        assert!(!none.iter().any(|r| r.contains(" tags ")), "{none:#?}");
    }
}
