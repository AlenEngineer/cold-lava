//! The list view: a declarative view any extension can send (docs/extensions.md, "Views"), the
//! "list / tree" word of the view vocabulary. The sender says what the groups and rows are; the
//! view moves the selection, folds groups, filters and draws, and answers keys with `Event`s
//! about rows by id. It knows nothing of tasks: a checkbox is a state to draw and a key to
//! report. Every type that crosses derives serde, like the table's; it moves to `crates/views`
//! when a second frontend draws one.

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::app::{ACCENT, DIM, HIT, MUTED, NOTE};
use crate::looks;
use crate::types::Look;
use crate::workspace::search_key;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct List {
    pub title: String,
    pub groups: Vec<Group>,
    /// Keys the sender gives a meaning to. The view's own keys come first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<KeyCommand>,
    /// What an empty list says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
    #[serde(default, skip_serializing_if = "Filter::is_slash")]
    pub filter: Filter,
}

/// How a view starts filtering: with `/` (the default), or as soon as you type, for a view
/// whose whole job is picking a row. Typing then takes every character, so the view's letter
/// keys and the sender's keys are out of reach; the arrows, Enter and Esc still work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filter {
    #[default]
    Slash,
    Typing,
}

impl Filter {
    pub fn is_slash(&self) -> bool {
        *self == Filter::Slash
    }

    /// Whether a view filtering this way takes `k` into its filter: when it filters as you
    /// type, a character (without Ctrl or Alt), Backspace and Ctrl-u.
    pub fn takes(self, k: &KeyEvent) -> bool {
        let plain = !(k.modifiers).intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        self == Filter::Typing
            && match k.code {
                KeyCode::Char('u') if k.modifiers == KeyModifiers::CONTROL => true,
                KeyCode::Char(_) => plain,
                KeyCode::Backspace => true,
                _ => false,
            }
    }
}

/// `k` typed into `text`, as `Filter::takes` says it takes it.
pub fn type_into(text: &mut String, k: &KeyEvent) {
    match k.code {
        KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Char(c) => text.push(c),
        KeyCode::Backspace => {
            text.pop();
        }
        _ => {}
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub title: String,
    /// Folded when it first shows; folding it by hand then wins.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    pub rows: Vec<Row>,
}

/// One row: an id the sender knows it by, its text, and what goes beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub text: Vec<Piece>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    /// Drawn at the right edge: a date, a note's name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub right: Vec<Piece>,
    /// How far it is nested under the row before it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: usize,
    /// A look over the whole row (a done task's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
}

/// Text with a look of its own: a priority's chip, an overdue date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
}

impl Piece {
    pub fn plain(text: impl Into<String>) -> Piece {
        Piece {
            text: text.into(),
            look: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Check {
    Unchecked,
    Partial,
    Checked,
    Crossed,
}

/// A key the sender binds: pressing it sends `Event::Command` with the selected row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyCommand {
    /// One character.
    pub key: char,
    pub command: String,
    pub title: String,
}

/// What the view tells its sender.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Enter on a row.
    Activated { row: String },
    /// `x` on a row with a checkbox.
    Toggled { row: String },
    /// One of the sender's keys.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        row: Option<String>,
    },
}

/// What the menu should do after a key.
#[derive(Debug, PartialEq)]
pub enum Action {
    Stay,
    Close,
    Send(Event),
}

/// The selected line, by id.
#[derive(Clone, Debug, PartialEq)]
enum Sel {
    Group(String),
    Row(String),
}

/// A line on screen: a group's header, or a row (group, row) of `list.groups`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shown {
    Header(usize),
    Row(usize, usize),
}

pub struct ListView {
    list: List,
    shown: Vec<Shown>,
    sel: usize,
    filter: String,
    filtering: bool,
    /// Groups folded or opened by hand, by id, over the sender's `collapsed`.
    folds: HashMap<String, bool>,
    /// A row to select when it shows up (after it was ticked and moved, say).
    follow: Option<String>,
}

impl ListView {
    pub fn new(list: List) -> ListView {
        let mut v = ListView {
            list,
            shown: Vec::new(),
            sel: 0,
            filter: String::new(),
            filtering: false,
            folds: HashMap::new(),
            follow: None,
        };
        v.reshow(None);
        v
    }

