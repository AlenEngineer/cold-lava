//! Where the menu's popups go and the frames they are drawn in: the prompts, the pickers and
//! the dialogs that confirm a change.

use crate::app::{ACCENT, MUTED, NOTE};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

/// A `w`×`h` popup centred across `area`, `top` rows down from its top.
pub fn centred(area: Rect, w: u16, h: u16, top: u16) -> Rect {
    Rect::new(area.x + (area.width - w) / 2, area.y + top, w, h)
}

/// The width of a dialog that confirms a change (rename, merge, delete) in `area`.
pub fn width(area: Rect) -> u16 {
    (area.width * 3 / 4).clamp(50.min(area.width), 120)
}

/// Where the rename and the merge dialogs go in `area`.
pub fn confirm(area: Rect) -> Rect {
    let h = (area.height * 2 / 3).clamp(10.min(area.height), 34);
    centred(area, width(area), h, area.height / 6)
}

/// Clear `r` and draw a popup's frame on it: a rounded border in `colour`, the title in bold,
/// the hint along the bottom. Returns what is inside the border.
pub fn frame(f: &mut Frame, r: Rect, title: &str, hint: &str, colour: Color) -> Rect {
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(colour))
        .title(Line::styled(
            title,
            Style::new().fg(colour).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(hint, Style::new().fg(MUTED)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

/// The centred popup shared by the quick switcher, the palette, the outline, the bookmarks,
/// tidy and the recipes; returns its inside.
pub fn picker(f: &mut Frame, area: Rect, title: &str, hint: &str) -> Rect {
    let w = (area.width * 2 / 3).clamp(40.min(area.width), 100);
    let h = (area.height * 2 / 3).clamp(8.min(area.height), 30);
    frame(f, centred(area, w, h, area.height / 6), title, hint, ACCENT)
}

/// Draw a one-line prompt in the upper middle of `area`; returns where it went.
pub fn prompt(f: &mut Frame, area: Rect, title: &str, label: &str, value: &str) -> Rect {
    let w = 60.min(area.width.saturating_sub(4));
    let r = centred(area, w, 3, area.height / 3);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label.to_string(), Style::new().fg(MUTED)),
            Span::styled(format!("{value}▏"), Style::new().fg(NOTE)),
        ]))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(title.to_string()),
        ),
        r,
    );
    r
}

/// A `w`×`h` popup under the point `at`, or above it when there is more room there, kept on
/// `screen` and shrunk to fit.
pub fn near(at: (u16, u16), w: u16, h: u16, screen: Rect) -> Rect {
    let (x, y) = at;
    let below = screen.bottom().saturating_sub(y + 1);
    let above = y.saturating_sub(screen.y);
    let (top, h) = if below >= h || below >= above {
        (y + 1, h.min(below))
    } else {
        (y - h.min(above), h.min(above))
    };
    let w = w.min(screen.width);
    let left = x.saturating_sub(2).clamp(screen.x, screen.right() - w);
    Rect::new(left, top, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn peek_opens_under_the_link_or_above_it_near_the_bottom() {
        let screen = Rect::new(0, 0, 100, 40);
        assert_eq!(near((30, 5), 60, 16, screen), Rect::new(28, 6, 60, 16));
        assert_eq!(near((90, 35), 60, 16, screen), Rect::new(40, 19, 60, 16));
        let small = Rect::new(0, 0, 30, 10);
        let r = near((0, 4), 60, 16, small);
        assert_eq!(r, Rect::new(0, 5, 30, 5), "shrunk to the room below");
    }

    #[test]
    fn a_confirm_dialog_is_centred_a_sixth_down_and_kept_inside_a_small_screen() {
        assert_eq!(confirm(Rect::new(0, 0, 120, 42)), Rect::new(15, 7, 90, 28));
        assert_eq!(
            confirm(Rect::new(0, 1, 200, 80)),
            Rect::new(40, 14, 120, 34),
            "at most"
        );
        assert_eq!(
            confirm(Rect::new(0, 0, 40, 9)),
            Rect::new(0, 1, 40, 9),
            "at least"
        );
        assert_eq!(width(Rect::new(0, 0, 60, 9)), 50);
    }

    #[test]
    fn a_frame_clears_its_place_and_gives_the_inside_of_its_border() {
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        let mut inside = Rect::default();
        terminal
            .draw(|f| {
                f.render_widget(Paragraph::new(vec![Line::raw("x".repeat(20)); 5]), f.area());
                inside = frame(f, Rect::new(1, 1, 18, 4), " T ", " h ", ACCENT);
            })
            .unwrap();
        assert_eq!(inside, Rect::new(2, 2, 16, 2));
        let buffer = terminal.backend().buffer();
        let row = |y: u16| {
            (0..20u16)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };
        assert_eq!(row(1), "x╭ T ─────────────╮x");
        assert_eq!(row(2), "x│                │x");
        assert_eq!(row(0), "x".repeat(20), "outside is left alone");
        assert_eq!(row(4), "x╰ h ─────────────╯x");
        assert!(
            buffer[(3u16, 1u16)].modifier.contains(Modifier::BOLD),
            "the title is bold"
        );
        assert_eq!(buffer[(3u16, 4u16)].fg, MUTED, "the hint is muted");
    }
}
