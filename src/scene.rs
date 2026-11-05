//! A backend-neutral description of one frame of the graph, drawn either with braille
//! characters (any terminal) or as real pixels (Kitty graphics protocol).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::canvas::{Circle, Context, Line as CLine, Points};

pub struct Edge {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub color: Color,
    pub bold: bool,
}

pub struct Disc {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub color: Color,
    /// Draw a ring around it (selection).
    pub ring: Option<Color>,
}

pub struct Label {
    /// World position of the node centre and its radius; the label goes just below it.
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub text: String,
    pub style: Style,
}

#[derive(Default)]
pub struct Layer {
    pub edges: Vec<Edge>,
    pub discs: Vec<Disc>,
}

pub struct Scene {
    /// World-space view rectangle: x0, x1, y0, y1 (y up).
    pub view: (f64, f64, f64, f64),
    /// World units -> braille dots.
    pub zoom: f64,
    /// Drawn back to front.
    pub layers: Vec<Layer>,
    pub labels: Vec<Label>,
}

impl Scene {
    pub fn paint_braille(&self, ctx: &mut Context) {
        for layer in &self.layers {
            for e in &layer.edges {
                ctx.draw(&CLine::new(e.a.0, e.a.1, e.b.0, e.b.1, e.color));
            }
            ctx.layer();
            for d in &layer.discs {
                // Filled disc: concentric rings spaced about one braille dot apart.
                let rings = (d.r * self.zoom).ceil().clamp(1.0, 12.0) as usize;
                for k in 1..=rings {
                    ctx.draw(&Circle::new(d.x, d.y, d.r * k as f64 / rings as f64, d.color));
                }
                ctx.draw(&Points::new(&[(d.x, d.y)], d.color));
                if let Some(c) = d.ring {
                    ctx.draw(&Circle::new(d.x, d.y, d.r + 2.5 / self.zoom, c));
                }
            }
            ctx.layer();
        }
    }

    /// Writes labels straight into the cell buffer (both backends).
    pub fn draw_labels(&self, buf: &mut Buffer, area: Rect) {
        let (x0, x1, y0, y1) = self.view;
        if area.width == 0 || area.height == 0 {
            return;
        }
        for l in &self.labels {
            let col = (l.x - x0) / (x1 - x0) * area.width as f64;
            // First full cell row below the node's bottom edge.
            let bottom = (y1 - (l.y - l.r)) / (y1 - y0) * area.height as f64;
            let len = l.text.chars().count() as f64;
            let start = (col - len / 2.0).round();
            let row = (bottom + 0.15).ceil();
            if row < 0.0 || row >= area.height as f64 {
                continue;
            }
            let y = area.y + row as u16;
            for (k, ch) in l.text.chars().enumerate() {
                let c = start + k as f64;
                if c < 0.0 || c >= area.width as f64 {
                    continue;
                }
                let cell = &mut buf[(area.x + c as u16, y)];
                cell.set_char(ch);
                cell.set_style(l.style);
            }
        }
    }
}