    pub fn title(&self) -> &str {
        &self.list.title
    }

    pub fn keys(&self) -> &[KeyCommand] {
        &self.list.keys
    }

    pub fn filters(&self) -> Filter {
        self.list.filter
    }

    /// The list as it is now, keeping the selection by id, the filter and the folds.
    pub fn update(&mut self, list: List) {
        let keep = match self.follow.take() {
            Some(id) => Some(Sel::Row(id)),
            None => self.at(),
        };
        self.list = list;
        self.reshow(keep);
    }

    /// Select the row with this id now, or when it shows up.
    pub fn select(&mut self, id: &str) {
        self.reshow(Some(Sel::Row(id.to_string())));
        if self.selected().is_none_or(|r| r.id != id) {
            self.follow = Some(id.to_string());
        }
    }

    pub fn selected(&self) -> Option<&Row> {
        match self.shown.get(self.sel)? {
            Shown::Row(g, r) => Some(&self.list.groups[*g].rows[*r]),
            Shown::Header(_) => None,
        }
    }

    fn at(&self) -> Option<Sel> {
        Some(match *self.shown.get(self.sel)? {
            Shown::Header(g) => Sel::Group(self.list.groups[g].id.clone()),
            Shown::Row(g, r) => Sel::Row(self.list.groups[g].rows[r].id.clone()),
        })
    }

    fn folded(&self, g: &Group) -> bool {
        self.filter.is_empty() && *self.folds.get(&g.id).unwrap_or(&g.collapsed)
    }

    /// The lines to show, then the selection put back on `keep`: its row, or the header of
    /// the folded group it is in.
    fn reshow(&mut self, keep: Option<Sel>) {
        let words: Vec<String> = search_key(&self.filter)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        self.shown.clear();
        for (g, group) in self.list.groups.iter().enumerate() {
            let rows: Vec<usize> = (0..group.rows.len())
                .filter(|&r| words.iter().all(|w| row_key(&group.rows[r]).contains(w)))
                .collect();
            if rows.is_empty() {
                continue;
            }
            self.shown.push(Shown::Header(g));
            if !self.folded(group) {
                self.shown
                    .extend(rows.into_iter().map(|r| Shown::Row(g, r)));
            }
        }
        let groups = &self.list.groups;
        let found = keep.and_then(|k| {
            let row_at = |id: &str| {
                let g = groups
                    .iter()
                    .position(|g| g.rows.iter().any(|r| r.id == id))?;
                (self.shown.iter())
                    .position(|s| matches!(*s, Shown::Row(sg, r) if groups[sg].rows[r].id == id))
                    .or_else(|| self.shown.iter().position(|s| *s == Shown::Header(g)))
            };
            match &k {
                Sel::Row(id) => row_at(id),
                Sel::Group(id) => (self.shown.iter())
                    .position(|s| matches!(*s, Shown::Header(g) if groups[g].id == *id)),
            }
        });
        self.sel = found
            .unwrap_or(self.sel)
            .min(self.shown.len().saturating_sub(1));
    }

    /// The group the selection is in.
    fn group(&self) -> Option<usize> {
        match *self.shown.get(self.sel)? {
            Shown::Header(g) | Shown::Row(g, _) => Some(g),
        }
    }

    fn fold(&mut self, g: usize, folded: bool) {
        let id = self.list.groups[g].id.clone();
        self.folds.insert(id.clone(), folded);
        self.reshow(Some(Sel::Group(id)));
    }

