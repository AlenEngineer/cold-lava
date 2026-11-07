//! Pixel rendering through the Kitty graphics protocol (supported by Ghostty, Kitty, WezTerm).
//!
//! Each frame is rasterised with tiny-skia, zlib-compressed, and sent as an image placed under
//! the text layer (z = -1), so ratatui's labels and borders still draw on top of it.

use crate::scene::Scene;
use base64::Engine;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::io::Write;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

const MAX_PIXELS: f64 = 2_000_000.0;

pub struct Kitty {
    /// Cell size in pixels.
    pub cell_w: f64,
    pub cell_h: f64,
    next_id: u32,
    shown: Option<u32>,
}

/// Does this terminal look like it speaks the Kitty graphics protocol?
pub fn supported() -> bool {
    if std::env::var_os("TMUX").is_some() || std::env::var_os("ZELLIJ").is_some() {
        return false; // multiplexers don't pass the protocol through by default
    }
    let term = std::env::var("TERM").unwrap_or_default();
    let prog = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || term.contains("kitty")
        || term.contains("ghostty")
        || prog == "ghostty"
        || prog == "wezterm"
}

impl Kitty {
    /// Needs the terminal to report its pixel size (TIOCGWINSZ).
    pub fn new() -> Option<Kitty> {
        let mut k = Kitty {
            cell_w: 0.0,
            cell_h: 0.0,
            next_id: 7001,
            shown: None,
        };
        k.measure().then_some(k)
    }

    /// Re-read the cell size (call after a resize). Returns false if unknown.
    pub fn measure(&mut self) -> bool {
        match ratatui::crossterm::terminal::window_size() {
            Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => {
                self.cell_w = ws.width as f64 / ws.columns as f64;
                self.cell_h = ws.height as f64 / ws.rows as f64;
                true
            }
            _ => false,
        }
    }

    /// Height/width of a cell (2.0 is the classic assumption).
    pub fn cell_aspect(&self) -> f64 {
        self.cell_h / self.cell_w
    }

    /// Rasterise the scene and return the escape sequence that displays it over `area`.
    pub fn frame(&mut self, scene: &Scene, area: Rect) -> Vec<u8> {
        // Cap the work per frame (HiDPI screens can be 5+ megapixels); the terminal scales the
        // image to the c×r cell box, so rendering a bit smaller just softens it slightly.
        let full_w = area.width as f64 * self.cell_w;
        let full_h = area.height as f64 * self.cell_h;
        let scale = (MAX_PIXELS / (full_w * full_h).max(1.0)).sqrt().min(1.0);
        let w = (full_w * scale).round() as u32;
        let h = (full_h * scale).round() as u32;
        let Some(mut pm) = Pixmap::new(w.max(1), h.max(1)) else {
            return Vec::new();
        };
        let (x0, x1, y0, y1) = scene.view;
        let sx = w as f64 / (x1 - x0);
        let sy = h as f64 / (y1 - y0);
        let to_px = |x: f64, y: f64| (((x - x0) * sx) as f32, ((y1 - y) * sy) as f32);
        // Line width: thin like Obsidian, a bit thicker when zoomed in.
        let base_w = (sx * 0.22).clamp(1.0, 3.5) as f32;

        for layer in &scene.layers {
            for e in &layer.edges {
                let mut pb = PathBuilder::new();
                let (ax, ay) = to_px(e.a.0, e.a.1);
                let (bx, by) = to_px(e.b.0, e.b.1);
                pb.move_to(ax, ay);
                pb.line_to(bx, by);
                if let Some(path) = pb.finish() {
                    let stroke = Stroke {
                        width: if e.bold { base_w * 1.7 } else { base_w },
                        ..Stroke::default()
                    };
                    pm.stroke_path(
                        &path,
                        &paint(e.color, 255),
                        &stroke,
                        Transform::identity(),
                        None,
                    );
                }
            }
            for d in &layer.discs {
                let (cx, cy) = to_px(d.x, d.y);
                let r = (d.r * sx).max(2.0) as f32;
                if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
                    pm.fill_path(
                        &path,
                        &paint(d.color, 255),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                if let Some(rc) = d.ring
                    && let Some(path) = PathBuilder::from_circle(cx, cy, r + 3.0 + base_w)
                {
                    let stroke = Stroke {
                        width: base_w * 1.5,
                        ..Stroke::default()
                    };
                    pm.stroke_path(&path, &paint(rc, 255), &stroke, Transform::identity(), None);
                }
            }
        }

        // tiny-skia stores premultiplied alpha; the protocol wants straight alpha.
        let mut rgba = pm.take();
        for px in rgba.as_chunks_mut::<4>().0 {
            let a = px[3] as u32;
            if a != 0 && a != 255 {
                px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
                px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
                px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        let _ = z.write_all(&rgba);
        let compressed = z.finish().unwrap_or_default();
        let b64 = base64::engine::general_purpose::STANDARD.encode(compressed);

        // Alternate between ids: show the new image, then delete the old one (no flicker).
        let id = self.next_id;
        self.next_id = if id == 7001 { 7002 } else { 7001 };
        let mut out = Vec::with_capacity(b64.len() + 256);
        // Save cursor, move to the canvas origin.
        let _ = write!(out, "\x1b7\x1b[{};{}H", area.y + 1, area.x + 1);
        let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
        for (k, chunk) in chunks.iter().enumerate() {
            let more = if k + 1 < chunks.len() { 1 } else { 0 };
            if k == 0 {
                let _ = write!(
                    out,
                    "\x1b_Ga=T,f=32,o=z,s={w},v={h},i={id},p=1,q=2,C=1,z=-1,c={},r={},m={more};",
                    area.width, area.height
                );
            } else {
                let _ = write!(out, "\x1b_Gm={more};");
            }
            out.extend_from_slice(chunk);
            out.extend_from_slice(b"\x1b\\");
        }
        if let Some(old) = self.shown.replace(id) {
            let _ = write!(out, "\x1b_Ga=d,d=I,i={old},q=2\x1b\\");
        }
        out.extend_from_slice(b"\x1b8");
        out
    }

    /// Escape sequence that removes our images (before leaving or switching modes).
    pub fn clear(&mut self) -> Vec<u8> {
        self.shown = None;
        b"\x1b_Ga=d,d=I,i=7001,q=2\x1b\\\x1b_Ga=d,d=I,i=7002,q=2\x1b\\".to_vec()
    }
}

fn paint(c: Color, a: u8) -> Paint<'static> {
    let (r, g, b) = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::White => (230, 230, 230),
        Color::Gray => (160, 160, 160),
        Color::DarkGray => (90, 90, 90),
        _ => (180, 180, 180),
    };
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}
