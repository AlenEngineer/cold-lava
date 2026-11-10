//! Items as pulldown-cmark read them before the syntax tree. The pass stays here to show the
//! tree's walk reads every note of the corpus as it did.

use super::*;
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

/// The items of the note at `note` whose text is `text`, read by pulldown-cmark.
pub(super) fn extract(note: &str, text: &str, s: &Settings) -> Vec<Item> {
    if s.templates.is_template(note) {
        return Vec::new();
    }
    let source = s.flavour.source(text);
    let lines = Lines::new(&source.text);
    let pass = pass(&lines, &s.flavour, Wants::of(&s.matcher));
    finish(note, text, s, &source, &lines, pass)
}

/// A container pulldown-cmark has opened and not yet closed.
struct Open {
    start: usize,
    line: usize,
    /// The `Raw` this container is, for a list item.
    item: Option<usize>,
    /// For a list item: the last line before its first nested list.
    own_end: Option<usize>,
    /// The first line of the last child block that ended.
    prev_child: Option<usize>,
}

fn pass(lines: &Lines, flavour: &Flavour, wants: Wants) -> Pass {
    let src = lines.src;
    let line_of = |offset: usize| lines.of(offset);
    let src_line = |n: usize| lines.line(n);
    let is_blank = |n: usize| lines.blank(n);
    let line_end = |n: usize| lines.end(n);

    let typing = wants.typing;
    let (want_rows, want_paragraphs) = (wants.rows, wants.paragraphs);
    let (want_lists, want_headings) = (wants.lists, wants.headings);
    let want_inline = wants.inline;
    let collect_rows = wants.collect_rows();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut code: Vec<Range<usize>> = Vec::new();
    let mut tables: Vec<Vec<String>> = Vec::new();
    let mut cells: Vec<Range<usize>> = Vec::new();
    let mut in_head = false;
    let mut quotes = 0usize;
    // Inside a code block or the frontmatter, text holds no tags.
    let mut verbatim = 0usize;
    let mut body_tags: Vec<String> = Vec::new();
    let mut links: Vec<String> = Vec::new();

    let mut raws: Vec<Raw> = Vec::new();
    let mut headings: Vec<(u8, String)> = Vec::new();
    let names = |h: &[(u8, String)]| h.iter().map(|h| h.1.clone()).collect::<Vec<_>>();
    let mut open = vec![Open {
        start: 0,
        line: 1,
        item: None,
        own_end: None,
        prev_child: None,
    }];
    let enclosing = |open: &[Open]| open.iter().rev().find_map(|o| o.item);

    for (event, range) in Parser::new_ext(src, flavour.options()).into_offset_iter() {
        match event {
            Event::Text(t) if wants.tags && verbatim == 0 && t.contains('#') => {
                body_tags.extend(matcher::Note::tags_in(&t).map(|(tag, _)| tag.to_string()));
            }
            Event::Code(_) | Event::InlineMath(_) | Event::DisplayMath(_) if typing => {
                code.push(range)
            }
            Event::Start(tag) => {
                let line = line_of(range.start);
                let mut item = None;
                match tag {
                    Tag::Heading { level, .. } => {
                        let level = level as u8;
                        while headings.last().is_some_and(|h| h.0 >= level) {
                            headings.pop();
                        }
                        let text = heading_text(&src[range.clone()]);
                        let end_line = line_of(range.end.saturating_sub(1)).max(line);
                        if want_headings || want_inline {
                            let words = heading_range(src, range.clone());
                            candidates.push(Candidate {
                                on: On::Heading,
                                typeable: want_headings,
                                own: words.clone(),
                                text: words,
                                raw: Some(raws.len()),
                                line,
                                end_line,
                                section: names(&headings),
                                parent: enclosing(&open),
                                block_id: None,
                                in_quote: quotes > 0,
                                table: 0,
                                cells: Vec::new(),
                            });
                        }
                        raws.push(Raw {
                            line,
                            end_line,
                            section: names(&headings),
                            parent: enclosing(&open),
                            block_id: None,
                            data: Data::Heading {
                                level,
                                text: text.clone(),
                            },
                            key: String::new(),
                        });
                        headings.push((level, text));
                    }
                    Tag::Item => {
                        item = Some(raws.len());
                        raws.push(Raw {
                            line,
                            end_line: line,
                            section: names(&headings),
                            parent: enclosing(&open),
                            block_id: None,
                            data: Data::Block,
                            key: String::new(),
                        });
                    }
                    Tag::List(_) => {
                        if let Some(top) = open.last_mut()
                            && top.item.is_some()
                            && top.own_end.is_none()
                        {
                            top.own_end = Some(line.saturating_sub(1).max(top.line));
                        }
                    }
                    Tag::BlockQuote(_) => quotes += 1,
                    Tag::CodeBlock(_) => {
                        verbatim += 1;
                        if typing {
                            code.push(range.clone());
                        }
                    }
                    Tag::MetadataBlock(_) => verbatim += 1,
                    Tag::Link {
                        link_type,
                        ref dest_url,
                        ..
                    } if wants.links => {
                        let local = matches!(link_type, LinkType::WikiLink { .. })
                            || dest_url
                                .split('#')
                                .next()
                                .is_some_and(|d| d.ends_with(".md"));
                        if local {
                            let target = dest_url.split('#').next().unwrap_or_default();
                            links.extend(crate::workspace::clean_target(target));
                        }
                    }
                    Tag::Table(_) if collect_rows => tables.push(Vec::new()),
                    Tag::TableHead => in_head = true,
                    Tag::TableCell if collect_rows => {
                        let cell = trim_cell(src, range.clone());
                        match tables.last_mut() {
                            Some(header) if in_head => header.push(src[cell].to_string()),
                            _ => cells.push(cell),
                        }
                    }
                    _ => {}
                }
                open.push(Open {
                    start: range.start,
                    line,
                    item,
                    own_end: None,
                    prev_child: None,
                });
            }
            Event::End(end) => {
                let Some(frame) = open.pop() else { break };
                let Some(parent) = open.last() else { break };
                let (parent_item, parent_prev) = (parent.item, parent.prev_child);
                let mut last = line_of(range.end.saturating_sub(1)).max(frame.line);
                match (end, frame.item) {
                    (TagEnd::Item, Some(at)) => {
                        last = frame.own_end.unwrap_or(last);
                        while last > frame.line && is_blank(last) {
                            last -= 1;
                        }
                        let first = &src[frame.start..];
                        let first = first.lines().next().unwrap_or_default();
                        let (first, block_id) = if flavour.has(Syntax::BlockIds) {
                            let own_last = if last == frame.line {
                                first
                            } else {
                                src_line(last)
                            };
                            match markdown::block_id_pattern().captures(own_last) {
                                Some(c) if last == frame.line => {
                                    (&first[..c.get(0).unwrap().start()], Some(c[1].to_string()))
                                }
                                Some(c) => (first, Some(c[1].to_string())),
                                None => (first, None),
                            }
                        } else {
                            (first, None)
                        };
                        if want_lists || want_inline {
                            let start = frame.start + list_text_start(first);
                            let end = frame.start + first.trim_end().len();
                            let own_end = start + src[start..line_end(last)].trim_end().len();
                            candidates.push(Candidate {
                                on: On::ListItem,
                                typeable: want_lists,
                                own: start..own_end.max(start),
                                text: start..end.max(start),
                                raw: Some(at),
                                line: frame.line,
                                end_line: last,
                                section: raws[at].section.clone(),
                                parent: None,
                                block_id: None,
                                in_quote: quotes > 0,
                                table: 0,
                                cells: Vec::new(),
                            });
                        }
                        let raw = &mut raws[at];
                        raw.end_line = last;
                        raw.block_id = block_id;
                        raw.data = Data::List {
                            text: list_text(first),
                        };
                    }
                    (TagEnd::Paragraph, _) if parent_item.is_none() => {
                        while last > frame.line && is_blank(last) {
                            last -= 1;
                        }
                        let id = flavour
                            .has(Syntax::BlockIds)
                            .then(|| markdown::block_id_pattern().captures(src_line(last)))
                            .flatten();
                        let alone = id
                            .as_ref()
                            .is_some_and(|c| last == frame.line && c.get(0).unwrap().start() == 0);
                        let mut raw = None;
                        if let Some(c) = &id {
                            let line = match parent_prev {
                                Some(prev) if alone => prev,
                                _ => frame.line,
                            };
                            raw = Some(raws.len());
                            raws.push(Raw {
                                line,
                                end_line: last,
                                section: names(&headings),
                                parent: enclosing(&open),
                                block_id: Some(c[1].to_string()),
                                data: Data::Block,
                                key: String::new(),
                            });
                        }
                        if (want_paragraphs || want_inline) && !alone {
                            let mut text = src[frame.start..range.end].trim_end();
                            if let Some(c) = markdown::block_id_pattern().captures(text)
                                && id.is_some()
                            {
                                text = text[..c.get(0).unwrap().start()].trim_end();
                            }
                            let text = frame.start..frame.start + text.len();
                            candidates.push(Candidate {
                                on: On::Paragraph,
                                typeable: want_paragraphs,
                                own: text.clone(),
                                text,
                                raw,
                                line: frame.line,
                                end_line: last,
                                section: names(&headings),
                                parent: enclosing(&open),
                                block_id: None,
                                in_quote: quotes > 0,
                                table: 0,
                                cells: Vec::new(),
                            });
                        }
                    }
                    (TagEnd::BlockQuote(_), _) => quotes = quotes.saturating_sub(1),
                    (TagEnd::CodeBlock | TagEnd::MetadataBlock(_), _) => {
                        verbatim = verbatim.saturating_sub(1)
                    }
                    (TagEnd::TableHead, _) => in_head = false,
                    (TagEnd::TableRow, _) if collect_rows => {
                        let row = src[frame.start..range.end].trim_end();
                        let text = frame.start..frame.start + row.len();
                        candidates.push(Candidate {
                            on: On::TableRow,
                            typeable: want_rows,
                            own: text.clone(),
                            text,
                            raw: None,
                            line: frame.line,
                            end_line: frame.line,
                            section: names(&headings),
                            parent: enclosing(&open),
                            block_id: None,
                            in_quote: quotes > 0,
                            table: tables.len().saturating_sub(1),
                            cells: std::mem::take(&mut cells),
                        });
                    }
                    _ => {}
                }
                if let Some(parent) = open.last_mut() {
                    parent.prev_child = Some(frame.line);
                }
            }
            _ => {}
        }
    }
    Pass {
        raws,
        candidates,
        code,
        tables,
        tags: body_tags,
        links,
    }
}