    /// Typing a filter: keys are text then.
    pub fn typing(&self) -> bool {
        self.filtering
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.filtering {
            let keep = self.at();
            match k.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filtering = false;
                }
                KeyCode::Enter | KeyCode::Down | KeyCode::Up => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char('u') if ctrl => self.filter.clear(),
                KeyCode::Char(c) if !ctrl => self.filter.push(c),
                _ => {}
            }
            self.reshow(keep);
            return Action::Stay;
        }
        if self.list.filter.takes(&k) {
            let keep = self.at();
            type_into(&mut self.filter, &k);
            self.reshow(keep);
            return Action::Stay;
        }
        let last = self.shown.len().saturating_sub(1);
        let here = self.shown.get(self.sel).copied();
        match k.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                let keep = self.at();
                self.reshow(keep);
            }
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.sel = 0,
            KeyCode::End | KeyCode::Char('G') => self.sel = last,
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(g) = self.group() {
                    self.fold(g, true);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => match here {
                Some(Shown::Header(g)) if self.folded(&self.list.groups[g]) => self.fold(g, false),
                Some(Shown::Header(_)) => self.sel = (self.sel + 1).min(last),
                _ => {}
            },
            KeyCode::Tab => {
                if let Some(g) = self.group() {
                    let folded = self.folded(&self.list.groups[g]);
                    self.fold(g, !folded);
                }
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Enter => match (here, self.selected()) {
                (Some(Shown::Header(g)), _) => {
                    let folded = self.folded(&self.list.groups[g]);
                    self.fold(g, !folded);
                }
                (_, Some(row)) => {
                    return Action::Send(Event::Activated {
                        row: row.id.clone(),
                    });
                }
                _ => {}
            },
            KeyCode::Char('x') if self.selected().is_some_and(|r| r.check.is_some()) => {
                let row = self.selected().map(|r| r.id.clone()).unwrap_or_default();
                return Action::Send(Event::Toggled { row });
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(cmd) = self.list.keys.iter().find(|kc| kc.key == c) {
                    return Action::Send(Event::Command {
                        command: cmd.command.clone(),
                        row: self.selected().map(|r| r.id.clone()),
                    });
                }
            }
            _ => {}
        }
        Action::Stay
    }

    /// The line under the list: the filter being typed, else a summary.
    pub fn status(&self) -> Line<'static> {
        if self.filtering {
            return Line::from(vec![
                Span::styled(" / ", Style::new().fg(HIT)),
                Span::styled(format!("{}▏", self.filter), Style::new().fg(NOTE)),
            ]);
        }
        let all: usize = self.list.groups.iter().map(|g| g.rows.len()).sum();
        let s = match self.filter.is_empty() {
            true => format!(" {}", plural(all, "row")),
            false => {
                let shown: usize = (self.list.groups.iter())
                    .map(|g| (g.rows.iter()).filter(|r| self.matches(r)).count())
                    .sum();
                format!(" {shown} of {} · /{}", plural(all, "row"), self.filter)
            }
        };
        Line::styled(s, Style::new().fg(MUTED))
    }

    fn matches(&self, row: &Row) -> bool {
        let key = row_key(row);
        search_key(&self.filter)
            .split_whitespace()
            .all(|w| key.contains(w))
    }

    /// The list in `height` lines of `width` columns, scrolled to the selection.
    pub fn lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        let start = self.sel.saturating_sub(height.saturating_sub(1));
        let mut out = Vec::new();
        for (i, &s) in self.shown.iter().enumerate().skip(start).take(height) {
            let selected = i == self.sel;
            let mut line = match s {
                Shown::Header(g) => self.header(g),
                Shown::Row(g, r) => row_line(&self.list.groups[g].rows[r], width),
            };
            if selected {
                line = line.patch_style(Style::new().bg(SELECTED));
                let used = line.width();
                line.push_span(Span::styled(
                    " ".repeat(width.saturating_sub(used)),
                    Style::new().bg(SELECTED),
                ));
            }
            out.push(line);
        }
        if self.shown.is_empty() {
            let why = match (self.filter.is_empty(), &self.list.empty) {
                (false, _) => "no rows match".to_string(),
                (true, Some(e)) => e.clone(),
                (true, None) => "nothing here".to_string(),
            };
            out.push(Line::styled(format!("  {why}"), Style::new().fg(MUTED)));
        }
        out
    }

    fn header(&self, g: usize) -> Line<'static> {
        let group = &self.list.groups[g];
        let arrow = if self.folded(group) { "▸" } else { "▾" };
        let n = match self.filter.is_empty() {
            true => group.rows.len(),
            false => group.rows.iter().filter(|r| self.matches(r)).count(),
        };
        Line::from(vec![
            Span::styled(
                format!("{arrow} {}", group.title),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {n}"), Style::new().fg(MUTED)),
        ])
    }
}

