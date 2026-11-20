//! The command palette: run a menu command by (fuzzy) name. It holds what is typed and which
//! row is chosen; the menu hands it the commands that match (`commands::matching`, less those
//! it can't run now) and runs the one it picks.

use crate::app::{ACCENT, HIT, MUTED, NOTE};
use crate::commands::{Keymap, Match, MenuCommand};
use crate::line_input::CtrlU;
use crate::pick_list::{self, Picked};
use crate::popup;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// What the menu should do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    /// Close, then run this command.
    Run(MenuCommand),
}

#[derive(Clone, Default)]
pub struct Palette {
    query: String,
    /// A row of the commands that match `query`.
    sel: usize,
}

impl Palette {
    pub fn new() -> Palette {
        Palette::default()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// What closing it would lose, in words for the status line: a command's name being typed.
    pub fn unsaved(&self) -> Option<&'static str> {
        (!self.query.is_empty()).then_some("a command is being typed")
    }

    /// `hits` are the commands that match the query, as drawn.
    pub fn key(&mut self, k: KeyEvent, hits: &[Match]) -> Action {
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Enter => match hits.get(self.sel) {
                Some(m) => Action::Run(m.command.action),
                None => Action::Close,
            },
            _ => {
                let typed =
                    pick_list::key(&mut self.query, &mut self.sel, hits.len(), k, CtrlU::Clears);
                if typed == Some(Picked::Typed) {
                    self.sel = 0;
                }
                Action::Stay
            }
        }
    }

    /// Draw it over `area`: the query, then `hits` with the keys each has in `keys`.
    pub fn draw(&self, f: &mut Frame, area: Rect, hits: &[Match], keys: &Keymap<MenuCommand>) {
        let inner = popup::picker(f, area, " Commands ", " ↑↓ choose · ⏎ run · Esc close ");
        let query = &self.query;
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    if query.is_empty() {
                        "  type a command's name or id".to_string()
                    } else if hits.len() == 1 {
                        "  1 command".to_string()
                    } else {
                        format!("  {} commands", hits.len())
                    },
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let start = self.sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for (i, m) in hits.iter().enumerate().skip(start).take(rows) {
            let selected = i == self.sel;
            let base = if selected {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            let dim = if selected {
                base
            } else {
                Style::new().fg(MUTED)
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            let c = m.command;
            let mut spans = vec![Span::styled(" ", base)];
            let mut push = |text: &str, hits: &[usize], style: Style| {
                for (ci, ch) in text.chars().enumerate() {
                    let st = if hits.contains(&ci) { hl } else { style };
                    spans.push(Span::styled(ch.to_string(), st));
                }
            };
            push(&c.title, &m.title_hits, base);
            push("  ", &[], base);
            push(&c.id, &m.id_hits, dim);
            let right = format!("{} ", keys.label(&c.id));
            let used = 3 + c.title.chars().count() + c.id.chars().count();
            let pad = width.saturating_sub(used + right.chars().count());
            spans.push(Span::styled(" ".repeat(pad), base));
            let keys = if selected {
                base
            } else {
                Style::new().fg(ACCENT)
            };
            spans.push(Span::styled(right, keys));
            lines.push(Line::from(spans));
        }
        if hits.is_empty() {
            lines.push(Line::styled(" no commands match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{self, Entry, MenuAction};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn table() -> Vec<Entry<MenuCommand>> {
        commands::menu([]).0
    }

    fn press(p: &mut Palette, table: &[Entry<MenuCommand>], code: KeyCode) -> Action {
        let hits = commands::matching(table, p.query());
        p.key(KeyEvent::new(code, KeyModifiers::NONE), &hits)
    }

    fn typed(table: &[Entry<MenuCommand>], text: &str) -> Palette {
        let mut p = Palette::new();
        for c in text.chars() {
            assert_eq!(press(&mut p, table, KeyCode::Char(c)), Action::Stay);
        }
        p
    }

    fn id_of(table: &[Entry<MenuCommand>], action: Action) -> String {
        let Action::Run(c) = action else {
            panic!("ran nothing: {action:?}");
        };
        table.iter().find(|e| e.action == c).unwrap().id.clone()
    }

    #[test]
    fn enter_runs_the_chosen_command() {
        let table = table();
        let mut p = typed(&table, "graph");
        assert_eq!(
            id_of(&table, press(&mut p, &table, KeyCode::Enter)),
            "graph.open"
        );

        let mut p = Palette::new();
        let second = commands::matching(&table, "")[1].command.id.clone();
        assert_eq!(press(&mut p, &table, KeyCode::Down), Action::Stay);
        assert_eq!(id_of(&table, press(&mut p, &table, KeyCode::Enter)), second);
    }

    #[test]
    fn typing_puts_the_cursor_back_on_the_best_match() {
        let table = table();
        let mut p = Palette::new();
        press(&mut p, &table, KeyCode::Down);
        press(&mut p, &table, KeyCode::Down);
        press(&mut p, &table, KeyCode::Char('r'));
        let best = commands::matching(&table, "r")[0].command.id.clone();
        assert_eq!(id_of(&table, press(&mut p, &table, KeyCode::Enter)), best);
    }

    #[test]
    fn esc_closes_and_enter_with_nothing_matching_closes() {
        let table = table();
        assert_eq!(
            press(&mut typed(&table, "graph"), &table, KeyCode::Esc),
            Action::Close
        );
        let mut p = typed(&table, "zzzz");
        assert_eq!(press(&mut p, &table, KeyCode::Enter), Action::Close);
    }

    #[test]
    fn a_typed_name_is_unsaved_and_an_empty_one_is_not() {
        let table = table();
        assert_eq!(Palette::new().unsaved(), None);
        assert_eq!(
            typed(&table, " ").unsaved(),
            Some("a command is being typed")
        );
        let mut p = typed(&table, "gr");
        press(&mut p, &table, KeyCode::Backspace);
        press(&mut p, &table, KeyCode::Backspace);
        assert_eq!(p.unsaved(), None);
    }

    #[test]
    fn it_never_offers_itself() {
        let table = table();
        let hits = commands::matching(&table, "palette");
        let palette = MenuCommand::Core(MenuAction::Palette);
        assert!(hits.iter().all(|m| m.command.action != palette));
    }

    #[test]
    fn it_draws_the_query_the_count_and_the_chosen_row_with_its_keys() {
        let table = table();
        let keys = Keymap::configured(&table);
        let p = typed(&table, "graph");
        let hits = commands::matching(&table, p.query());
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        terminal
            .draw(|f| p.draw(f, f.area(), &hits, &keys))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: Vec<String> = (0..30u16)
            .map(|y| (0..90u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let at = |s: &str| text.iter().position(|row| row.contains(s));
        let count = format!("{} command", hits.len());
        let query = at("▸ graph▏").expect("the query");
        assert!(text[query].contains(&count), "{}", text[query]);
        let first = at("graph.open").expect("the best match");
        assert_eq!(first, query + 2, "under a blank line");
        let label = keys.label("graph.open");
        assert!(text[first].trim_end_matches(['│', ' ']).ends_with(&label));
        let x = text[first].find("graph.open").unwrap();
        let x = text[first][..x].chars().count() as u16;
        assert_eq!(buffer[(x, first as u16)].bg, ACCENT, "the chosen row");
        assert!(at("Commands").is_some());
    }
}
