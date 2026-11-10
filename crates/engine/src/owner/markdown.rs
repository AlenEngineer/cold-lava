//! Markdown, the engine's own format and the first owner. Its three jobs, each on a note's text:
//! `read` (links, tags and aliases, when the note is scanned) with `items` (on first use),
//! `edit` (one item → the note's new text) and `relink` (links to moved files → new text).
//! `tags_in` says where each tag is written, for `read` and for recipes that change tags.

use crate::edit::{self, SetField};
use crate::frontmatter::{self, Value};
use crate::gateway::{Edit, Error, Op, Reason};
use crate::items::{self, Item, Settings};
use crate::rename::Change;
use crate::tasks;
use crate::workspace::{Ref, attachment_kind, clean_target};
use chrono::NaiveDate;
use regex::Regex;
use std::ops::Range;
use std::sync::OnceLock;

/// What reading a note gives besides its items, which are read on first use (`items`).
pub struct Read {
    /// Its links, with where they are.
    pub refs: Vec<Ref>,
    pub tags: Vec<String>,
    /// Frontmatter `aliases:` (other names the quick switcher finds the note by).
    pub aliases: Vec<String>,
}

/// Read a note's links, tags and aliases.
pub fn read(text: &str) -> Read {
    let (refs, tags, aliases) = parse(text, parsers());
    Read {
        refs,
        tags,
        aliases,
    }
}

/// The items of the note at workspace-relative `note` (with `.md`) whose text is `text`.
pub fn items(note: &str, text: &str, s: &Settings) -> Vec<Item> {
    items::extract(note, text, s)
}

/// What an edit makes of a note: its new text, which the engine writes.
pub struct Made {
    pub text: String,
    /// False when the note already said this.
    pub changed: bool,
    /// What it did, for the status line: "set due", "ticked".
    pub what: String,
    pub after: After,
    pub warning: Option<String>,
}

/// How the edited item is found again once the note is read again.
pub enum After {
    /// The item as `set-field` read it back.
    Set(Item),
    /// The task's line now, and its next occurrence's.
    Toggled { line: usize, next: Option<usize> },
}

/// Make `edit` on `item` of the note at workspace-relative `note` whose text is `text`: through
/// `edit::plan` for `set-field`, `tasks::toggle_text` for `toggle`. Nothing is written.
pub fn edit(
    note: &str,
    text: &str,
    s: &Settings,
    item: &Item,
    req: &Edit,
    today: NaiveDate,
) -> Result<Made, Error> {
    match &req.op {
        Op::SetField { field, value } => {
            let set = SetField {
                id: &req.id,
                rev: Some(&req.rev),
                field,
                value: value.clone(),
            };
            let set = edit::plan(note, text, s, &set)?;
            let removed = value.is_null() || value.as_str().is_some_and(|v| v.trim().is_empty());
            let what = match removed {
                true => format!("removed {field}"),
                false => format!("set {field}"),
            };
            Ok(Made {
                text: set.text,
                changed: set.changed,
                what,
                after: After::Set(set.item),
                warning: None,
            })
        }
        Op::Toggle => {
            if !item.data.answers_to("task") {
                let message = format!("{} is a {}, not a task", req.id, item.data.kind());
                return Err(Error::new(Reason::Refused, message));
            }
            let t = tasks::toggle_text(note, note, text, s, item.line, Some(&req.rev), today)
                .map_err(|e| {
                    let reason = match e.downcast_ref::<tasks::Changed>() {
                        Some(_) => Reason::Changed,
                        None => Reason::Refused,
                    };
                    Error::new(reason, format!("{e:#}"))
                })?;
            Ok(Made {
                text: t.text,
                changed: true,
                what: "ticked".into(),
                after: After::Toggled {
                    line: t.line,
                    next: t.next.map(|n| n.line),
                },
                warning: t.warning,
            })
        }
    }
}

/// Rewrite the links in a note's text (or a canvas text card's): `link` says what each target,
/// as written, becomes (`md`: in a Markdown link), or `None` to leave it. Returns the new text,
/// the changed lines and how many links changed.
pub(crate) fn relink(
    text: &str,
    link: impl Fn(&str, bool) -> Option<String>,
) -> (String, Vec<Change>, usize) {
    let p = patterns();
    let mut out = String::with_capacity(text.len());
    let mut changes = Vec::new();
    let mut count = 0;
    let mut in_fence = false;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        let t = body.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        }
        if in_fence || t.starts_with("```") || t.starts_with("~~~") {
            out.push_str(line);
            continue;
        }
        let (new_body, n) = relink_line(body, &link, p);
        if n > 0 {
            count += n;
            changes.push(Change {
                line: i + 1,
                before: body.to_string(),
                after: new_body.clone(),
            });
        }
        out.push_str(&new_body);
        out.push_str(ending);
    }
    (out, changes, count)
}

