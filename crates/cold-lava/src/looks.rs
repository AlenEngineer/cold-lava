//! Types' looks in the reader (docs/extensions.md, "Looks, as built"): which text of a note each
//! look covers, and the one table that turns a colour's name into a terminal colour.

use std::collections::HashMap;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use serde_json::Value as Json;

use crate::app::{ACCENT, MUTED, NOTE};
use crate::items::{Data, Item};
use crate::markdown::{self, Doc, Flavour, Inline};
use crate::types::{Colour, Definition, Look, On, Types};

/// One value (or one whole item) a look covers. Its index is the id of its `Inline::Mark`s.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// The look for this value: `by-value` already picked.
    pub look: Look,
    pub field: String,
    /// The value as text, for a chip that replaces hidden text, the properties line, and the
    /// target of a `link` look.
    pub value: String,
}

/// The looks of one note, worked out once when it is read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Looks {
    pub marks: Vec<Mark>,
    /// Marks of the note's own type that its body doesn't show (frontmatter, its name): drawn
    /// on a line above the note.
    pub header: Vec<usize>,
}

/// The note `text`, parsed with a mark around every value whose type gives it a look.
pub fn read(text: &str, flavour: &Flavour, items: &[Item], types: &Types) -> (Doc, Looks) {
    let defs: HashMap<&str, &Definition> = types
        .types
        .iter()
        .map(|t| (t.name.as_str(), &t.definition))
        .collect();
    let mut marks = Vec::new();
    let mut ranges = Vec::new();
    let mut of_note = Vec::new();
    for item in items {
        let Data::Typed(t) = &item.data else { continue };
        let chain: Vec<&Definition> = std::iter::once(&t.kind)
            .chain(&t.is)
            .filter_map(|name| defs.get(name.as_str()).copied())
            .collect();
        let Some(own) = chain.first() else { continue };
        let note = own
            .from
            .get(t.place.shape)
            .is_some_and(|shape| shape.on == On::Note);
        for (field, value) in &t.fields {
            let Some(look) = chain.iter().find_map(|d| d.style.get(&**field)) else {
                continue;
            };
            let values = match value {
                Json::Array(values) => values.as_slice(),
                Json::Null => &[],
                one => std::slice::from_ref(one),
            };
            let spans = t.spans.get(field);
            for (i, value) in values.iter().enumerate() {
                let Some(look) = for_value(look, value) else {
                    continue;
                };
                let range = if look.whole {
                    // A whole note is no span of text to style.
                    if note {
                        continue;
                    }
                    t.place.span.clone()
                } else {
                    spans.and_then(|s| s.get(i)).cloned().unwrap_or(0..0)
                };
                if note {
                    of_note.push(marks.len());
                }
                ranges.push(range);
                marks.push(Mark {
                    look,
                    field: field.to_string(),
                    value: shown(value),
                });
            }
        }
    }
    let doc = markdown::parse_marked(text, flavour, &ranges);
    let mut placed = vec![false; marks.len()];
    for_marks(&doc.blocks, &mut |id| placed[id] = true);
    let header = of_note
        .into_iter()
        .filter(|&id| !placed[id] && !doc.broken[id] && !hidden(&marks[id].look))
        .collect();
    (doc, Looks { marks, header })
}

/// The look for one value: the field's, with its `by-value` look for this value over it. A
/// look given only by value is no look for the other values.
fn for_value(look: &Look, value: &Json) -> Option<Look> {
    if look.by_value.is_empty() {
        return Some(look.clone());
    }
    let over = value.as_str().and_then(|v| look.by_value.get(v));
    let base = Look {
        by_value: Default::default(),
        ..look.clone()
    };
    match over {
        Some(over) => Some(Look {
            colour: over.colour.or(base.colour),
            badge: base.badge || over.badge,
            icon: over.icon.clone().or(base.icon),
            dim: base.dim || over.dim,
            strike: base.strike || over.strike,
            bold: base.bold || over.bold,
            hide: base.hide || over.hide,
            link: base.link || over.link,
            whole: base.whole || over.whole,
            by_value: Default::default(),
        }),
        None if base == Look::default() => None,
        None => Some(base),
    }
}

