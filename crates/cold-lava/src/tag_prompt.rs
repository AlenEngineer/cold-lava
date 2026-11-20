//! The tag prompt: a tag to add to the notes marked when it opened, or to the note it was
//! opened on when none were; `-tag` removes it. Tab and the arrows complete it from the
//! workspace's tags. It holds whom it tags and the workspace they are in, and its title names
//! them, so a tag goes where the prompt says whatever is selected or marked by Enter.

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

/// Whom a tag goes on, as it was when the prompt opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Whom {
    /// The marked notes and folders, relative to the workspace's root (a folder is skipped).
    Marked(Vec<String>),
    /// The note selected, with nothing marked.
    Note(PathBuf),
    /// Nothing marked and no note selected.
    Nobody,
}

/// What the menu should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    /// Close, then add `tag` as shown and trimmed (or remove it, for `-tag`) on `whom`, in the
    /// workspace at `root`.
    Apply {
        tag: String,
        whom: Whom,
        root: PathBuf,
    },
}

pub struct TagPrompt {
    /// The workspace `whom` is in.
    root: PathBuf,
    whom: Whom,
    typed: String,
    /// The workspace's tags.
    all: Vec<String>,
    /// The one Tab completed to, among those that match `typed`.
    sel: Option<usize>,
}

impl TagPrompt {
    pub fn new(all: Vec<String>, root: PathBuf, whom: Whom) -> TagPrompt {
        TagPrompt {
            root,
            whom,
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
                tag: self.shown().trim().to_string(),
                whom: self.whom.clone(),
                root: self.root.clone(),
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

    /// Draw it over `area`, its title naming whom it tags, with the matching tags under it.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let whom = match &self.whom {
            Whom::Marked(marked) => format!(
                "{}{}",
                plural(marked.len(), "marked note"),
                crate::prompt::gone_of(&self.root, marked)
            ),
            Whom::Note(path) => path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            Whom::Nobody => "the note".to_string(),
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

    fn note() -> Whom {
        Whom::Note(PathBuf::from("/w/Volcanoes/Krakatoa.md"))
    }

    fn typed_on(whom: Whom, text: &str) -> TagPrompt {
        let all = ["ash", "ashfall", "lava", "lava/flow"].map(String::from);
        let mut t = TagPrompt::new(all.to_vec(), PathBuf::from("/w"), whom);
        for c in text.chars() {
            assert_eq!(press(&mut t, KeyCode::Char(c)), Action::Stay);
        }
        t
    }

    fn typed(text: &str) -> TagPrompt {
        typed_on(note(), text)
    }

    fn applied(t: &mut TagPrompt) -> String {
        match press(t, KeyCode::Enter) {
            Action::Apply { tag, whom, root } => {
                assert_eq!(whom, note(), "the note it was opened on");
                assert_eq!(root, PathBuf::from("/w"), "the workspace it was opened in");
                tag
            }
            other => panic!("applied nothing: {other:?}"),
        }
    }

    #[test]
    fn enter_applies_what_is_typed_to_the_note_it_was_opened_on() {
        assert_eq!(applied(&mut typed("#ember")), "#ember");
        assert_eq!(applied(&mut typed("-lava")), "-lava");
        assert_eq!(applied(&mut typed("  ember ")), "ember", "trimmed");
        assert_eq!(press(&mut typed("ash"), KeyCode::Esc), Action::Close);
        let marked = Whom::Marked(vec!["Volcanoes/Krakatoa.md".into()]);
        assert_eq!(
            press(&mut typed_on(marked.clone(), "ash"), KeyCode::Enter),
            Action::Apply {
                tag: "ash".into(),
                whom: marked,
                root: PathBuf::from("/w"),
            },
            "the marks as they were when it opened"
        );
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
            TagPrompt::new(Vec::new(), PathBuf::new(), Whom::Nobody).unsaved(),
            "a tag is being typed"
        );
    }

    fn drawn(t: &TagPrompt) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| t.draw(f, f.area())).unwrap();
        let buffer = terminal.backend().buffer();
        (0..30u16)
            .map(|y| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn it_draws_whom_it_tags_and_the_matching_tags() {
        let mut t = typed("la");
        press(&mut t, KeyCode::Tab);
        let text = drawn(&t);
        let at = |s: &str| text.iter().position(|r| r.contains(s)).expect(s);
        assert!(at(" tag Krakatoa · -tag removes") < at("tag: #lava▏"));
        assert!(at("tag: #lava▏") < at(" 2 tags "));
        assert!(at("▸ #lava ") < at("  #lava/flow"));
        let marked = Whom::Marked(vec!["a.md".into(), "b.md".into(), "c".into()]);
        let shown = |whom, title: &str| {
            let text = drawn(&typed_on(whom, ""));
            assert!(text.iter().any(|r| r.contains(title)), "{title}: {text:#?}");
        };
        shown(marked, " tag 3 marked notes (3 gone) · -tag removes");
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let marked = Whom::Marked(vec!["Cargo.toml".into(), "src/main.rs".into()]);
        let text = drawn(&TagPrompt::new(Vec::new(), here, marked));
        let title = " tag 2 marked notes · -tag removes";
        assert!(text.iter().any(|r| r.contains(title)), "{text:#?}");
        shown(Whom::Nobody, " tag the note · -tag removes");
        let none = drawn(&typed("zz"));
        assert!(!none.iter().any(|r| r.contains(" tags ")), "{none:#?}");
    }
}
