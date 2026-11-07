//! The Tidy view: the engine's `tidy::Report` as a list to act on. It owns the rows, the
//! selection and its own marks, and answers each key with an `Action` the menu carries out
//! (opening, deleting through the menu's confirmed, undoable delete, creating a note).

use crate::app::{ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE};
use crate::tidy::Report;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Orphans,
    Attachments,
    Folders,
    Unresolved,
}

/// One line of the view. Items index into the report's lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Section(Section),
    /// The heading of the orphans that are daily notes.
    Daily,
    /// "nothing" under an empty section.
    Nothing,
    Orphan(usize),
    Attachment(usize),
    Folder(usize),
    /// An unresolved target, then each place that links to it.
    Target(usize),
    Link(usize, usize),
}

impl Row {
    fn item(self) -> bool {
        !matches!(self, Row::Section(_) | Row::Daily | Row::Nothing)
    }
}

/// What the menu should do after a key.
pub enum Action {
    Stay,
    Close,
    /// Open a note (vault-relative with `.md`), at a line.
    OpenNote(String, Option<usize>),
    /// Open an attachment in the system's app.
    OpenFile(String),
    /// Show a folder in the tree.
    OpenFolder(String),
    /// Ask to delete these (vault-relative) to the trash.
    Delete(Vec<String>),
    /// Create the missing note `target`, as linked from `from`.
    Create {
        target: String,
        from: String,
    },
}

pub struct TidyView {
    report: Report,
    /// Also list notes nothing links to that do link elsewhere.
    all_orphans: bool,
    sel: usize,
    /// Marked items, vault-relative.
    marks: BTreeSet<String>,
}

impl TidyView {
    pub fn new(report: Report) -> TidyView {
        let mut v = TidyView {
            report,
            all_orphans: false,
            sel: 0,
            marks: BTreeSet::new(),
        };
        v.sel = v.step(0, 1);
        v
    }

    /// The vault changed: show it as it is now, keeping the marks that still exist and the
    /// selection on the same item if it is still there.
    pub fn update(&mut self, report: Report) {
        let at = self.rows().get(self.sel).map(|&r| self.id(r));
        self.report = report;
        let rows = self.rows();
        let items: BTreeSet<String> = rows.iter().filter_map(|&r| self.path(r)).collect();
        self.marks.retain(|m| items.contains(m));
        match rows.iter().position(|&r| Some(self.id(r)) == at) {
            Some(i) => self.sel = i,
            None => {
                self.sel = self.sel.min(rows.len().saturating_sub(1));
                if !rows.get(self.sel).is_some_and(|r| r.item()) {
                    self.sel = self.step(self.sel, 1);
                }
            }
        }
    }

    /// What a row shows, independent of where it is in the list.
    fn id(&self, row: Row) -> String {
        let r = &self.report;
        match row {
            Row::Target(t) => format!("?{}", r.unresolved[t].target),
            Row::Link(t, l) => {
                let l = &r.unresolved[t].from[l];
                format!("?{}:{}:{}", r.unresolved[t].target, l.note, l.line)
            }
            row => self.path(row).unwrap_or_else(|| format!("{row:?}")),
        }
    }

    fn orphan_shown(&self, i: usize) -> bool {
        self.all_orphans || self.report.orphans[i].both_ways()
    }

    fn rows(&self) -> Vec<Row> {
        let r = &self.report;
        let mut rows = vec![Row::Section(Section::Orphans)];
        let shown: Vec<usize> = (0..r.orphans.len())
            .filter(|&i| self.orphan_shown(i))
            .collect();
        let (daily, notes): (Vec<usize>, Vec<usize>) =
            shown.iter().partition(|&&i| r.orphans[i].daily);
        rows.extend(notes.into_iter().map(Row::Orphan));
        if !daily.is_empty() {
            rows.push(Row::Daily);
            rows.extend(daily.into_iter().map(Row::Orphan));
        }
        if shown.is_empty() {
            rows.push(Row::Nothing);
        }
        let mut section = |s: Section, items: Vec<Row>| {
            rows.push(Row::Section(s));
            match items.is_empty() {
                true => rows.push(Row::Nothing),
                false => rows.extend(items),
            }
        };
        let attachments = (0..r.unused_attachments.len()).map(Row::Attachment);
        section(Section::Attachments, attachments.collect());
        let folders = (0..r.empty_folders.len()).map(Row::Folder);
        section(Section::Folders, folders.collect());
        let mut unresolved = Vec::new();
        for (t, u) in r.unresolved.iter().enumerate() {
            unresolved.push(Row::Target(t));
            unresolved.extend((0..u.from.len()).map(|l| Row::Link(t, l)));
        }
        section(Section::Unresolved, unresolved);
        rows
    }

