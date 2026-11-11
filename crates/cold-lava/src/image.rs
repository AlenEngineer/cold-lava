//! The image view kind (`{"kind": "image", …}` on the view gateway, docs/extensions.md, "The
//! image view, as built"): a picture from the workspace or sent as bytes. A terminal with the
//! Kitty graphics protocol shows its pixels (`kitty::place_png`); any other shows its name, kind
//! and size, and a PNG as a thumbnail of half blocks, two pixels to a cell.

use std::path::Path;

use base64::Engine;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tiny_skia::Pixmap;

use crate::app::{MUTED, NOTE};

pub use cold_lava_views::image::{Image, MAX_BYTES, Source};

/// A picture read for drawing.
pub struct Picture {
    /// Its file's name, or its title.
    name: String,
    /// `PNG image · 64×48 · 1.2 KB`, or why it can't be shown.
    about: String,
    png: Option<Vec<u8>>,
    pixels: Option<Pixmap>,
}

impl Picture {
    /// `image` read from the workspace at `root`, when it names a file there.
    pub fn load(image: &Image, root: Option<&Path>) -> Picture {
        let (name, bytes) = match &image.source {
            Source::Path(rel) => {
                let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
                let bytes = match root {
                    None => Err("no workspace is open".to_string()),
                    Some(root) => crate::files::resolve(root, rel)
                        .map_err(|e| e.message)
                        .and_then(|p| read(&p)),
                };
                (name, bytes)
            }
            Source::Png(b64) => {
                let bytes = (base64::engine::general_purpose::STANDARD.decode(b64))
                    .map_err(|e| format!("its png isn't base64: {e}"));
                (image.title.clone(), bytes)
            }
        };
        let bytes = match bytes {
            Ok(b) => b,
            Err(why) => {
                return Picture {
                    name,
                    about: format!("can't show it: {why}"),
                    png: None,
                    pixels: None,
                };
            }
        };
        let is_png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
        let pixels = is_png.then(|| Pixmap::decode_png(&bytes).ok()).flatten();
        let kind = match (&pixels, name.rsplit_once('.')) {
            (Some(_), _) => "PNG image".to_string(),
            (None, Some((_, x))) => format!("{} image", x.to_uppercase()),
            (None, None) => "image".to_string(),
        };
        let mut about = vec![kind];
        if let Some(p) = &pixels {
            about.push(format!("{}×{}", p.width(), p.height()));
        }
        about.push(size(bytes.len()));
        Picture {
            name,
            about: about.join(" · "),
            png: pixels.is_some().then_some(bytes),
            pixels,
        }
    }

    /// The PNG to place with Kitty graphics, when it is one.
    pub fn png(&self) -> Option<&[u8]> {
        self.png.as_deref()
    }

    /// Its name and what it is, then, unless the terminal shows its pixels under the text
    /// (`pixels`), a thumbnail of half blocks in the lines left.
    pub fn lines(&self, width: usize, height: usize, pixels: bool) -> Vec<Line<'static>> {
        let mut out = vec![
            Line::from(Span::styled(
                self.name.clone(),
                Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(self.about.clone(), Style::new().fg(MUTED))),
        ];
        if let (Some(p), false) = (&self.pixels, pixels) {
            out.extend(thumbnail(p, width, height.saturating_sub(out.len())));
        }
        out.truncate(height);
        out
    }
}