fn relink_line(
    body: &str,
    link: &impl Fn(&str, bool) -> Option<String>,
    p: &Patterns,
) -> (String, usize) {
    // Collect replacements (byte range of the target, new text), then apply from the end.
    let mut reps: Vec<(usize, usize, String)> = Vec::new();
    for (r, md) in line_targets(body, p) {
        if let Some(new) = link(&body[r.clone()], md) {
            reps.push((r.start, r.end, new));
        }
    }
    reps.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut s = body.to_string();
    for (a, b, new) in &reps {
        s.replace_range(*a..*b, new);
    }
    (s, reps.len())
}

struct Patterns {
    wikilink: Regex,
    mdlink: Regex,
    inline_code: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // [[target#heading|alias]] — target is group 1.
        wikilink: Regex::new(r"\[\[([^\]\|#\^]*)((?:[#\^][^\]\|]*)?)((?:\|[^\]]*)?)\]\]").unwrap(),
        // [text](path#heading) — path is group 1; kept if it names a note or an attachment.
        mdlink: Regex::new(r"\]\(([^)\s#]+)((?:#[^)]*)?)\)").unwrap(),
        inline_code: Regex::new(r"`[^`\n]*`").unwrap(),
    })
}

/// The links on one line outside inline code, as (byte range of the target, Markdown link?):
/// wikilinks, then Markdown links naming a note or an attachment.
fn line_targets(body: &str, p: &Patterns) -> Vec<(Range<usize>, bool)> {
    let code: Vec<(usize, usize)> = p
        .inline_code
        .find_iter(body)
        .map(|m| (m.start(), m.end()))
        .collect();
    let in_code = |s: usize| code.iter().any(|&(a, b)| s >= a && s < b);
    let wiki = (p.wikilink.captures_iter(body))
        .map(|c| c.get(1).unwrap())
        .filter(|m| !m.as_str().trim().is_empty())
        .map(|m| (m.range(), false));
    let md = (p.mdlink.captures_iter(body))
        .map(|c| c.get(1).unwrap())
        .filter(|m| {
            let t = m.as_str();
            (t.ends_with(".md") || attachment_kind(t).is_some()) && !t.contains("://")
        })
        .map(|m| (m.range(), true));
    wiki.chain(md).filter(|(r, _)| !in_code(r.start)).collect()
}

/// Every link target in a note's (or a canvas text card's) text outside code, as written.
pub(crate) fn link_targets(text: &str) -> Vec<&str> {
    let p = patterns();
    let mut in_fence = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            out.extend(line_targets(line, p).into_iter().map(|(r, _)| &line[r]));
        }
    }
    out
}

struct Parsers {
    wikilink: Regex,
    mdlink: Regex,
    tag: Regex,
    inline_code: Regex,
}

impl Parsers {
    fn new() -> Self {
        Self {
            // [[target]], [[target#heading]], [[target|alias]], ![[embed]]
            wikilink: Regex::new(r"\[\[([^\]\|#\^]*)[^\]]*\]\]").unwrap(),
            // [text](some/note.md) — local markdown links only
            mdlink: Regex::new(r"\]\(([^)\s]+?\.md)(?:#[^)]*)?\)").unwrap(),
            tag: Regex::new(crate::markdown::TAG).unwrap(),
            inline_code: Regex::new(r"`[^`\n]*`").unwrap(),
        }
    }
}

fn parsers() -> &'static Parsers {
    static P: OnceLock<Parsers> = OnceLock::new();
    P.get_or_init(Parsers::new)
}

fn parse(text: &str, p: &Parsers) -> (Vec<Ref>, Vec<String>, Vec<String>) {
    let mut refs = Vec::new();
    let mut tags = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let add = |refs: &mut Vec<Ref>, target: Option<String>, i: usize, line: &str| {
        if let Some(target) = target {
            let mut context: String = line.trim().chars().take(240).collect();
            if line.trim().chars().count() > 240 {
                context.push('…');
            }
            refs.push(Ref {
                target,
                line: i + 1,
                context,
            });
        }
    };

    for t in tags_in(text) {
        push_tag(&mut tags, &text[t.name]);
    }

    // YAML frontmatter: aliases, and links in properties.
    if lines.first().map(|l| l.trim_end()) == Some("---") {
        i = 1;
        let mut in_aliases = false;
        while i < lines.len() {
            let line = lines[i];
            let t = line.trim();
            if t == "---" || t == "..." {
                i += 1;
                break;
            }
            if let Some(rest) = t
                .strip_prefix("aliases:")
                .or_else(|| t.strip_prefix("alias:"))
            {
                in_aliases = true;
                let rest = rest.trim().trim_start_matches('[').trim_end_matches(']');
                aliases.extend(rest.split(',').filter_map(clean_alias));
            } else if in_aliases && t.starts_with("- ") {
                aliases.extend(clean_alias(&t[2..]));
            } else if !t.is_empty() {
                in_aliases = false;
            }
            // Wikilinks inside frontmatter properties count too, like in Obsidian.
            for c in p.wikilink.captures_iter(line) {
                add(&mut refs, clean_target(&c[1]), i, line);
            }
            i += 1;
        }
    }

    let mut in_fence = false;
    for (n, raw) in lines.iter().enumerate().skip(i) {
        let t = raw.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for target in line_links(raw) {
            add(&mut refs, Some(target), n, raw);
        }
    }
    (refs, tags, aliases)
}

