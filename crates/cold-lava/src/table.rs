//! The table view: a declarative view any extension can send (docs/extensions.md, "Views").
//! The sender says what the columns are and what each row holds; the view sorts, filters,
//! scrolls and draws it, and answers keys with `Event`s about rows by id. Every type that
//! crosses derives serde, so a table can come over the view gateway unchanged; it moves to
//! `crates/views` when a second frontend draws one.

use std::cmp::Ordering;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::app::{ACCENT, DIM, HIT, MUTED, NOTE};
use crate::looks;
use crate::types::Look;
use crate::vault::search_key;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub title: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// How its cells sort, align and read.
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
    /// Why its cells can't be edited, when they can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<String>,
}

/// A column's value type: numbers and durations sort as numbers and align right, dates as
/// ISO text, enums in their declared order, the rest as folded text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Kind {
    Text,
    Number,
    Date,
    Datetime,
    /// Minutes.
    Duration,
    Bool,
    Link,
    Tag,
    Enum {
        values: Vec<String>,
    },
}

/// One row: an id the sender knows it by, and a value per column (`null` for none, a list
/// for several).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub cells: Vec<Json>,
}

/// What the view tells its sender.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Enter on a row.
    Activated { row: String },
    /// A cell was edited to `value`, as typed.
    Edit {
        row: String,
        column: String,
        value: String,
    },
}

/// What the menu should do after a key.
#[derive(Debug, PartialEq)]
pub enum Action {
    Stay,
    Close,
    Send(Event),
    Say(String),
}

/// Where the view is, for Back to come back to: the selected row and column, the sort and
/// the filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spot {
    row: Option<String>,
    col: usize,
    sort: Option<(usize, bool)>,
    filter: String,
}

/// What the bottom line is typing into.
#[derive(Clone, Debug, PartialEq)]
enum Input {
    None,
    Filter,
    Edit { column: usize, text: String },
}

/// The widest a column gets; longer values are cut with `…`.
const CAP: usize = 32;
const GAP: usize = 2;

pub struct TableView {
    table: Table,
    /// Rows shown, as indexes into `table.rows`: filtered, then sorted.
    order: Vec<usize>,
    /// The selected row, in `order`, and column.
    sel: usize,
    col: usize,
    /// The first column drawn.
    left: usize,
    /// The column sorted by, and whether descending.
    sort: Option<(usize, bool)>,
    filter: String,
    input: Input,
    /// After an edit gave the row a new id: select it when it shows up.
    follow: Option<String>,
}

impl TableView {
    pub fn new(table: Table) -> TableView {
        let mut v = TableView {
            order: Vec::new(),
            table,
            sel: 0,
            col: 0,
            left: 0,
            sort: None,
            filter: String::new(),
            input: Input::None,
            follow: None,
        };
        v.reorder(None);
        v
    }

    pub fn title(&self) -> &str {
        &self.table.title
    }

    /// The table as it is now, keeping the sort, the filter and the selection by row id.
    pub fn update(&mut self, table: Table) {
        let keep = self
            .follow
            .take()
            .or_else(|| self.selected().map(|r| r.id.clone()));
        if self.sort.is_some_and(|(c, _)| c >= table.columns.len()) {
            self.sort = None;
        }
        self.table = table;
        self.col = self.col.min(self.table.columns.len().saturating_sub(1));
        self.reorder(keep.as_deref());
    }

    pub fn spot(&self) -> Spot {
        Spot {
            row: self.selected().map(|r| r.id.clone()),
            col: self.col,
            sort: self.sort,
            filter: self.filter.clone(),
        }
    }

    pub fn restore(&mut self, s: Spot) {
        let cols = self.table.columns.len();
        self.col = s.col.min(cols.saturating_sub(1));
        self.sort = s.sort.filter(|&(c, _)| c < cols);
        self.filter = s.filter;
        self.reorder(s.row.as_deref());
    }

    pub fn columns(&self) -> &[Column] {
        &self.table.columns
    }

    /// The table with its rows as shown: filtered and sorted.
    pub fn sorted(&self) -> Table {
        Table {
            rows: self
                .order
                .iter()
                .map(|&r| self.table.rows[r].clone())
                .collect(),
            ..self.table.clone()
        }
    }