/// Where the pixels go under a view `area` of a picture: below its two lines of text.
pub fn pixels_area(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let top = 2.min(area.height);
    ratatui::layout::Rect::new(area.x, area.y + top, area.width, area.height - top)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len as usize > MAX_BYTES {
        return Err(format!("it is over {} MB", MAX_BYTES >> 20));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// `p` in at most `cols` × `rows` cells, each cell an upper half block whose foreground is
/// the pixel above and background the pixel below, so a cell is about square.
fn thumbnail(p: &Pixmap, cols: usize, rows: usize) -> Vec<Line<'static>> {
    let (w, h) = (p.width() as usize, p.height() as usize);
    if cols == 0 || rows == 0 || w == 0 || h == 0 {
        return Vec::new();
    }
    let scale = (w as f64 / cols as f64)
        .max(h as f64 / (rows * 2) as f64)
        .max(1.0);
    let (tw, th) = (
        ((w as f64 / scale).round() as usize).max(1),
        ((h as f64 / scale).round() as usize).max(1),
    );
    let pixel = |x: usize, y: usize| {
        let (sx, sy) = (
            ((x as f64 + 0.5) * scale) as u32,
            ((y as f64 + 0.5) * scale) as u32,
        );
        let c = p.pixel(sx.min(p.width() - 1), sy.min(p.height() - 1));
        let c = c.map(|c| c.demultiply());
        match c {
            Some(c) if c.alpha() > 0 => Some(Color::Rgb(c.red(), c.green(), c.blue())),
            _ => None,
        }
    };
    (0..th.div_ceil(2))
        .map(|row| {
            let spans: Vec<Span<'static>> = (0..tw)
                .map(|x| {
                    let top = pixel(x, row * 2);
                    let bottom = (row * 2 + 1 < th).then(|| pixel(x, row * 2 + 1)).flatten();
                    let mut style = Style::new();
                    if let Some(t) = top {
                        style = style.fg(t);
                    }
                    if let Some(b) = bottom {
                        style = style.bg(b);
                    }
                    Span::styled("▀", style)
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` PNG, red on top and blue below.
    pub fn png(w: u32, h: u32) -> Vec<u8> {
        let mut p = Pixmap::new(w, h).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        let top = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, (h / 2) as f32).unwrap();
        let mut red = tiny_skia::Paint::default();
        red.set_color_rgba8(255, 0, 0, 255);
        p.fill_rect(top, &red, tiny_skia::Transform::identity(), None);
        p.encode_png().unwrap()
    }

    #[test]
    fn without_pixels_a_png_is_its_name_its_size_and_half_blocks() {
        let dir = std::env::temp_dir().join(format!("cold-lava-image-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Volcanoes")).unwrap();
        std::fs::write(dir.join("Volcanoes/lake.png"), png(8, 4)).unwrap();
        std::fs::write(dir.join("Volcanoes/lake.jpg"), b"\xff\xd8\xff not decoded").unwrap();
        let at = |rel: &str| Image {
            title: rel.into(),
            source: Source::Path(rel.into()),
        };
        let lake = Picture::load(&at("Volcanoes/lake.png"), Some(&dir));
        let lines = lake.lines(20, 10, false);
        assert_eq!(lines[0].to_string(), "lake.png");
        assert!(lines[1].to_string().starts_with("PNG image · 8×4 · "));
        assert_eq!(lines.len(), 4, "4 pixels high is 2 rows of half blocks");
        assert_eq!(lines[2].to_string(), "▀".repeat(8));
        let red_over_red = lines[2].spans[0].style;
        assert_eq!(
            (red_over_red.fg, red_over_red.bg),
            (Some(Color::Rgb(255, 0, 0)), Some(Color::Rgb(255, 0, 0)))
        );
        let blue = lines[3].spans[0].style;
        assert_eq!(blue.fg, Some(Color::Rgb(0, 0, 255)));
        assert_eq!(
            lake.lines(20, 10, true).len(),
            2,
            "pixels show under the text"
        );
        assert!(lake.png().is_some());

        let jpg = Picture::load(&at("Volcanoes/lake.jpg"), Some(&dir));
        let text: Vec<String> = jpg
            .lines(20, 10, false)
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(text, ["lake.jpg", "JPG image · 15 B"]);
        assert!(jpg.png().is_none(), "only PNG goes to the terminal");

        let outside = Picture::load(&at("../secret.png"), Some(&dir));
        assert!(
            outside.lines(40, 4, false)[1]
                .to_string()
                .starts_with("can't show it: ")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