/// A tag as written in a note: the bytes of its name (no `#`), and for one in the frontmatter's
/// `tags`, the bytes that take it out of its list (`None` in the body, or when it is the list's
/// only one).
#[derive(Clone, Debug, PartialEq)]
pub struct TagSpan {
    pub name: Range<usize>,
    pub place: TagPlace,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TagPlace {
    /// `#tag` in the body.
    Body,
    /// A value of the frontmatter's `tags`.
    Frontmatter {
        /// The bytes that take it out of the list; `None` when it is the only one.
        entry: Option<Range<usize>>,
        /// Written in quotes, so any name fits; a bare one YAML would misread needs them.
        quoted: bool,
    },
}

/// Every tag in a note's text, in order: the frontmatter's `tags` (or `tag`) in each form
/// Obsidian reads (`- item` lines, `[a, b]`, or one string, comma-separated; each with or
/// without `#`), then `#tags` in the body outside code blocks and inline code. A nested tag is
/// one span (`area/work`). The tags `read` gives a note are exactly these.
pub fn tags_in(text: &str) -> Vec<TagSpan> {
    let mut out = Vec::new();
    let mut body_from = 0;
    if let Some(fm) = frontmatter::read(text) {
        body_from = (text.split_inclusive('\n').take(fm.end_line))
            .map(str::len)
            .sum();
        let key = ["tags", "tag"].into_iter().find(|k| fm.get(k).is_some());
        match key.and_then(|k| fm.get(k)) {
            Some(Value::List(items)) => {
                let block = items.first().is_some_and(|w| {
                    let (start, _) = line_around(text, w.span.start);
                    text[start..w.span.start].trim_start().starts_with('-')
                });
                for (k, w) in items.iter().enumerate() {
                    let entry = match block {
                        true => Some(line_range(text, w.span.start)),
                        false => list_entry(items, k),
                    };
                    out.extend(frontmatter_tag(text, w, entry));
                }
            }
            Some(Value::Scalar(w)) => {
                let mut parts = Vec::new();
                let mut at = w.span.start;
                for part in w.text.split(',') {
                    let start = at + part.len() - part.trim_start().len();
                    parts.push(frontmatter::Written {
                        text: part.trim(),
                        span: start..start + part.trim().len(),
                    });
                    at += part.len() + 1;
                }
                parts.retain(|w| !w.text.is_empty());
                for (k, w) in parts.iter().enumerate() {
                    out.extend(frontmatter_tag(text, w, list_entry(&parts, k)));
                }
            }
            _ => {}
        }
    }
    let p = parsers();
    let mut in_fence = false;
    let mut at = body_from;
    for line in text[body_from..].split_inclusive('\n') {
        let start = at;
        at += line.len();
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !line.contains('#') {
            continue;
        }
        let code: Vec<Range<usize>> = (p.inline_code.find_iter(line)).map(|m| m.range()).collect();
        for c in p.tag.captures_iter(line) {
            let name = c.get(1).expect("the pattern has a name");
            let hash = name.start() - 1;
            if code.iter().any(|r| r.contains(&hash)) || is_number(name.as_str()) {
                continue;
            }
            out.push(TagSpan {
                name: start + name.start()..start + name.end(),
                place: TagPlace::Body,
            });
        }
    }
    out
}

/// One value of the frontmatter's `tags`, its `#` left out of its name.
fn frontmatter_tag(
    text: &str,
    w: &frontmatter::Written,
    entry: Option<Range<usize>>,
) -> Option<TagSpan> {
    let hash = w.text.len() - w.text.trim_start_matches('#').len();
    let name = w.span.start + hash..w.span.end;
    if name.is_empty() || is_number(&text[name.clone()]) {
        return None;
    }
    let quoted = w.span.start > 0 && matches!(text.as_bytes()[w.span.start - 1], b'"' | b'\'');
    Some(TagSpan {
        name,
        place: TagPlace::Frontmatter { entry, quoted },
    })
}

/// The bytes that take the `k`th of a comma-separated list out: it and the comma before it, or
/// for the first, it and what separates it from the second. Quotes around a value stay with the
/// neighbour's, which leaves them balanced.
fn list_entry(items: &[frontmatter::Written], k: usize) -> Option<Range<usize>> {
    match k {
        _ if items.len() < 2 => None,
        0 => Some(items[0].span.start..items[1].span.start),
        k => Some(items[k - 1].span.end..items[k].span.end),
    }
}

/// The line holding byte `at`, with its line ending.
fn line_range(text: &str, at: usize) -> Range<usize> {
    let (start, end) = line_around(text, at);
    start..end
}

fn line_around(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    (start, end)
}

/// `#123` is not a tag: a tag needs at least one character that isn't a digit.
fn is_number(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_digit())
}