    /// The header and rows as plain text, every value whole.
    pub fn text(&self) -> Vec<String> {
        let t = &self.table;
        let widths = widths(t, usize::MAX);
        let line = |cells: Vec<String>| {
            (cells.iter().zip(&t.columns).zip(&widths))
                .map(|((s, c), &w)| fit(s, w, align(&c.kind)))
                .collect::<Vec<_>>()
                .join(&" ".repeat(GAP))
        };
        let head = t.columns.iter().map(|c| c.name.clone()).collect();
        let rows = self.order.iter().map(|&r| {
            let cells = t.columns.iter().zip(&t.rows[r].cells);
            line(cells.map(|(c, v)| shown(&c.kind, v)).collect())
        });
        std::iter::once(line(head)).chain(rows).collect()
    }

    /// Select the row with this id now, or when it shows up.
    pub fn select(&mut self, id: &str) {
        match self.order.iter().position(|&r| self.table.rows[r].id == id) {
            Some(i) => self.sel = i,
            None => self.follow = Some(id.to_string()),
        }
    }

    pub fn selected(&self) -> Option<&Row> {
        self.order.get(self.sel).map(|&r| &self.table.rows[r])
    }

    pub fn sort_by(&mut self, column: usize, descending: bool) {
        let keep = self.selected().map(|r| r.id.clone());
        self.sort = Some((column, descending));
        self.reorder(keep.as_deref());
    }

    fn reorder(&mut self, keep: Option<&str>) {
        let t = &self.table;
        let words: Vec<String> = search_key(&self.filter)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        self.order = (0..t.rows.len())
            .filter(|&r| words.is_empty() || matches(t, &t.rows[r], &words))
            .collect();
        if let Some((c, desc)) = self.sort {
            sort(t, &mut self.order, c, desc);
        }
        let found = keep.and_then(|id| self.order.iter().position(|&r| t.rows[r].id == id));
        self.sel = found
            .unwrap_or(self.sel)
            .min(self.order.len().saturating_sub(1));
    }

