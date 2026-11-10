//! The palette, and how the graph and pictures are drawn: Kitty pixels or text.

use ratatui::style::Color;

// Palette (truecolor; Ghostty, Kitty, WezTerm, iTerm2 all support it).
pub const ACCENT: Color = Color::Rgb(167, 139, 250);
pub const ACCENT_SOFT: Color = Color::Rgb(214, 200, 255);
pub const NOTE: Color = Color::Rgb(205, 205, 215);
pub const DIM: Color = Color::Rgb(85, 85, 98);
pub const EDGE: Color = Color::Rgb(68, 68, 82);
pub const GHOST: Color = Color::Rgb(115, 115, 128);
pub const TAG: Color = Color::Rgb(120, 200, 145);
pub const HIT: Color = Color::Rgb(250, 200, 85);
pub const MUTED: Color = Color::Rgb(140, 140, 155);

#[derive(Clone, Copy)]
pub enum Render {
    Auto,
    Braille,
    Pixels,
}