const SELECTED: Color = Color::Rgb(45, 45, 62);

fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn plural(n: usize, word: &str) -> String {
    match n {
        1 => format!("1 {word}"),
        n => format!("{n} {word}s"),
    }
}

/// A row's text and right side, folded for matching.
fn row_key(row: &Row) -> String {
    let text: Vec<&str> = (row.text.iter().chain(&row.right))
        .map(|p| p.text.as_str())
        .collect();
    search_key(&text.join(" "))
}

fn width_of(s: &str) -> usize {
    Span::raw(s).width()
}

/// `s` cut to at most `w` columns, with `…` where it was cut.
fn cut(s: &str, w: usize) -> String {
    if width_of(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let mut buf = [0; 4];
        if width_of(&out) + width_of(c.encode_utf8(&mut buf)) + 1 > w {
            break;
        }
        out.push(c);
    }
    if w > 0 {
        out.push('…');
    }
    out
}

/// A piece as drawn: its look on `base`, a chip padded, an icon before it.
fn piece_span(p: &Piece, base: Style) -> Span<'static> {
    let Some(look) = &p.look else {
        return Span::styled(p.text.clone(), base);
    };
    let mut text = p.text.clone();
    if let Some(icon) = &look.icon {
        text = format!("{icon} {text}");
    }
    if look.badge {
        text = format!(" {text} ");
    }
    Span::styled(text, looks::apply(base, look))
}