    /// What deleting this row deletes, vault-relative.
    fn path(&self, row: Row) -> Option<String> {
        let r = &self.report;
        match row {
            Row::Orphan(i) => Some(r.orphans[i].note.clone()),
            Row::Attachment(i) => Some(r.unused_attachments[i].clone()),
            Row::Folder(i) => Some(r.empty_folders[i].clone()),
            _ => None,
        }
    }

    /// The next item row from `from` in direction `dir` (±1), or `from` if there is none.
    fn step(&self, from: usize, dir: isize) -> usize {
        let rows = self.rows();
        let mut i = from as isize;
        loop {
            i += dir;
            match rows.get(i as usize) {
                _ if i < 0 => return from,
                None => return from.min(rows.len().saturating_sub(1)),
                Some(r) if r.item() => return i as usize,
                Some(_) => {}
            }
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        let rows = self.rows();
        let row = rows.get(self.sel).copied().filter(|r| r.item());
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Down | KeyCode::Char('j') => self.sel = self.step(self.sel, 1),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.step(self.sel, -1),
            KeyCode::Home | KeyCode::Char('g') => self.sel = self.step(0, 1),
            KeyCode::End | KeyCode::Char('G') => self.sel = self.step(rows.len(), -1),
            KeyCode::Char(' ') => {
                if let Some(p) = row.and_then(|r| self.path(r))
                    && !self.marks.remove(&p)
                {
                    self.marks.insert(p);
                }
                self.sel = self.step(self.sel, 1);
            }
            KeyCode::Char('a') => {
                let at = row.and_then(|r| self.path(r));
                self.all_orphans = !self.all_orphans;
                let rows = self.rows();
                self.sel = (rows.iter())
                    .position(|&r| at.is_some() && self.path(r) == at)
                    .unwrap_or(self.sel.min(rows.len().saturating_sub(1)));
                if !rows[self.sel].item() {
                    self.sel = self.step(self.sel, 1);
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                let marked: Vec<String> = (rows.iter())
                    .filter_map(|&r| self.path(r))
                    .filter(|p| self.marks.contains(p))
                    .collect();
                if !marked.is_empty() {
                    return Action::Delete(marked);
                }
                if let Some(p) = row.and_then(|r| self.path(r)) {
                    return Action::Delete(vec![p]);
                }
            }
            KeyCode::Char('n') => {
                if let Some(Row::Target(t) | Row::Link(t, _)) = row {
                    let u = &self.report.unresolved[t];
                    return Action::Create {
                        target: u.target.clone(),
                        from: u.from[0].note.clone(),
                    };
                }
            }
            KeyCode::Enter => {
                let r = &self.report;
                return match row {
                    Some(Row::Orphan(i)) => Action::OpenNote(r.orphans[i].note.clone(), None),
                    Some(Row::Attachment(i)) => Action::OpenFile(r.unused_attachments[i].clone()),
                    Some(Row::Folder(i)) => Action::OpenFolder(r.empty_folders[i].clone()),
                    Some(Row::Target(t)) => {
                        let l = &r.unresolved[t].from[0];
                        Action::OpenNote(l.note.clone(), Some(l.line))
                    }
                    Some(Row::Link(t, l)) => {
                        let l = &r.unresolved[t].from[l];
                        Action::OpenNote(l.note.clone(), Some(l.line))
                    }
                    _ => Action::Stay,
                };
            }
            _ => {}
        }
        Action::Stay
    }

    pub fn marks(&self) -> usize {
        self.marks.len()
    }

    /// The view's lines in `height` rows of `width` columns, scrolled to the selection.
    pub fn lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        let r = &self.report;
        let rows = self.rows();
        let start = self.sel.saturating_sub(height.saturating_sub(2));
        let mut out = Vec::new();
        for (i, &row) in rows.iter().enumerate().skip(start).take(height) {
            let selected = i == self.sel;
            let marked = self.path(row).is_some_and(|p| self.marks.contains(&p));
            let mark = if marked { "✓ " } else { "  " };
            let daily_orphan = matches!(row, Row::Orphan(o) if r.orphans[o].daily);
            let (left, right, style) = match row {
                Row::Section(s) => {
                    let (title, n) = match s {
                        Section::Orphans => {
                            let n = (0..r.orphans.len()).filter(|&o| self.orphan_shown(o));
                            let title = match self.all_orphans {
                                true => "Notes nothing links to",
                                false => "Orphans: no links in or out",
                            };
                            (title, n.count())
                        }
                        Section::Attachments => ("Unused attachments", r.unused_attachments.len()),
                        Section::Folders => ("Empty folders", r.empty_folders.len()),
                        Section::Unresolved => ("Unresolved links", r.unresolved.len()),
                    };
                    let bold = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
                    (format!(" {title} · {n}"), String::new(), bold)
                }
                Row::Daily => (
                    "   daily notes".to_string(),
                    String::new(),
                    Style::new().fg(DIM).add_modifier(Modifier::ITALIC),
                ),
                Row::Nothing => (
                    "   nothing".to_string(),
                    String::new(),
                    Style::new().fg(MUTED),
                ),
                Row::Orphan(o) => {
                    let right = match r.orphans[o].links_out {
                        0 => String::new(),
                        n => format!("links to {n} "),
                    };
                    let fg = if daily_orphan { DIM } else { NOTE };
                    (
                        format!(" {mark}• {}", r.orphans[o].note),
                        right,
                        Style::new().fg(fg),
                    )
                }
                Row::Attachment(a) => {
                    let rel = &r.unused_attachments[a];
                    let kind = crate::vault::attachment_kind(rel).unwrap_or_default();
                    (
                        format!(" {mark}▫ {rel}"),
                        format!("{kind} "),
                        Style::new().fg(NOTE),
                    )
                }
                Row::Folder(f) => (
                    format!(" {mark}▤ {}/", r.empty_folders[f]),
                    String::new(),
                    Style::new().fg(NOTE),
                ),
                Row::Target(t) => {
                    let u = &r.unresolved[t];
                    let n = u.from.len();
                    let links = if n == 1 {
                        "1 link".into()
                    } else {
                        format!("{n} links")
                    };
                    (
                        format!("   ? {}", u.target),
                        format!("{links} "),
                        Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD),
                    )
                }
                Row::Link(t, l) => {
                    let l = &r.unresolved[t].from[l];
                    let note = l.note.trim_end_matches(".md");
                    (
                        format!("       ← {note}:{}  {}", l.line, l.context),
                        String::new(),
                        Style::new().fg(MUTED),
                    )
                }
            };
            let style = match selected {
                true => Style::new().fg(Color::Black).bg(ACCENT),
                false if marked => style.fg(HIT),
                false => style,
            };
            let room = width.saturating_sub(right.chars().count());
            let mut left: String = left.chars().take(room).collect();
            let pad = room.saturating_sub(left.chars().count());
            left.push_str(&" ".repeat(pad));
            let right_style = if selected {
                style
            } else {
                Style::new().fg(MUTED)
            };
            out.push(Line::from(vec![
                Span::styled(left, style),
                Span::styled(right, right_style),
            ]));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tidy::{LinkAt, Orphan, Unresolved};
    use ratatui::crossterm::event::KeyModifiers;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn view() -> TidyView {
        let orphan = |note: &str, links_out, daily| Orphan {
            note: note.into(),
            links_out,
            daily,
        };
        TidyView::new(Report {
            orphans: vec![
                orphan("A.md", 0, false),
                orphan("B.md", 2, false),
                orphan("Journal/2026-11-05.md", 0, true),
            ],
            unused_attachments: vec!["x.png".into()],
            empty_folders: vec![],
            unresolved: vec![Unresolved {
                target: "Lahar".into(),
                from: vec![LinkAt {
                    note: "C.md".into(),
                    line: 3,
                    context: "[[Lahar]]".into(),
                }],
            }],
        })
    }

    #[test]
    fn rows_skip_headings_and_marks_delete_together() {
        let mut v = view();
        assert_eq!(v.rows()[v.sel], Row::Orphan(0));
        v.key(key(KeyCode::Char(' ')));
        assert_eq!(v.rows()[v.sel], Row::Orphan(2), "B has links out: hidden");
        v.key(key(KeyCode::Down));
        assert_eq!(v.rows()[v.sel], Row::Attachment(0), "past a heading");
        v.key(key(KeyCode::Char(' ')));
        assert!(matches!(
            v.key(key(KeyCode::Char('d'))),
            Action::Delete(p) if p == ["A.md", "x.png"]
        ));
        v.key(key(KeyCode::Char('a')));
        assert!(
            v.rows().contains(&Row::Orphan(1)),
            "a shows notes with links out"
        );
    }

    #[test]
    fn unresolved_rows_open_the_link_or_create_the_note() {
        let mut v = view();
        for _ in 0..10 {
            v.key(key(KeyCode::Down));
        }
        assert_eq!(v.rows()[v.sel], Row::Link(0, 0));
        assert!(matches!(
            v.key(key(KeyCode::Enter)),
            Action::OpenNote(n, Some(3)) if n == "C.md"
        ));
        assert!(matches!(
            v.key(key(KeyCode::Char('n'))),
            Action::Create { target, from } if target == "Lahar" && from == "C.md"
        ));
        assert!(matches!(v.key(key(KeyCode::Char('d'))), Action::Stay));
    }
}