/// Hidden with nothing drawn in its place.
fn hidden(look: &Look) -> bool {
    look.hide && !look.badge && look.icon.is_none()
}

fn shown(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        Json::Bool(true) => "yes".into(),
        Json::Bool(false) => "no".into(),
        other => other.to_string(),
    }
}

fn for_marks(blocks: &[markdown::Block], f: &mut impl FnMut(usize)) {
    use markdown::BlockKind as K;
    for b in blocks {
        match &b.kind {
            K::Heading { inlines, .. } | K::Paragraph(inlines) => inlines_marks(inlines, f),
            K::List { items, .. } => items.iter().for_each(|i| for_marks(&i.blocks, f)),
            K::Quote(blocks) | K::FootnoteDef { blocks, .. } => for_marks(blocks, f),
            K::Callout { title, blocks, .. } => {
                inlines_marks(title, f);
                for_marks(blocks, f);
            }
            K::Table { header, rows } => header
                .iter()
                .chain(rows.iter().flatten())
                .for_each(|c| inlines_marks(c, f)),
            K::Code { .. } | K::Rule | K::Math(_) | K::Embed { .. } => {}
        }
    }
}

fn inlines_marks(inlines: &[Inline], f: &mut impl FnMut(usize)) {
    for i in inlines {
        match i {
            Inline::Mark { id, .. } => f(*id),
            Inline::Strong(c) | Inline::Emph(c) | Inline::Strike(c) | Inline::Highlight(c) => {
                inlines_marks(c, f)
            }
            Inline::Link { text, .. } => inlines_marks(text, f),
            _ => {}
        }
    }
}

/// Whether colours are off (https://no-color.org): looks then draw with modifiers only.
fn no_colour() -> bool {
    static OFF: OnceLock<bool> = OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()))
}

/// The one place a colour's name becomes a terminal colour: Obsidian's canvas colours, the
/// accent and a muted grey. A light terminal would swap this table.
pub fn colour(c: Colour) -> Color {
    match c {
        Colour::Red => Color::Rgb(233, 73, 73),
        Colour::Orange => Color::Rgb(233, 151, 63),
        Colour::Yellow => Color::Rgb(224, 222, 113),
        Colour::Green => Color::Rgb(68, 207, 110),
        Colour::Cyan => Color::Rgb(83, 223, 221),
        Colour::Purple => Color::Rgb(168, 130, 255),
        Colour::Accent => ACCENT,
        Colour::Muted => MUTED,
    }
}

/// A chip's background: its colour, darkened.
fn chip_background(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let dark = |v: u8| (v as u16 * 30 / 100 + 18) as u8;
            Color::Rgb(dark(r), dark(g), dark(b))
        }
        other => other,
    }
}