/// A row: indent, checkbox, text, and the right side at the right edge; the text is cut to
/// leave room for it.
fn row_line(row: &Row, width: usize) -> Line<'static> {
    let mut base = Style::new().fg(NOTE);
    if let Some(l) = &row.look {
        base = looks::apply(
            base,
            &Look {
                badge: false,
                ..l.clone()
            },
        );
    }
    let mut left = vec![Span::raw(format!("  {}", "  ".repeat(row.depth)))];
    if let Some(c) = row.check {
        let (mark, colour) = match c {
            Check::Unchecked => ("☐ ", MUTED),
            Check::Partial => ("◐ ", HIT),
            Check::Checked => ("☑ ", ACCENT),
            Check::Crossed => ("☒ ", DIM),
        };
        left.push(Span::styled(mark, Style::new().fg(colour)));
    }
    let mut right: Vec<Span> = Vec::new();
    for p in &row.right {
        right.push(Span::raw("  "));
        right.push(piece_span(p, Style::new().fg(MUTED)));
    }
    let right_w: usize = right.iter().map(Span::width).sum();
    let mut room = width.saturating_sub(right_w + left.iter().map(Span::width).sum::<usize>());
    for (i, p) in row.text.iter().enumerate() {
        let sep = if i > 0 { " " } else { "" };
        let mut span = piece_span(p, base);
        span.content = format!("{sep}{}", span.content).into();
        let w = span.width();
        if w > room {
            span.content = cut(&span.content, room).into();
            left.push(span);
            room = 0;
            break;
        }
        room -= w;
        left.push(span);
    }
    left.push(Span::raw(" ".repeat(room)));
    left.extend(right);
    Line::from(left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn row(id: &str, text: &str) -> Row {
        Row {
            id: id.into(),
            text: vec![Piece::plain(text)],
            check: Some(Check::Unchecked),
            right: vec![Piece::plain("Journal")],
            depth: 0,
            look: None,
        }
    }

    fn list() -> List {
        List {
            title: "agenda".into(),
            groups: vec![
                Group {
                    id: "today".into(),
                    title: "Today".into(),
                    collapsed: false,
                    rows: vec![row("a", "Pay rent"), row("b", "Ring Kīlauea")],
                },
                Group {
                    id: "later".into(),
                    title: "Later".into(),
                    collapsed: false,
                    rows: vec![Row {
                        check: None,
                        ..row("c", "Read about lahars")
                    }],
                },
                Group {
                    id: "undated".into(),
                    title: "Undated".into(),
                    collapsed: true,
                    rows: vec![row("d", "Sort photos")],
                },
            ],
            keys: vec![KeyCommand {
                key: 'd',
                command: "show-done".into(),
                title: "show done".into(),
            }],
            empty: None,
            filter: Default::default(),
        }
    }

    /// The lines as text, `>` marking the selected one.
    fn text(v: &ListView) -> Vec<String> {
        (v.shown.iter().enumerate())
            .map(|(i, s)| {
                let mark = if i == v.sel { ">" } else { " " };
                match *s {
                    Shown::Header(g) => format!("{mark}# {}", v.list.groups[g].title),
                    Shown::Row(g, r) => format!("{mark}  {}", v.list.groups[g].rows[r].id),
                }
            })
            .collect()
    }

    #[test]
    fn moves_through_headers_and_rows_and_folds_groups() {
        let mut v = ListView::new(list());
        assert_eq!(
            text(&v),
            ["># Today", "   a", "   b", " # Later", "   c", " # Undated"],
            "Undated starts folded"
        );
        v.key(key(KeyCode::Char('j')));
        v.key(key(KeyCode::Char('j')));
        assert_eq!(v.selected().unwrap().id, "b");
        v.key(key(KeyCode::Char('h')));
        assert_eq!(text(&v), ["># Today", " # Later", "   c", " # Undated"]);
        v.key(key(KeyCode::Char('l')));
        assert_eq!(v.shown.len(), 6, "l opens a folded group");
        v.key(key(KeyCode::Char('G')));
        v.key(key(KeyCode::Tab));
        assert_eq!(v.shown.len(), 7, "Tab opens Undated");
        v.key(key(KeyCode::Char('j')));
        assert_eq!(v.selected().unwrap().id, "d");
        let mut v2 = ListView::new(list());
        v2.key(key(KeyCode::Enter));
        assert_eq!(v2.shown.len(), 4, "Enter on a header folds it");
    }

    #[test]
    fn sends_events_about_rows_by_id() {
        let mut v = ListView::new(list());
        assert_eq!(
            v.key(key(KeyCode::Char('x'))),
            Action::Stay,
            "a header has no box"
        );
        v.key(key(KeyCode::Down));
        assert_eq!(
            v.key(key(KeyCode::Char('x'))),
            Action::Send(Event::Toggled { row: "a".into() })
        );
        assert_eq!(
            v.key(key(KeyCode::Enter)),
            Action::Send(Event::Activated { row: "a".into() })
        );
        assert_eq!(
            v.key(key(KeyCode::Char('d'))),
            Action::Send(Event::Command {
                command: "show-done".into(),
                row: Some("a".into())
            })
        );
        v.key(key(KeyCode::Char('G')));
        v.key(key(KeyCode::Up));
        assert_eq!(v.selected().unwrap().id, "c");
        assert_eq!(
            v.key(key(KeyCode::Char('x'))),
            Action::Stay,
            "no box, nothing to tick"
        );
        assert_eq!(v.key(key(KeyCode::Char('q'))), Action::Close);
    }

    #[test]
    fn filters_by_folded_words_opening_folded_groups() {
        let mut v = ListView::new(list());
        v.key(key(KeyCode::Char('/')));
        for c in "KILA".chars() {
            v.key(key(KeyCode::Char(c)));
        }
        assert!(v.typing());
        assert_eq!(text(&v), ["># Today", "   b"]);
        v.key(key(KeyCode::Enter));
        v.key(key(KeyCode::Down));
        assert_eq!(v.selected().unwrap().id, "b");
        v.key(key(KeyCode::Char('/')));
        for _ in 0..4 {
            v.key(key(KeyCode::Backspace));
        }
        for c in "photos".chars() {
            v.key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            text(&v),
            ["># Undated", "   d"],
            "a folded group shows its matches"
        );
        v.key(key(KeyCode::Enter));
        assert_eq!(
            v.key(key(KeyCode::Esc)),
            Action::Stay,
            "Esc clears the filter first"
        );
        assert_eq!(v.shown.len(), 6);
        assert_eq!(v.key(key(KeyCode::Esc)), Action::Close);
    }

    #[test]
    fn a_list_that_filters_as_you_type_takes_its_letter_keys_as_text() {
        let mut v = ListView::new(List {
            keys: Vec::new(),
            filter: Filter::Typing,
            ..list()
        });
        v.key(key(KeyCode::Down));
        assert_eq!(v.selected().unwrap().id, "a");
        assert_eq!(
            v.key(key(KeyCode::Char('x'))),
            Action::Stay,
            "x is typed, not a tick"
        );
        assert!(v.shown.is_empty(), "no row has an x");
        v.key(key(KeyCode::Backspace));
        for c in "lah".chars() {
            assert_eq!(
                v.key(key(KeyCode::Char(c))),
                Action::Stay,
                "{c} is typed, not a fold"
            );
        }
        assert_eq!(v.shown.len(), 2, "Later and its lahars");
        assert!(!v.typing(), "no prompt: Enter and the arrows act at once");
        v.key(key(KeyCode::End));
        assert_eq!(
            v.key(key(KeyCode::Enter)),
            Action::Send(Event::Activated { row: "c".into() })
        );
        assert_eq!(
            v.key(key(KeyCode::Esc)),
            Action::Stay,
            "Esc clears the filter first"
        );
        assert_eq!(v.shown.len(), 6);
        assert_eq!(v.key(key(KeyCode::Esc)), Action::Close);
    }

    #[test]
    fn an_update_keeps_the_selection_folds_and_filter() {
        let mut v = ListView::new(list());
        v.key(key(KeyCode::Down));
        v.key(key(KeyCode::Down));
        v.key(key(KeyCode::Char('/')));
        v.key(key(KeyCode::Char('r')));
        v.key(key(KeyCode::Enter));
        let mut l = list();
        l.groups[0].rows.remove(0);
        l.groups[1].rows.push(row("a", "Pay rent"));
        v.update(l.clone());
        assert_eq!(v.selected().unwrap().id, "b");
        v.select("a");
        assert_eq!(v.selected().unwrap().id, "a", "a row that moved group");
        v.select("e");
        let mut l2 = l.clone();
        l2.groups[1].rows.push(row("e", "Sort rocks"));
        v.update(l2);
        assert_eq!(v.selected().unwrap().id, "e", "followed when it showed up");
        v.key(key(KeyCode::Char('h')));
        assert_eq!(v.filter, "r");
        let mut l3 = list();
        l3.groups[0].rows.clear();
        v.update(l3);
        assert!(
            !text(&v).contains(&" # Today".to_string()),
            "an empty group isn't shown"
        );
        assert_eq!(v.folds.get("later"), Some(&true), "the fold stays");
    }

    #[test]
    fn a_row_cuts_its_text_to_leave_room_for_the_right_side() {
        let r = Row {
            text: vec![
                Piece::plain("Look up the 2022 eruption"),
                Piece {
                    text: "⏫".into(),
                    look: Some(Look {
                        badge: true,
                        ..Look::default()
                    }),
                },
            ],
            ..row("a", "")
        };
        let l = row_line(&r, 30);
        let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(s, "  ☐ Look up the 2022…  Journal");
        assert_eq!(l.width(), 30);
        let l = row_line(&r, 60);
        let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(s.starts_with("  ☐ Look up the 2022 eruption  ⏫ "), "{s}");
        assert!(s.ends_with("  Journal"));
    }

    #[test]
    fn the_list_json_is_pinned() {
        let mut l = list();
        l.groups.truncate(1);
        l.groups[0].rows.truncate(1);
        assert_eq!(
            serde_json::to_value(&l).unwrap(),
            json!({
                "title": "agenda",
                "groups": [{
                    "id": "today",
                    "title": "Today",
                    "rows": [{
                        "id": "a",
                        "text": [{"text": "Pay rent"}],
                        "check": "unchecked",
                        "right": [{"text": "Journal"}],
                    }],
                }],
                "keys": [{"key": "d", "command": "show-done", "title": "show done"}],
            })
        );
        l.filter = Filter::Typing;
        assert_eq!(serde_json::to_value(&l).unwrap()["filter"], "typing");
        let e = Event::Command {
            command: "show-done".into(),
            row: None,
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"event": "command", "command": "show-done"})
        );
        assert_eq!(
            serde_json::to_value(Event::Toggled { row: "a".into() }).unwrap(),
            json!({"event": "toggled", "row": "a"})
        );
    }
}
