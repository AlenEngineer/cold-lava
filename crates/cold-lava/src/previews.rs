//! Previews of embedded files (`![[Eruptions.csv]]` in a note): what each file's owner answered
//! to `file/preview`, kept by the file's version (docs/extensions.md, "Previews, as built"). The
//! reader asks while it lays a note out and never waits: until the answer comes it draws the
//! preview it had for an older version, else a line saying it is asking.

use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::MUTED;
use crate::ext::view::Content;

/// The lines a preview takes in the reader, at most.
pub const HEIGHT: u16 = 8;

#[derive(Default)]
pub struct Previews {
    files: HashMap<String, Preview>,
    /// Asked for while drawing, sent before the next frame: (path, rev, width).
    wanted: Vec<(String, String, u16)>,
}

struct Preview {
    /// The version asked for, or answered.
    rev: String,
    answer: Option<Result<Content, String>>,
    /// The last content its owner gave, for any version: drawn while a newer one is asked.
    last: Option<Content>,
}

impl Previews {
    /// What to draw for the file at `path`, at version `rev`, `width` columns wide: its
    /// owner's view, or a line saying why there is none yet. Asks for it when it hasn't.
    pub fn lines(
        &mut self,
        path: &str,
        rev: &str,
        owner: &str,
        width: u16,
        root: Option<&std::path::Path>,
    ) -> Vec<Line<'static>> {
        let p = self.files.get(path);
        let asked = p.is_some_and(|p| p.rev == rev);
        if !asked && !self.wanted.iter().any(|(w, r, _)| w == path && r == rev) {
            self.wanted.push((path.to_string(), rev.to_string(), width));
        }
        let note = |text: String| vec![Line::from(Span::styled(text, Style::new().fg(MUTED)))];
        match p {
            Some(Preview {
                answer: Some(Ok(content)),
                ..
            }) if asked => crate::views::preview(content, root, width as usize, HEIGHT as usize),
            Some(Preview {
                answer: Some(Err(why)),
                ..
            }) if asked => note(format!("no preview: {why}")),
            Some(Preview {
                last: Some(content),
                ..
            }) => crate::views::preview(content, root, width as usize, HEIGHT as usize),
            _ => note(format!("asking {owner} for a preview…")),
        }
    }

    /// The previews asked for since the last call, each now waiting for its owner.
    pub fn take_wanted(&mut self) -> Vec<(String, String, u16)> {
        let wanted = std::mem::take(&mut self.wanted);
        for (path, rev, _) in &wanted {
            let p = self.files.entry(path.clone()).or_insert(Preview {
                rev: rev.clone(),
                answer: None,
                last: None,
            });
            p.rev = rev.clone();
            p.answer = None;
        }
        wanted
    }

    /// The owner's answer for `path` at `rev`. Whether the reader should draw again: an answer
    /// for a version no longer asked for is dropped.
    pub fn answer(&mut self, path: &str, rev: &str, content: Result<Content, String>) -> bool {
        let Some(p) = self.files.get_mut(path).filter(|p| p.rev == rev) else {
            return false;
        };
        if let Ok(c) = &content {
            p.last = Some(c.clone());
        }
        p.answer = Some(content);
        true
    }

    /// Whether a note may show a preview of `path`, so a change to it should redraw.
    pub fn has(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::{Group, List, Row};

    fn list(text: &str) -> Content {
        Content::List(List {
            title: "Eruptions".into(),
            groups: vec![Group {
                id: "g".into(),
                title: "Eruptions".into(),
                collapsed: false,
                rows: vec![Row {
                    id: "1".into(),
                    text: vec![crate::list::Piece::plain(text)],
                    check: None,
                    right: Vec::new(),
                    depth: 0,
                    look: None,
                }],
            }],
            keys: Vec::new(),
            empty: None,
            filter: Default::default(),
        })
    }

    fn text(lines: &[Line]) -> String {
        let lines: Vec<String> = lines
            .iter()
            .map(|l| l.to_string().trim_end().into())
            .collect();
        lines.join("\n")
    }

    #[test]
    fn a_preview_is_asked_once_per_version_and_the_last_one_shows_meanwhile() {
        let mut p = Previews::default();
        let csv = "Volcanoes/Eruptions.csv";
        assert_eq!(
            text(&p.lines(csv, "r1", "csv-table", 40, None)),
            "asking csv-table for a preview…"
        );
        p.lines(csv, "r1", "csv-table", 40, None);
        assert_eq!(
            p.take_wanted(),
            [(csv.to_string(), "r1".to_string(), 40)],
            "once"
        );
        assert!(p.take_wanted().is_empty());
        assert!(
            !p.answer(csv, "r0", Ok(list("old"))),
            "an answer for another version"
        );
        assert!(p.answer(csv, "r1", Ok(list("Krakatoa 1883"))));
        assert!(text(&p.lines(csv, "r1", "csv-table", 40, None)).contains("Krakatoa 1883"));
        assert!(p.take_wanted().is_empty(), "answered: nothing to ask");

        assert!(text(&p.lines(csv, "r2", "csv-table", 40, None)).contains("Krakatoa 1883"));
        assert_eq!(p.take_wanted().len(), 1, "a new version is asked for");
        assert!(text(&p.lines(csv, "r2", "csv-table", 40, None)).contains("Krakatoa 1883"));
        assert!(p.answer(csv, "r2", Err("it stopped".into())));
        assert_eq!(
            text(&p.lines(csv, "r2", "csv-table", 40, None)),
            "no preview: it stopped"
        );
        assert!(p.has(csv) && !p.has("Other.csv"));
    }
}