    /// Typing a filter or an edit: keys are text then.
    pub fn typing(&self) -> bool {
        self.input != Input::None
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match &mut self.input {
            Input::Filter => {
                let keep = self.selected().map(|r| r.id.clone());
                match k.code {
                    KeyCode::Esc => {
                        self.filter.clear();
                        self.input = Input::None;
                    }
                    KeyCode::Enter | KeyCode::Down | KeyCode::Up => self.input = Input::None,
                    KeyCode::Backspace => {
                        self.filter.pop();
                    }
                    KeyCode::Char('u') if ctrl => self.filter.clear(),
                    KeyCode::Char(c) if !ctrl => self.filter.push(c),
                    _ => {}
                }
                self.reorder(keep.as_deref());
                return Action::Stay;
            }
            Input::Edit { column, text } => {
                match k.code {
                    KeyCode::Esc => self.input = Input::None,
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char('u') if ctrl => text.clear(),
                    KeyCode::Char(c) if !ctrl => text.push(c),
                    KeyCode::Enter => {
                        let (column, value) = (*column, std::mem::take(text));
                        self.input = Input::None;
                        let Some(row) = self.selected() else {
                            return Action::Stay;
                        };
                        return Action::Send(Event::Edit {
                            row: row.id.clone(),
                            column: self.table.columns[column].name.clone(),
                            value,
                        });
                    }
                    _ => {}
                }
                return Action::Stay;
            }
            Input::None => {}
        }
        let rows = self.order.len();
        let cols = self.table.columns.len();
        match k.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                let keep = self.selected().map(|r| r.id.clone());
                self.reorder(keep.as_deref());
            }
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel = (self.sel + 1).min(rows.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(rows.saturating_sub(1)),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.sel = 0,
            KeyCode::End | KeyCode::Char('G') => self.sel = rows.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => {
                self.col = (self.col + 1).min(cols.saturating_sub(1))
            }
            KeyCode::Left | KeyCode::Char('h') => self.col = self.col.saturating_sub(1),
            KeyCode::Char('0') => self.col = 0,
            KeyCode::Char('$') => self.col = cols.saturating_sub(1),
            KeyCode::Char('s') if cols > 0 => self.sort_by(self.col, false),
            KeyCode::Char('S') if cols > 0 => self.sort_by(self.col, true),
            KeyCode::Char('/') => self.input = Input::Filter,
            KeyCode::Char('e') => {
                let (Some(row), Some(c)) = (self.selected(), self.table.columns.get(self.col))
                else {
                    return Action::Stay;
                };
                if let Some(why) = &c.read_only {
                    return Action::Say(why.clone());
                }
                let text = match &row.cells[self.col] {
                    Json::Array(_) => {
                        return Action::Say(format!(
                            "{} has several values: edit it in the note",
                            c.name
                        ));
                    }
                    v => shown(&c.kind, v),
                };
                self.input = Input::Edit {
                    column: self.col,
                    text,
                };
            }
            KeyCode::Enter => {
                if let Some(row) = self.selected() {
                    return Action::Send(Event::Activated {
                        row: row.id.clone(),
                    });
                }
            }
            _ => {}
        }
        Action::Stay
    }

    /// The line under the table: the filter or edit being typed, else a summary.
    pub fn status(&self) -> Line<'static> {
        let typed = |label: String, text: &str| {
            Line::from(vec![
                Span::styled(label, Style::new().fg(HIT)),
                Span::styled(format!("{text}▏"), Style::new().fg(NOTE)),
            ])
        };
        match &self.input {
            Input::Filter => typed(" / ".into(), &self.filter),
            Input::Edit { column, text } => {
                let name = &self.table.columns[*column].name;
                typed(format!(" {name}: "), text)
            }
            Input::None => {
                let n = self.table.rows.len();
                let mut s = match self.filter.is_empty() {
                    true => format!(" {n} rows"),
                    false => format!(" {} of {n} rows · /{}", self.order.len(), self.filter),
                };
                if let Some((c, desc)) = self.sort {
                    let dir = if desc { "descending" } else { "ascending" };
                    s.push_str(&format!(" · by {} {dir}", self.table.columns[c].name));
                }
                Line::styled(s, Style::new().fg(MUTED))
            }
        }
    }

    /// The header and rows in `height` lines of `width` columns, scrolled to the selection.
    pub fn lines(&mut self, width: usize, height: usize) -> Vec<Line<'static>> {
        let t = &self.table;
        let widths = widths(t, CAP);
        // Room at the end for ‹›, when the columns don't all fit.
        let width = width.saturating_sub(2);
        self.left = scroll(&widths, self.col, self.left.min(self.col), width);
        let drawn: Vec<usize> = (self.left..widths.len())
            .scan(0, |used, c| {
                let start = *used;
                *used += widths[c] + GAP;
                (c == self.left || start + widths[c] <= width).then_some(c)
            })
            .collect();
        let more_left = self.left > 0;
        let more_right = drawn.last().is_some_and(|&c| c + 1 < widths.len());
        let mut out = Vec::new();
        let mut head = Vec::new();
        for &c in &drawn {
            let col = &t.columns[c];
            let arrow = match self.sort {
                Some((s, false)) if s == c => " ▲",
                Some((s, true)) if s == c => " ▼",
                _ => "",
            };
            let name = format!("{}{arrow}", col.name);
            let mut style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
            if c == self.col {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            head.push(Span::styled(fit(&name, widths[c], align(&col.kind)), style));
            head.push(Span::raw(" ".repeat(GAP)));
        }
        if more_left || more_right {
            let hint = format!(
                "{}{}",
                if more_left { "‹" } else { "" },
                if more_right { "›" } else { "" }
            );
            head.push(Span::styled(hint, Style::new().fg(MUTED)));
        }
        out.push(Line::from(head));
        let body = height.saturating_sub(1);
        let start = self.sel.saturating_sub(body.saturating_sub(1));
        for (i, &r) in self.order.iter().enumerate().skip(start).take(body) {
            let row = &t.rows[r];
            let selected = i == self.sel;
            let mut base = Style::new().fg(NOTE);
            for (col, v) in t.columns.iter().zip(&row.cells) {
                if let Some(l) = col
                    .look
                    .as_ref()
                    .and_then(|l| for_cell(l, v))
                    .filter(|l| l.whole)
                {
                    base = looks::apply(base, &Look { badge: false, ..l });
                }
            }
            if selected {
                base = base.bg(Color::Rgb(45, 45, 62));
            }
            let mut spans = Vec::new();
            for &c in &drawn {
                let col = &t.columns[c];
                let v = &row.cells[c];
                let look = col.look.as_ref().and_then(|l| for_cell(l, v));
                let mut text = shown(&col.kind, v);
                let mut style = base;
                if let Some(l) = look.as_ref().filter(|_| !v.is_null()) {
                    if let Some(icon) = &l.icon {
                        text = format!("{icon} {text}");
                    }
                    if l.badge {
                        text = format!(" {text} ");
                    }
                    style = looks::apply(style, l);
                }
                if v.is_null() {
                    style = style.fg(DIM);
                    text = "·".into();
                }
                let w = widths[c];
                let cell = fit(&text, w, align(&col.kind));
                if selected && c == self.col {
                    let on = Style::new().fg(Color::Black).bg(ACCENT);
                    spans.push(Span::styled(cell, on));
                    spans.push(Span::styled(" ".repeat(GAP), base));
                    continue;
                }
                // A chip is only as wide as its text; the rest of the cell is plain.
                let cut = text_width(&text).min(w);
                let (inside, outside) = match align(&col.kind) {
                    Align::Left => split_at_width(&cell, cut),
                    Align::Right => {
                        let (pad, chip) = split_at_width(&cell, w - cut);
                        spans.push(Span::styled(pad, base));
                        (chip, String::new())
                    }
                };
                spans.push(Span::styled(inside, style));
                spans.push(Span::styled(outside, base));
                spans.push(Span::styled(" ".repeat(GAP), base));
            }
            out.push(Line::from(spans));
        }
        if self.order.is_empty() {
            let why = match self.filter.is_empty() {
                true => "  no items",
                false => "  no rows match",
            };
            out.push(Line::styled(why, Style::new().fg(MUTED)));
        }
        out
    }
}

