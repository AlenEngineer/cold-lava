//! The which-key popup: a screen's commands in columns by group, each with its key, at the
//! bottom of the screen. Every screen builds its lines from its own command table and keymap
//! (`commands::key_hints`), so the popup shows the keys config.toml set.

use crate::app::{ACCENT, MUTED, NOTE};
use crate::commands::{Group, KeyHint};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::collections::BTreeMap;

const GAP: usize = 4;

/// One group's column: its title, then `key  name` rows with the keys right-aligned.
struct Column<'a> {
    title: &'static str,
    rows: Vec<(String, &'a str)>,
    key_width: usize,
    width: usize,
}

fn width(s: &str) -> usize {
    Span::raw(s).width()
}

fn columns(hints: &[KeyHint]) -> Vec<Column<'_>> {
    let mut by_group: BTreeMap<Group, Vec<&KeyHint>> = BTreeMap::new();
    for h in hints {
        by_group.entry(h.group).or_default().push(h);
    }
    by_group
        .into_iter()
        .map(|(group, hints)| {
            let rows: Vec<(String, &str)> = hints
                .iter()
                .map(|h| (h.key.to_string(), h.name.as_str()))
                .collect();
            let key_width = rows.iter().map(|(k, _)| width(k)).max().unwrap_or(0);
            let row_width = rows.iter().map(|(_, n)| key_width + 2 + width(n)).max();
            Column {
                title: group.title(),
                width: row_width.unwrap_or(0).max(width(group.title())),
                rows,
                key_width,
            }
        })
        .collect()
}

/// Columns side by side, starting a new band below when the next one doesn't fit in `room`.
fn bands(columns: Vec<Column<'_>>, room: usize) -> Vec<Vec<Column<'_>>> {
    let mut bands: Vec<Vec<Column>> = Vec::new();
    let mut used = 0;
    for c in columns {
        match bands.last_mut() {
            Some(band) if used + GAP + c.width <= room => {
                used += GAP + c.width;
                band.push(c);
            }
            _ => {
                used = c.width;
                bands.push(vec![c]);
            }
        }
    }
    bands
}

fn lines<'a>(bands: &[Vec<Column<'a>>]) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    for (i, band) in bands.iter().enumerate() {
        if i > 0 {
            out.push(Line::raw(""));
        }
        let height = band.iter().map(|c| c.rows.len()).max().unwrap_or(0);
        let mut title = Vec::new();
        for c in band {
            title.push(Span::styled(
                format!("{:<w$}", c.title, w = c.width + GAP),
                Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
            ));
        }
        out.push(Line::from(title));
        for r in 0..height {
            let mut spans = Vec::new();
            for c in band {
                let (key, name) = c.rows.get(r).map_or(("", ""), |(k, n)| (k.as_str(), *n));
                let pad = c.key_width - width(key);
                spans.push(Span::styled(
                    format!("{}{key}  ", " ".repeat(pad)),
                    Style::new().fg(ACCENT),
                ));
                let rest = c.width + GAP - c.key_width - 2;
                spans.push(Span::styled(
                    format!("{name:<rest$}"),
                    Style::new().fg(NOTE),
                ));
            }
            out.push(Line::from(spans));
        }
    }
    out
}

/// The popup over the bottom of `area`, as wide as its columns need.
pub fn draw(f: &mut Frame, area: Rect, hints: &[KeyHint]) {
    let room = area.width.saturating_sub(4) as usize;
    let bands = bands(columns(hints), room);
    let content = bands
        .iter()
        .map(|b| b.iter().map(|c| c.width + GAP).sum::<usize>() - GAP)
        .max()
        .unwrap_or(0);
    let lines = lines(&bands);
    let w = (content as u16 + 4).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.bottom() - h, w, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(" keys · type one to run it · Esc closes "),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Key;

    fn hint(group: Group, key: &str, name: &str) -> KeyHint {
        KeyHint {
            group,
            key: Key::parse(key).unwrap(),
            name: name.into(),
        }
    }

    #[test]
    fn groups_become_columns_in_group_order_and_wrap_into_bands() {
        let hints = [
            hint(Group::App, "?", "help"),
            hint(Group::Move, "j", "down"),
            hint(Group::App, "q", "quit"),
            hint(Group::Note, "ctrl+e", "edit"),
        ];
        let cols = columns(&hints);
        let titles: Vec<&str> = cols.iter().map(|c| c.title).collect();
        assert_eq!(titles, ["move", "note", "app"]);
        assert_eq!(cols[1].width, "Ctrl-e  edit".len());
        let rendered: Vec<String> = lines(&bands(columns(&hints), 80))
            .iter()
            .map(|l| l.to_string().trim_end().to_string())
            .collect();
        assert_eq!(
            rendered,
            [
                "move       note            app",
                "j  down    Ctrl-e  edit    ?  help",
                "                           q  quit",
            ]
        );
        assert_eq!(
            bands(columns(&hints), 30).len(),
            2,
            "no room for a third column"
        );
    }
}