/// The link targets on one line outside inline code: wikilinks, then Markdown links.
pub fn line_links(line: &str) -> Vec<String> {
    // Without code to take out, a link needs `[[` or `](`; most lines have neither.
    if !line.contains('`') && !line.contains("[[") && !line.contains("](") {
        return Vec::new();
    }
    let p = parsers();
    let line = p.inline_code.replace_all(line, "");
    let wiki = p.wikilink.captures_iter(&line);
    let md = p.mdlink.captures_iter(&line);
    wiki.chain(md).filter_map(|c| clean_target(&c[1])).collect()
}

fn clean_alias(raw: &str) -> Option<String> {
    let a = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    (!a.is_empty()).then(|| a.to_string())
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let t = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim_start_matches('#');
    // A tag needs at least one non-digit character (`#123` is not a tag).
    if !t.is_empty() && !t.chars().all(|c| c.is_ascii_digit()) {
        let t = t.to_lowercase();
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links_and_tags(text: &str) -> (Vec<String>, Vec<String>) {
        let (refs, tags, _) = parse(text, parsers());
        (refs.into_iter().map(|r| r.target).collect(), tags)
    }

    #[test]
    fn link_positions() {
        let (refs, _, _) = parse(
            "---\nup: \"[[Parent]]\"\n---\n\nSee [[A]] and\n[[B|bee]] here",
            &Parsers::new(),
        );
        let pos: Vec<(&str, usize, &str)> = refs
            .iter()
            .map(|r| (r.target.as_str(), r.line, r.context.as_str()))
            .collect();
        assert_eq!(
            pos,
            [
                ("Parent", 2, "up: \"[[Parent]]\""),
                ("A", 5, "See [[A]] and"),
                ("B", 6, "[[B|bee]] here")
            ]
        );
    }

    #[test]
    fn wikilink_forms() {
        let (links, _) = links_and_tags(
            "[[Plain]] [[With Alias|shown]] [[Heading#Section]] ![[Embedded]] [[Block^abc]] [[folder/Deep]]",
        );
        assert_eq!(
            links,
            [
                "Plain",
                "With Alias",
                "Heading",
                "Embedded",
                "Block",
                "folder/Deep"
            ]
        );
    }

    #[test]
    fn markdown_links_and_attachments() {
        let (links, _) = links_and_tags(
            "[a](Other%20Note.md) [b](https://example.com/x.md) ![[photo.png]] [[file.pdf]] [[v1.2 notes]] [c](K%C4%ABlauea.md)",
        );
        // Wikilinks are collected before markdown links on each line.
        assert_eq!(links, ["v1.2 notes", "Other Note", "Kīlauea"]);
    }

    #[test]
    fn code_is_ignored() {
        let (links, tags) = links_and_tags("```\n[[InFence]] #fenced\n```\n`[[Inline]]` [[Real]]");
        assert_eq!(links, ["Real"]);
        assert!(tags.is_empty());
    }

    #[test]
    fn tags_inline_and_frontmatter() {
        let text = "---\ntags: [Project, area/work]\naliases:\n  - x\n---\n# Heading\nText #Idea and #123 and #nested/tag.";
        let (_, tags) = links_and_tags(text);
        assert_eq!(tags, ["project", "area/work", "idea", "nested/tag"]);
    }

    #[test]
    fn frontmatter_aliases() {
        let (_, _, a) = parse(
            "---\naliases: [Big Island, \"Hawaiʻi\"]\ntags: x\n---\n",
            &Parsers::new(),
        );
        assert_eq!(a, ["Big Island", "Hawaiʻi"]);
        let (_, t, a) = parse(
            "---\naliases:\n  - One\n  - 'Two'\ntags:\n  - t\n---\n",
            &Parsers::new(),
        );
        assert_eq!(
            (a, t),
            (
                vec!["One".to_string(), "Two".to_string()],
                vec!["t".to_string()]
            )
        );
    }

    #[test]
    fn frontmatter_list_tags() {
        let (_, tags) = links_and_tags("---\ntags:\n  - one\n  - \"two\"\ntitle: x\n---\n");
        assert_eq!(tags, ["one", "two"]);
    }
}
