//! The outline: the reader's headings, to jump to one by (fuzzy) name. It holds what is typed
//! and which row is chosen; the menu gives it the reader and jumps where it says.

use crate::app::{ACCENT, HIT, MUTED, NOTE};
use crate::line_input::CtrlU;
use crate::pick_list::{self, Picked};
use crate::popup;
use crate::reader::{Heading, Reader};
use crate::search::{self, Fuzzy};
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
    /// Close, then scroll the reader to this heading (an index into `Reader::headings`).
    Jump(usize),
}

pub struct Outline {
    query: String,
    /// A row of `hits`.
    sel: usize,
}

impl Outline {
    /// The outline of `reader`, on the heading being read.
    pub fn open(reader: Option<&Reader>) -> Outline {
        Outline {
            query: String::new(),
            sel: start(reader, ""),
        }
    }

    /// What closing it would lose, in words for the status line: a heading being typed.
    pub fn unsaved(&self) -> Option<&'static str> {
        (!self.query.is_empty()).then_some("a heading is being typed")
    }

    pub fn key(&mut self, k: KeyEvent, reader: Option<&Reader>) -> Action {
        let hits = hits(headings(reader), &self.query);
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Enter => match hits.get(self.sel) {
                Some(&(i, _)) => Action::Jump(i),
                None => Action::Close,
            },
            _ => {
                let typed =
                    pick_list::key(&mut self.query, &mut self.sel, hits.len(), k, CtrlU::Clears);
                if typed == Some(Picked::Typed) {
                    self.sel = start(reader, &self.query);
                }
                Action::Stay
            }
        }
    }

    /// Draw it over `area`: the query, then the headings that match, indented by level, the
    /// one being read marked.
    pub fn draw(&self, f: &mut Frame, area: Rect, reader: &Reader) {
        let inner = popup::picker(f, area, " Outline ", " ↑↓ choose · ⏎ jump · Esc close ");
        let (query, sel) = (&self.query, &self.sel);
        let headings = reader.headings();
        let hits = hits(headings, query);
        let reading = reader.section().last().copied();
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    format!("  {} of {} headings", hits.len(), headings.len()),
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let top = headings.iter().map(|h| h.level).min().unwrap_or(1);
        let rows = inner.height.saturating_sub(2) as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        for (row, (i, m)) in hits.iter().enumerate().skip(start).take(rows) {
            let h = &headings[*i];
            let selected = row == *sel;
            let here = Some(*i) == reading;
            let base = match (selected, here) {
                (true, _) => Style::new().fg(Color::Black).bg(ACCENT),
                (false, true) => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                (false, false) => Style::new().fg(NOTE),
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            let indent = "  ".repeat((h.level - top) as usize);
            let mark = if here { "▸ " } else { "  " };
            let mut spans = vec![Span::styled(format!(" {indent}{mark}"), base)];
            for (ci, ch) in h.text.chars().enumerate() {
                let style = if m.positions.contains(&ci) { hl } else { base };
                spans.push(Span::styled(ch.to_string(), style));
            }
            if selected {
                let used: usize = spans.iter().map(Span::width).sum();
                let pad = (inner.width as usize).saturating_sub(used);
                spans.push(Span::styled(" ".repeat(pad), base));
            }
            lines.push(Line::from(spans));
        }
        if hits.is_empty() {
            lines.push(Line::styled(" no headings match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn headings(reader: Option<&Reader>) -> &[Heading] {
    reader.map_or(&[][..], Reader::headings)
}

/// The rows for `query`, in the note's order: (heading, match).
fn hits(headings: &[Heading], query: &str) -> Vec<(usize, Fuzzy)> {
    let hits = headings.iter().enumerate();
    hits.filter_map(|(i, h)| Some((i, search::fuzzy(query, &h.text)?)))
        .collect()
}

/// The row to select for `query`: the heading being read, or the best match.
fn start(reader: Option<&Reader>, query: &str) -> usize {
    if query.trim().is_empty() {
        let section = reader.map(Reader::section);
        return section.and_then(|s| s.last().copied()).unwrap_or(0);
    }
    let hits = hits(headings(reader), query);
    let best = hits.iter().map(|(_, m)| m.score).max();
    hits.iter()
        .position(|(_, m)| Some(m.score) == best)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looks::Looks;
    use crate::markdown::{Flavour, parse};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyModifiers;

    /// Headings `Top`, `One`, `Deep`, `Two`, `Volcano`, each over ten paragraphs.
    fn reader() -> Reader {
        let paras = "para\n\n".repeat(10);
        let text = format!(
            "# Top\n\n{paras}## One\n\n{paras}### Deep\n\n{paras}## Two\n\n{paras}## Volcano\n\n{paras}"
        );
        let doc = parse(&text, &Flavour::built_in("obsidian").unwrap());
        let mut r = Reader::new(doc, Looks::default());
        r.layout(40, &|_| true, &|_, _| None);
        let area = Rect::new(0, 0, 40, 5);
        r.render(area, &mut Buffer::empty(area));
        r
    }

    fn press(o: &mut Outline, r: &Reader, code: KeyCode) -> Action {
        o.key(KeyEvent::new(code, KeyModifiers::NONE), Some(r))
    }

    fn typed(o: &mut Outline, r: &Reader, text: &str) {
        for c in text.chars() {
            assert_eq!(press(o, r, KeyCode::Char(c)), Action::Stay);
        }
    }

    #[test]
    fn it_opens_on_the_heading_being_read() {
        let mut r = reader();
        assert_eq!(
            press(&mut Outline::open(Some(&r)), &r, KeyCode::Enter),
            Action::Jump(0)
        );
        r.go_to_heading(2);
        let mut o = Outline::open(Some(&r));
        assert_eq!(press(&mut o, &r, KeyCode::Enter), Action::Jump(2), "Deep");
    }

    #[test]
    fn typing_chooses_the_best_match_and_enter_jumps_to_it() {
        let r = reader();
        let mut o = Outline::open(Some(&r));
        typed(&mut o, &r, "vol");
        assert_eq!(press(&mut o, &r, KeyCode::Enter), Action::Jump(4));
    }

    #[test]
    fn a_cleared_query_goes_back_to_the_heading_being_read() {
        let mut r = reader();
        r.go_to_heading(1);
        let mut o = Outline::open(Some(&r));
        typed(&mut o, &r, "vol");
        press(&mut o, &r, KeyCode::Backspace);
        press(&mut o, &r, KeyCode::Backspace);
        press(&mut o, &r, KeyCode::Backspace);
        assert_eq!(o.unsaved(), None);
        assert_eq!(press(&mut o, &r, KeyCode::Enter), Action::Jump(1), "One");
    }

    #[test]
    fn the_cursor_moves_down_the_rows_and_stops_at_the_end() {
        let r = reader();
        let mut o = Outline::open(Some(&r));
        for _ in 0..10 {
            assert_eq!(press(&mut o, &r, KeyCode::Down), Action::Stay);
        }
        assert_eq!(press(&mut o, &r, KeyCode::Enter), Action::Jump(4));
    }

    #[test]
    fn esc_closes_and_enter_with_nothing_matching_closes() {
        let r = reader();
        let mut o = Outline::open(Some(&r));
        typed(&mut o, &r, "zzzz");
        assert_eq!(o.unsaved(), Some("a heading is being typed"));
        assert_eq!(press(&mut o, &r, KeyCode::Enter), Action::Close);
        assert_eq!(
            press(&mut Outline::open(Some(&r)), &r, KeyCode::Esc),
            Action::Close
        );
    }

    #[test]
    fn without_a_reader_it_types_and_offers_nothing() {
        let mut o = Outline::open(None);
        let k = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(o.key(k, None), Action::Stay);
        assert_eq!(o.unsaved(), Some("a heading is being typed"));
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(o.key(enter, None), Action::Close);
    }

    #[test]
    fn it_draws_the_count_and_indents_each_heading_by_its_level() {
        let r = reader();
        let o = Outline::open(Some(&r));
        let mut terminal = Terminal::new(TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| o.draw(f, f.area(), &r)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: Vec<String> = (0..30u16)
            .map(|y| (0..60u16).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let at = |s: &str| text.iter().position(|row| row.contains(s));
        let query = at("▸ ▏").expect("the query");
        assert!(text[query].contains("5 of 5 headings"), "{}", text[query]);
        let top = at("Top").expect("Top");
        let deep = at("Deep").expect("Deep");
        let column = |row: usize, s: &str| {
            let x = text[row].find(s).unwrap();
            text[row][..x].chars().count()
        };
        assert_eq!(
            column(deep, "Deep"),
            column(top, "Top") + 4,
            "two levels down"
        );
        assert!(at("Outline").is_some());
    }
}