/// `style` with `look` on it. A badge is drawn as a chip: its colour on a dark tint of it.
pub fn apply(style: Style, look: &Look) -> Style {
    let mut style = style;
    let colour = look.colour.map(colour);
    if no_colour() {
        if look.badge {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if look.dim {
            style = style.add_modifier(Modifier::DIM);
        }
    } else {
        if let Some(c) = colour {
            style = style.fg(c);
        }
        if look.dim {
            style = match colour {
                Some(_) => style.add_modifier(Modifier::DIM),
                None => style.fg(crate::app::DIM),
            };
        }
        if look.badge {
            let fg = colour.unwrap_or(NOTE);
            style = style.fg(fg).bg(chip_background(fg));
        }
    }
    if look.strike {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if look.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Settings;
    use std::path::Path;

    fn example() -> Settings {
        Settings::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"))
    }

    fn read_note(note: &str, text: &str) -> (Doc, Looks) {
        let s = example();
        let items = crate::items::extract(note, text, &s);
        read(text, &s.flavour, &items, &s.types)
    }

    /// Each placed mark's field and the text it covers.
    fn covered(doc: &Doc, looks: &Looks) -> Vec<(String, String)> {
        fn walk(inlines: &[Inline], open: &mut Vec<usize>, out: &mut Vec<(usize, String)>) {
            for i in inlines {
                match i {
                    Inline::Mark { id, open: true } => {
                        open.push(*id);
                        out.push((*id, String::new()));
                    }
                    Inline::Mark { id, open: false } => open.retain(|o| o != id),
                    Inline::Text(t) => {
                        for id in open.iter() {
                            if let Some(o) = out.iter_mut().rev().find(|o| o.0 == *id) {
                                o.1.push_str(t);
                            }
                        }
                    }
                    Inline::Link { text, .. } | Inline::Strong(text) | Inline::Emph(text) => {
                        walk(text, open, out)
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        let mut each = |inlines: &[Inline]| walk(inlines, &mut Vec::new(), &mut out);
        fn blocks(b: &[markdown::Block], each: &mut dyn FnMut(&[Inline])) {
            use markdown::BlockKind as K;
            for b in b {
                match &b.kind {
                    K::Paragraph(i) | K::Heading { inlines: i, .. } => each(i),
                    K::List { items, .. } => items.iter().for_each(|i| blocks(&i.blocks, each)),
                    K::Table { rows, .. } => rows.iter().flatten().for_each(|c| each(c)),
                    _ => {}
                }
            }
        }
        blocks(&doc.blocks, &mut each);
        out.into_iter()
            .filter(|(id, _)| !doc.broken[*id])
            .map(|(id, t)| (looks.marks[id].field.clone(), t))
            .collect()
    }

    #[test]
    fn inline_values_cover_their_text() {
        let (doc, looks) = read_note(
            "Volcanoes/Krakatoa.md",
            "Its 1883 eruption (VEI 6, see [[Volcanic Explosivity Index]]) was loud.\n",
        );
        assert_eq!(covered(&doc, &looks), [("vei".into(), "VEI 6".into())]);
        assert_eq!(looks.marks[0].look.colour, Some(Colour::Orange));
    }

    #[test]
    fn table_cells_and_tasks_are_marked() {
        let (doc, looks) = read_note(
            "Concepts/VEI.md",
            "| VEI | Example |\n| --- | --- |\n| 6 | [[Krakatoa]] 1883 |\n",
        );
        assert_eq!(covered(&doc, &looks), [("vei".into(), "6".into())]);
        let (doc, looks) = read_note(
            "Todo.md",
            "- [x] Read about [[Lava]] **flows** ✅ 2026-11-05\n- [ ] Plan ⏫\n",
        );
        assert_eq!(
            covered(&doc, &looks),
            [
                (
                    "status_type".into(),
                    "Read about Lava flows ✅ 2026-11-05".into()
                ),
                ("priority".into(), "⏫".into()),
            ]
        );
    }

    #[test]
    fn frontmatter_values_go_on_the_properties_line() {
        let (doc, looks) = read_note(
            "Volcanoes/Fuji.md",
            "---\ntags: [volcano]\nelevation_m: 3776\n---\n# Fuji\n",
        );
        assert!(covered(&doc, &looks).is_empty());
        let header: Vec<_> = looks.header.iter().map(|&i| &looks.marks[i]).collect();
        assert_eq!(header.len(), 1);
        assert_eq!(
            (&*header[0].field, &*header[0].value),
            ("elevation", "3776")
        );
    }

    #[test]
    fn a_mark_that_would_cut_a_tag_or_land_in_a_comment_is_skipped() {
        let s = example();
        let marks = [4..8, 20..26];
        let doc = markdown::parse_marked("see #volcano and %%hid den%% x\n", &s.flavour, &marks);
        assert_eq!(doc.broken, [true, true]);
        let doc = markdown::parse_marked(
            "see #volcano and x\n",
            &s.flavour,
            std::slice::from_ref(&(4..12)),
        );
        assert_eq!(doc.broken, [false]);
    }
}
