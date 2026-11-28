//! The types matched over what a walk found: each block some type reads is typed, or holds
//! inline types, and becomes an item or types the item it already is.

use super::{Lines, Pass, Raw, keys};
use crate::frontmatter;
use crate::items::{Data, Settings};
use crate::markdown::Source;
use crate::matcher;
use crate::sources::On;

/// Matches the types over what a pass found: its items, each typed where a type reads it, and
/// the typed items made of what no item was, before any has an id.
pub(super) fn finish(
    note: &str,
    text: &str,
    s: &Settings,
    source: &Source,
    lines: &Lines,
    pass: Pass,
) -> Vec<Raw> {
    let Pass {
        mut raws,
        mut candidates,
        code,
        tables,
        tags,
        links,
        written,
    } = pass;
    let m = &s.matcher;
    if !m.is_empty() {
        let src = lines.src;
        let want_inline = m.reads(On::Inline);
        let mut found = note_of(note, text, source, m.needs_links);
        for tag in &tags {
            found.add_tag(tag);
        }
        found.links.extend(links);
        found.written = written;
        let gate = m.gate(&found);
        let mut note_is: Vec<String> = Vec::new();
        if let Some((typed, fields)) = m.note(&gate, &found) {
            let key = keys::typed(&typed, fields, On::Document, found.body.0);
            note_is.push(typed.kind.clone());
            note_is.extend(typed.is.iter().cloned());
            raws.push(Raw {
                line: 1,
                end_line: found.frontmatter.as_ref().map_or(1, |f| f.end_line),
                section: Vec::new(),
                parent: None,
                block_id: None,
                data: Data::Typed(Box::new(typed)),
                key,
            });
        }
        // In document order, so a list item is typed before the items inside it ask.
        candidates.sort_by_key(|c| c.text.start);
        // (type, where) of each inline match, so a block inside a list item's lines (a quote,
        // a table) doesn't find it twice.
        let mut inline_seen: std::collections::HashSet<(String, usize)> = Default::default();
        for c in candidates {
            // What it sits inside, when some type asks. A piece is there under the name it had
            // before the roles were named (`On::alias`), since this list also holds kinds and
            // type names: `where.inside = ["item"]` must still mean a type called `item`. The
            // matcher maps a role written there to that name.
            let ask = m.needs_inside;
            let mut inside = if ask { note_is.clone() } else { Vec::new() };
            if ask && c.in_quote {
                inside.push(On::Aside.alias().into());
            }
            if let Some(p) = c.parent.filter(|_| ask) {
                inside.push(On::Item.alias().into());
                inside.push(raws[p].data.kind().to_string());
                if let Data::Typed(t) = &raws[p].data {
                    inside.extend(t.is.iter().cloned());
                }
            }
            let matched = c
                .typeable
                .then(|| {
                    m.block(
                        &gate,
                        &found,
                        &matcher::Block {
                            on: c.on,
                            text: &src[c.text.clone()],
                            at: c.text.start,
                            section: &c.section,
                            header: tables.get(c.table).map_or(&[], Vec::as_slice),
                            cells: &c.cells,
                            inside: inside.iter().map(String::as_str).collect(),
                            code: &code,
                            given: c.given.clone(),
                        },
                    )
                })
                .flatten()
                .map(|(typed, fields)| {
                    let words = match c.on {
                        On::Row => c.cells.first().map_or("", |cell| &src[cell.clone()]),
                        _ => &src[c.text.clone()],
                    };
                    let key = keys::typed(&typed, fields, c.on, words);
                    (typed, key)
                });
            // The item the block is, which its inline items name as their parent.
            let mut host = c.raw;
            if let Some((typed, key)) = matched {
                match c.raw {
                    Some(i) => {
                        raws[i].data = Data::Typed(Box::new(typed));
                        raws[i].key = key;
                    }
                    None => {
                        host = Some(raws.len());
                        raws.push(Raw {
                            line: c.line,
                            end_line: c.end_line,
                            section: c.section.clone(),
                            parent: c.parent,
                            block_id: c.block_id.clone(),
                            data: Data::Typed(Box::new(typed)),
                            key: match c.on {
                                On::Aside => keys::aside(&key),
                                _ => key,
                            },
                        });
                    }
                }
            }
            if !want_inline {
                continue;
            }
            if ask {
                inside.push(c.on.alias().into());
            }
            if let Some(h) = host.filter(|_| ask) {
                inside.push(raws[h].data.kind().to_string());
                if let Data::Typed(t) = &raws[h].data {
                    inside.extend(t.is.iter().cloned());
                }
            }
            let block = matcher::Block {
                on: On::Inline,
                text: &src[c.own.clone()],
                at: c.own.start,
                section: &c.section,
                header: &[],
                cells: &[],
                inside: inside.iter().map(String::as_str).collect(),
                code: &code,
                given: matcher::Given::default(),
            };
            for (mut typed, fields, r) in m.inline(&gate, &found, &block) {
                let written = &block.text[r.clone()];
                let key = keys::typed(&typed, fields, On::Inline, written);
                let start = c.own.start + r.start + (written.len() - written.trim_start().len());
                let end = c.own.start + r.start + written.trim_end().len();
                if !inline_seen.insert((typed.kind.clone(), start)) {
                    continue;
                }
                typed.place.span = source.file_range(start..end);
                let line = lines.of(start);
                raws.push(Raw {
                    line,
                    end_line: lines.of(end.saturating_sub(1)).max(line),
                    section: c.section.clone(),
                    parent: host,
                    block_id: None,
                    data: Data::Typed(Box::new(typed)),
                    key,
                });
            }
        }
    }
    raws
}

/// What the matcher knows about the note at `path` with this text, read as `source`: its
/// frontmatter, where its body starts, and the frontmatter's tags; `links` says whether some
/// type reads its links, which then start with the frontmatter's.
fn note_of<'a>(
    path: &'a str,
    text: &'a str,
    source: &'a Source<'a>,
    links: bool,
) -> matcher::Note<'a> {
    let frontmatter = frontmatter::read(text);
    let src: &str = &source.text;
    let body_at = match &frontmatter {
        Some(fm) => src
            .match_indices('\n')
            .nth(fm.end_line - 1)
            .map_or(src.len(), |(i, _)| i + 1),
        None => 0,
    };
    let (tags, end_line): (Vec<String>, usize) = match &frontmatter {
        Some(fm) => (
            ["tags", "tag"]
                .iter()
                .flat_map(|k| fm.values(k))
                .map(|w| w.text.trim_start_matches('#').to_string())
                .collect(),
            fm.end_line,
        ),
        None => (Vec::new(), 0),
    };
    let mut note = matcher::Note::new(path, source, frontmatter, body_at);
    for t in tags {
        note.add_tag(&t);
    }
    if links {
        for line in text.lines().take(end_line) {
            note.links.extend(crate::owner::markdown::line_links(line));
        }
    }
    note
}