/// The look for one cell; the first value's, for a list.
fn for_cell(look: &Look, v: &Json) -> Option<Look> {
    let v = match v {
        Json::Array(a) => a.first()?,
        v => v,
    };
    looks::for_value(look, v)
}

#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Right,
}

fn align(kind: &Kind) -> Align {
    match kind {
        Kind::Number | Kind::Duration => Align::Right,
        _ => Align::Left,
    }
}

/// A value as the table shows it, and as an edit starts from.
pub fn shown(kind: &Kind, v: &Json) -> String {
    match v {
        Json::Null => String::new(),
        Json::Array(a) => (a.iter().map(|v| shown(kind, v)))
            .collect::<Vec<_>>()
            .join(", "),
        Json::Bool(true) => "yes".into(),
        Json::Bool(false) => "no".into(),
        Json::Number(n) if *kind == Kind::Duration => {
            let m = n.as_f64().unwrap_or(0.0).round() as i64;
            match (m / 60, m % 60) {
                (0, m) => format!("{m}m"),
                (h, 0) => format!("{h}h"),
                (h, m) => format!("{h}h{m}m"),
            }
        }
        Json::String(s) if *kind == Kind::Tag => format!("#{s}"),
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Whether a row has every word (already folded) in one of its cells' text.
fn matches(t: &Table, row: &Row, words: &[String]) -> bool {
    let text: String = (t.columns.iter().zip(&row.cells))
        .map(|(c, v)| search_key(&shown(&c.kind, v)))
        .collect::<Vec<_>>()
        .join("\u{1f}");
    words.iter().all(|w| text.contains(w.as_str()))
}

/// Sort `order` by column `c`, keeping the sender's order between equal cells. Empty cells go
/// last either way.
fn sort(t: &Table, order: &mut [usize], c: usize, desc: bool) {
    let kind = &t.columns[c].kind;
    order.sort_by(|&a, &b| {
        let (x, y) = (&t.rows[a].cells[c], &t.rows[b].cells[c]);
        match (x.is_null(), y.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                let o = compare(kind, x, y);
                if desc { o.reverse() } else { o }
            }
        }
    });
}

/// Two non-empty cells of a column of `kind`.
pub fn compare(kind: &Kind, a: &Json, b: &Json) -> Ordering {
    let first = |v: &Json| match v {
        Json::Array(a) => a.first().cloned().unwrap_or(Json::Null),
        v => v.clone(),
    };
    let (a, b) = (first(a), first(b));
    match kind {
        Kind::Number | Kind::Duration => {
            let n = |v: &Json| v.as_f64().unwrap_or(f64::INFINITY);
            n(&a).total_cmp(&n(&b))
        }
        Kind::Date | Kind::Datetime => a.as_str().cmp(&b.as_str()),
        Kind::Bool => a.as_bool().cmp(&b.as_bool()),
        Kind::Enum { values } => {
            let at = |v: &Json| {
                let s = v.as_str().unwrap_or_default();
                values.iter().position(|n| n == s).unwrap_or(usize::MAX)
            };
            at(&a)
                .cmp(&at(&b))
                .then_with(|| text_key(kind, &a).cmp(&text_key(kind, &b)))
        }
        Kind::Text | Kind::Link | Kind::Tag => text_key(kind, &a).cmp(&text_key(kind, &b)),
    }
}

fn text_key(kind: &Kind, v: &Json) -> String {
    search_key(&shown(kind, v))
}

/// Each column's width: its widest cell or its name, at most `cap`.
pub fn widths(t: &Table, cap: usize) -> Vec<usize> {
    (t.columns.iter().enumerate())
        .map(|(c, col)| {
            let cells = t.rows.iter().map(|r| {
                let v = &r.cells[c];
                let extra = match col.look.as_ref().and_then(|l| for_cell(l, v)) {
                    Some(l) if !v.is_null() => {
                        l.icon.as_deref().map_or(0, |i| text_width(i) + 1) + 2 * l.badge as usize
                    }
                    _ => 0,
                };
                text_width(&shown(&col.kind, v)) + extra
            });
            let name = text_width(&col.name) + 2;
            cells.chain([name]).max().unwrap_or(1).clamp(1, cap)
        })
        .collect()
}

/// The first column to draw so that the selected one, `col`, fits in `width`: `left` unless
/// that leaves it off screen.
pub fn scroll(widths: &[usize], col: usize, left: usize, width: usize) -> usize {
    let mut left = left.min(col);
    while left < col && widths[left..=col].iter().map(|w| w + GAP).sum::<usize>() > width {
        left += 1;
    }
    left
}

fn char_width(c: char) -> usize {
    let mut buf = [0; 4];
    Span::raw(&*c.encode_utf8(&mut buf)).width()
}

fn text_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// `s` cut or padded to exactly `w` columns.
fn fit(s: &str, w: usize, align: Align) -> String {
    let mut out = String::new();
    let mut used = 0;
    let total = text_width(s);
    for c in s.chars() {
        let cw = char_width(c);
        if total > w && used + cw >= w {
            out.push('…');
            used += 1;
            break;
        }
        out.push(c);
        used += cw;
    }
    let pad = " ".repeat(w.saturating_sub(used));
    match align {
        Align::Left => out + &pad,
        Align::Right => pad + &out,
    }
}

/// `s` split after `w` columns.
fn split_at_width(s: &str, w: usize) -> (String, String) {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        if used >= w {
            return (s[..i].to_string(), s[i..].to_string());
        }
        used += char_width(c);
    }
    (s.to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn table() -> Table {
        let col = |name: &str, kind| Column {
            name: name.into(),
            kind,
            look: None,
            read_only: None,
        };
        let row = |id: &str, cells: Json| Row {
            id: id.into(),
            cells: cells.as_array().unwrap().clone(),
        };
        Table {
            title: "volcano".into(),
            columns: vec![
                col("name", Kind::Text),
                col("elevation", Kind::Number),
                col(
                    "status",
                    Kind::Enum {
                        values: vec!["todo".into(), "in_progress".into(), "done".into()],
                    },
                ),
                col("due", Kind::Date),
                Column {
                    read_only: Some("note is read-only".into()),
                    ..col("note", Kind::Link)
                },
            ],
            rows: vec![
                row(
                    "a",
                    json!(["Kīlauea", 1247, "done", "2026-11-12", "Kīlauea"]),
                ),
                row("b", json!(["etna", 3357, "todo", null, "Etna"])),
                row(
                    "c",
                    json!(["Fuji", null, "in_progress", "2026-01-02", "Mount Fuji"]),
                ),
                row(
                    "d",
                    json!(["Mauna Loa", 4169, "todo", "2025-12-31", "Mauna Loa"]),
                ),
            ],
        }
    }

    fn ids(v: &TableView) -> Vec<&str> {
        v.order
            .iter()
            .map(|&r| v.table.rows[r].id.as_str())
            .collect()
    }

    #[test]
    fn sorts_by_the_column_type_with_empty_cells_last() {
        let mut v = TableView::new(table());
        v.sort_by(0, false);
        assert_eq!(ids(&v), ["b", "c", "a", "d"], "text folds case and accents");
        v.sort_by(1, false);
        assert_eq!(ids(&v), ["a", "b", "d", "c"]);
        v.sort_by(1, true);
        assert_eq!(
            ids(&v),
            ["d", "b", "a", "c"],
            "descending, still empty last"
        );
        v.sort_by(2, false);
        assert_eq!(ids(&v), ["b", "d", "c", "a"], "enums in declared order");
        v.sort_by(3, false);
        assert_eq!(ids(&v), ["d", "c", "a", "b"]);
    }

    #[test]
    fn filters_by_folded_words_and_keeps_the_selection() {
        let mut v = TableView::new(table());
        v.key(key(KeyCode::Down));
        assert_eq!(v.selected().unwrap().id, "b");
        v.key(key(KeyCode::Char('/')));
        for c in "ETNA".chars() {
            v.key(key(KeyCode::Char(c)));
        }
        assert_eq!(ids(&v), ["b"]);
        v.key(key(KeyCode::Enter));
        assert!(!v.typing());
        v.key(key(KeyCode::Esc));
        assert_eq!(ids(&v).len(), 4, "Esc clears the filter first");
        assert_eq!(v.selected().unwrap().id, "b");
        v.key(key(KeyCode::Char('/')));
        for c in "kilauea todo".chars() {
            v.key(key(KeyCode::Char(c)));
        }
        assert!(ids(&v).is_empty(), "every word must match");
        assert_eq!(v.key(key(KeyCode::Esc)), Action::Stay);
        assert_eq!(v.key(key(KeyCode::Esc)), Action::Close);
    }

    #[test]
    fn an_update_keeps_the_selected_row_by_id() {
        let mut v = TableView::new(table());
        v.sort_by(1, false);
        v.key(key(KeyCode::Down));
        assert_eq!(v.selected().unwrap().id, "b");
        let mut t = table();
        t.rows.remove(0);
        v.update(t);
        assert_eq!(v.selected().unwrap().id, "b");
        v.select("e");
        let mut t = table();
        t.rows.push(Row {
            id: "e".into(),
            cells: vec![
                json!("New"),
                json!(1),
                json!("todo"),
                Json::Null,
                json!("New"),
            ],
        });
        v.update(t);
        assert_eq!(v.selected().unwrap().id, "e", "followed after an edit");
    }

    #[test]
    fn edits_start_from_the_value_and_read_only_columns_say_so() {
        let mut v = TableView::new(table());
        v.key(key(KeyCode::Right));
        v.key(key(KeyCode::Char('e')));
        assert!(v.typing());
        v.key(key(KeyCode::Backspace));
        v.key(key(KeyCode::Char('9')));
        assert_eq!(
            v.key(key(KeyCode::Enter)),
            Action::Send(Event::Edit {
                row: "a".into(),
                column: "elevation".into(),
                value: "1249".into()
            })
        );
        v.key(key(KeyCode::Char('$')));
        assert_eq!(
            v.key(key(KeyCode::Char('e'))),
            Action::Say("note is read-only".into())
        );
        assert_eq!(
            v.key(key(KeyCode::Enter)),
            Action::Send(Event::Activated { row: "a".into() })
        );
    }

    #[test]
    fn widths_are_capped_and_scroll_keeps_the_column_on_screen() {
        let mut t = table();
        t.rows[0].cells[0] = json!("x".repeat(50));
        assert_eq!(widths(&t, 32), [32, 11, 11, 10, 10]);
        let w = [10, 10, 10, 10];
        assert_eq!(scroll(&w, 0, 0, 30), 0);
        assert_eq!(scroll(&w, 3, 0, 30), 2, "the last two fit");
        assert_eq!(scroll(&w, 1, 2, 30), 1, "back left to the selected one");
        assert_eq!(fit("Kīlauea", 5, Align::Left), "Kīla…");
        assert_eq!(fit("42", 4, Align::Right), "  42");
    }

    #[test]
    fn the_table_json_is_pinned() {
        let mut t = table();
        t.columns.truncate(3);
        t.rows.truncate(1);
        for r in &mut t.rows {
            r.cells.truncate(3);
        }
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            json!({
                "title": "volcano",
                "columns": [
                    {"name": "name", "type": "text"},
                    {"name": "elevation", "type": "number"},
                    {"name": "status", "type": "enum", "values": ["todo", "in_progress", "done"]},
                ],
                "rows": [{"id": "a", "cells": ["Kīlauea", 1247, "done"]}],
            })
        );
        let e = Event::Edit {
            row: "a".into(),
            column: "due".into(),
            value: "2026-12-01".into(),
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"event": "edit", "row": "a", "column": "due", "value": "2026-12-01"})
        );
    }
}
