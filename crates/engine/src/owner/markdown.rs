//! Markdown, the engine's own format and the first owner. Its three jobs, each on a note's text:
//! `read` (links, tags and aliases, when the note is scanned) with `items` (on first use),
//! `edit` (one item → the note's new text) and `relink` (links to moved files → new text).
//! `tags_in` says where each tag is written, for `read` and for recipes that change tags, and
//! `links` where each link's target is: `read`, `relink`, tidy's `link_targets` and an item's own
//! `links` all take a note's links from it, so they agree on what a link is.

use crate::cst::{self, Kind, Step};
use crate::edit::{self, SetField};
use crate::frontmatter::{self, Value};
use crate::gateway::{Edit, Error, Op, Reason};
use crate::items::{self, Item, Settings};
use crate::rename::Change;
use crate::style::{self, Lazy};
use crate::tasks;
use crate::workspace::{Ref, attachment_kind, clean_target};
use chrono::NaiveDate;
use std::ops::Range;

/// What reading a note gives besides its items, which are read on first use (`items`).
pub struct Read {
    /// Its links, with where they are.
    pub refs: Vec<Ref>,
    pub tags: Vec<String>,
    /// Frontmatter `aliases:` (other names the quick switcher finds the note by).
    pub aliases: Vec<String>,
    /// How its body's links are written, for the workspace's style.
    pub written: style::Links,
}

/// Read a note's links, tags and aliases from its syntax tree.
pub fn read(text: &str) -> Read {
    let (refs, tags, aliases, written) = parse(text);
    Read {
        refs,
        tags,
        aliases,
        written,
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
/// `edit::plan` for `set-field`, `tasks::toggle_text` for `toggle`, adding text in `style`.
/// Nothing is written.
pub fn edit(
    note: &str,
    text: &str,
    s: &Settings,
    style: &Lazy,
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
            let set = edit::plan(note, text, s, style, &set)?;
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
            let line = item.line;
            let t = tasks::toggle_text(note, note, text, s, style, line, Some(&req.rev), today)
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

/// How a link's target is written in a note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum By {
    /// `[[target]]` or `![[target]]`.
    Wiki,
    /// `[text](target)` or `![text](target)`.
    Inline,
    /// `[text][label]`, `[label][]` or `[label]`: the target is its label's definition's.
    Reference,
    /// `[label]: target`.
    Definition,
}

/// A file named in a note: the name or path as written, before any `#heading`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Link<'t> {
    /// Where the target is written; for a link by reference, where its label is.
    pub at: usize,
    pub target: &'t str,
    pub by: By,
}

impl Link<'_> {
    /// Written as a Markdown link, not a wikilink.
    pub(crate) fn md(&self) -> bool {
        self.by != By::Wiki
    }

    /// The note it names, as the index holds a link's target; `None` for an attachment, a web
    /// address, or a Markdown link to anything but a `.md` file.
    pub(crate) fn note(&self) -> Option<String> {
        let note = !self.md() || self.target.ends_with(".md");
        clean_target(self.target).filter(|_| note)
    }

    /// Whether it names a file a rename can move: any wikilink's target, and a Markdown link's
    /// when it is a note or an attachment.
    fn file(&self) -> bool {
        match self.md() {
            true => self.target.ends_with(".md") || attachment_kind(self.target).is_some(),
            false => !self.target.trim().is_empty(),
        }
    }
}

/// The file a Markdown link's destination names, as written before its `#heading`; the
/// destination comes without its title or its `<>`. `None` for a web address.
fn md_path(dest: &str) -> Option<&str> {
    let path = dest.split('#').next().unwrap_or_default();
    (!path.is_empty() && !path.contains("://")).then_some(path)
}

/// A note's links, collected from the steps of a walk through its syntax tree.
#[derive(Default)]
pub(crate) struct Links<'t> {
    found: Vec<Link<'t>>,
    /// Each link by reference: where its label is, and the label.
    labels: Vec<(usize, &'t str)>,
    /// Each definition's destination as written, and the file it names.
    dests: Vec<(&'t str, &'t str)>,
}

impl<'t> Links<'t> {
    pub(crate) fn step(&mut self, step: Step<'t>) {
        let Step::Token(kind, text, at) = step else {
            return;
        };
        let (at, target, by) = match kind {
            Kind::Target => (at, Some(text), By::Wiki),
            Kind::LinkDest => {
                let Some(dest) = cst::link_destination(text) else {
                    return;
                };
                (at + dest.start, md_path(&text[dest]), By::Inline)
            }
            Kind::DefinitionDest => {
                let dest = cst::destination(text);
                let target = md_path(&text[dest.clone()]);
                self.dests
                    .extend(target.map(|target| (&text[dest.clone()], target)));
                (at + dest.start, target, By::Definition)
            }
            Kind::LinkLabel => {
                self.labels.push((at, text));
                return;
            }
            _ => return,
        };
        self.found
            .extend(target.map(|target| Link { at, target, by }));
    }

    /// The links in the order written; `defs`, the note's definitions, say where a link by
    /// reference leads.
    pub(crate) fn finish(mut self, defs: &cst::Definitions) -> Vec<Link<'t>> {
        for &(at, label) in &self.labels {
            let dest = defs.get(label);
            let defined = dest.and_then(|dest| self.dests.iter().find(|d| d.0 == dest));
            self.found.extend(defined.map(|d| Link {
                at,
                target: d.1,
                by: By::Reference,
            }));
        }
        if !self.labels.is_empty() {
            self.found.sort_by_key(|l| l.at);
        }
        self.found
    }
}

/// Every file named in a note's text (or a canvas text card's), in the order written, as the
/// index reads it (`cst::Gates::INDEX`): wikilinks and embeds, the frontmatter's included,
/// Markdown links and images, links by reference, and each definition's own destination.
pub(crate) fn links(text: &str) -> Vec<Link<'_>> {
    // A link is written with `[[` or `](`, a definition with `]:`.
    if !(text.contains("[[") || text.contains("](") || text.contains("]:")) {
        return Vec::new();
    }
    let (note, defs) = cst::blocks(text, cst::Gates::INDEX);
    let mut links = Links::default();
    note.read(&defs, &mut |step| links.step(step));
    links.finish(&defs)
}

/// Rewrite the links in a note's text (or a canvas text card's): `link` says what each target,
/// as written, becomes (`md`: in a Markdown link), or `None` to leave it. A link by reference
/// is rewritten where its target is written, in its definition. Returns the new text, the
/// changed lines and how many links changed. Every byte outside the targets stays.
pub(crate) fn relink(
    text: &str,
    link: impl Fn(&str, bool) -> Option<String>,
) -> (String, Vec<Change>, usize) {
    let written = links(text).into_iter();
    let reps: Vec<(Range<usize>, String)> = written
        .filter(|l| l.by != By::Reference && l.file())
        .filter_map(|l| Some((l.at..l.at + l.target.len(), link(l.target, l.md())?)))
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut changes = Vec::new();
    let (mut at, mut line, mut counted) = (0, 1, 0);
    let mut k = 0;
    while k < reps.len() {
        let first = reps[k].0.start;
        let start = text[..first].rfind('\n').map_or(0, |i| i + 1);
        let end = text[first..].find('\n').map_or(text.len(), |i| first + i);
        let end = start + text[start..end].trim_end_matches('\r').len();
        out.push_str(&text[at..start]);
        let from = out.len();
        at = start;
        while k < reps.len() && reps[k].0.start < end {
            out.push_str(&text[at..reps[k].0.start]);
            out.push_str(&reps[k].1);
            at = reps[k].0.end;
            k += 1;
        }
        out.push_str(&text[at..end]);
        at = end;
        line += newlines(&text.as_bytes()[counted..start]);
        counted = start;
        changes.push(Change {
            line,
            before: text[start..end].to_string(),
            after: out[from..].to_string(),
        });
    }
    out.push_str(&text[at..]);
    (out, changes, reps.len())
}

/// Every link target in a note's (or a canvas text card's) text, as written: what `read`
/// counts as a link, and the attachments linked too.
pub(crate) fn link_targets(text: &str) -> Vec<&str> {
    let written = links(text).into_iter();
    let linked = written.filter(|l| l.by != By::Definition && l.file());
    linked.map(|l| l.target).collect()
}

/// Read the note `text` forwards in the syntax every workspace's links are read in, as its
/// syntax tree would be walked, without making the tree.
fn read_syntax<'t>(text: &'t str, f: &mut impl FnMut(Step<'t>)) {
    cst::read(text, cst::Gates::INDEX, f)
}

fn body_tag(at: usize, tag: &str) -> TagSpan {
    TagSpan {
        name: at + 1..at + tag.len(),
        place: TagPlace::Body,
    }
}

fn newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

fn parse(text: &str) -> (Vec<Ref>, Vec<String>, Vec<String>, style::Links) {
    let mut written = style::Links::default();
    let mut refs = Vec::new();
    let mut tags = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let line_text = |at: usize| {
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        &text[start..end]
    };
    let add = |refs: &mut Vec<Ref>, target: Option<String>, n: usize, at: usize| {
        if let Some(target) = target {
            let line = line_text(at);
            let mut context: String = line.trim().chars().take(240).collect();
            if line.trim().chars().count() > 240 {
                context.push('…');
            }
            refs.push(Ref {
                target,
                line: n + 1,
                context,
            });
        }
    };

    // The note in the syntax every workspace's links are read in, with its link definitions,
    // which say where a reference link points.
    let (note, defs) = cst::blocks(text, cst::Gates::INDEX);
    let mut body_tags = Vec::new();
    let mut frontmatter_end = 0;
    let mut links = Links::default();
    note.read(&defs, &mut |step| {
        match step {
            Step::Leave(Kind::Frontmatter, at) => frontmatter_end = at,
            Step::Token(Kind::Tag, tag, at) => body_tags.push(body_tag(at, tag)),
            _ => {}
        }
        links.step(step);
    });
    // A definition is no link itself: the links that use its label are.
    let mut links = links.finish(&defs);
    links.retain(|l| l.by != By::Definition);
    for t in with_frontmatter_tags(text, body_tags) {
        push_tag(&mut tags, &text[t.name]);
    }

    // YAML frontmatter: aliases, and links in properties.
    if frontmatter_end > 0 {
        let mut in_aliases = false;
        let lines = text[..frontmatter_end].lines();
        for line in lines.skip(1) {
            let t = line.trim();
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
        }
    }
    // Wikilinks inside frontmatter properties count too, like in Obsidian. In the body, each
    // line's wikilinks come before its Markdown links.
    let (mut line, mut counted) = (0, 0);
    let mut numbered: Vec<(usize, Link)> = links
        .into_iter()
        .map(|w| {
            line += newlines(&text.as_bytes()[counted..w.at]);
            counted = w.at;
            (line, w)
        })
        .collect();
    let body = numbered.partition_point(|(_, w)| w.at < frontmatter_end);
    numbered[body..].sort_by_key(|(n, w)| (*n, w.md()));
    for (k, (n, w)) in numbered.into_iter().enumerate() {
        let target = w.note();
        if k >= body && target.is_some() {
            written.add(w.md(), w.target);
        }
        add(&mut refs, target, n, w.at);
    }
    (refs, tags, aliases, written)
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
    let mut body = Vec::new();
    read_syntax(text, &mut |step| {
        if let Step::Token(Kind::Tag, tag, at) = step {
            body.push(body_tag(at, tag));
        }
    });
    with_frontmatter_tags(text, body)
}

/// The frontmatter's tags, then `body`'s.
fn with_frontmatter_tags(text: &str, body: Vec<TagSpan>) -> Vec<TagSpan> {
    let mut out = Vec::new();
    if let Some(fm) = frontmatter::read(text) {
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
    out.extend(body);
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

/// The notes linked in a piece of text read on its own, as a field's value or a frontmatter
/// line is: wikilinks, then Markdown links.
pub fn line_links(text: &str) -> Vec<String> {
    let mut links = links(text);
    links.retain(|l| l.by != By::Definition);
    links.sort_by_key(|l| l.md());
    links.iter().filter_map(|l| l.note()).collect()
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
        let (refs, tags, _, _) = parse(text);
        (refs.into_iter().map(|r| r.target).collect(), tags)
    }

    #[test]
    fn link_positions() {
        let (refs, _, _, _) =
            parse("---\nup: \"[[Parent]]\"\n---\n\nSee [[A]] and\n[[B|bee]] here");
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
        let (_, _, a, _) = parse("---\naliases: [Big Island, \"Hawaiʻi\"]\ntags: x\n---\n");
        assert_eq!(a, ["Big Island", "Hawaiʻi"]);
        let (_, t, a, _) = parse("---\naliases:\n  - One\n  - 'Two'\ntags:\n  - t\n---\n");
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

    #[test]
    fn a_link_to_a_heading_is_not_a_tag() {
        let (_, tags) = links_and_tags("See [the agenda](#agenda), (#real) and #also.");
        assert_eq!(tags, ["real", "also"]);
    }

    #[test]
    fn tags_right_after_emphasis_and_highlight_markers() {
        let (_, tags) = links_and_tags(
            "**#bold** _#slanted_ ==#marked== ~~#struck~~, not a#b, a**#c**, x_#d, http://e.f/#g, [[#H]], [i](#j) or [k](**#l).",
        );
        assert_eq!(tags, ["bold", "slanted", "marked", "struck"]);
    }

    #[test]
    fn reference_links_count_where_they_are_written() {
        let text = "See [the note][n] and [N].\n[web][w], [none] and ![pic][n].\n\n[n]: Some%20Note.md \"Title\"\n[w]: https://example.com/x.md\n[unused]: Unused.md\n";
        let (refs, _, _, _) = parse(text);
        let pos: Vec<(&str, usize)> = refs.iter().map(|r| (r.target.as_str(), r.line)).collect();
        assert_eq!(pos, [("Some Note", 1), ("Some Note", 1), ("Some Note", 2)]);
        assert_eq!(refs[2].context, "[web][w], [none] and ![pic][n].");
    }

    #[test]
    fn a_links_text_may_hold_an_image_and_go_on_to_the_next_line() {
        let (links, _) = links_and_tags(
            "See [text that\ngoes on](B.md), [![alt](x.md)](C.md) and no bracket](D.md).\n\n[Source]: [[Book]]\n",
        );
        assert_eq!(links, ["B", "x", "C", "Book"]);
    }

    /// What a rename of the note `B` to `C`, and of `b.png` to `c.png`, makes of a link.
    fn b_to_c(raw: &str, md: bool) -> Option<String> {
        match (raw, md) {
            ("B", false) => Some("C".into()),
            ("B.md", true) => Some("C.md".into()),
            ("b.png", _) => Some("c.png".into()),
            _ => None,
        }
    }

    #[test]
    fn relink_rewrites_a_definition_where_it_is_written() {
        let text = "See [the note][n], [N] and ![pic][p].\n\n[n]: B.md \"Title\"\n[p]: <b.png> 'Pic'\n  [Unused]:   B.md#Heading\n[web]: https://example.com/B.md\n";
        let (new, changes, links) = relink(text, b_to_c);
        assert_eq!(
            new,
            "See [the note][n], [N] and ![pic][p].\n\n[n]: C.md \"Title\"\n[p]: <c.png> 'Pic'\n  [Unused]:   C.md#Heading\n[web]: https://example.com/B.md\n"
        );
        assert_eq!(links, 3);
        let lines: Vec<(usize, &str, &str)> = changes
            .iter()
            .map(|c| (c.line, c.before.as_str(), c.after.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                (3, "[n]: B.md \"Title\"", "[n]: C.md \"Title\""),
                (4, "[p]: <b.png> 'Pic'", "[p]: <c.png> 'Pic'"),
                (
                    5,
                    "  [Unused]:   B.md#Heading",
                    "  [Unused]:   C.md#Heading"
                ),
            ]
        );
    }

    #[test]
    fn relink_rewrites_every_style_in_one_note_and_keeps_every_other_byte() {
        let text = "---\nup: \"[[B]]\"\n---\r\n# [[B|shown]] and [[B#H]]\r\n  - [ ] ![[B]] [md](B.md#H) ![i](b.png)  \r\n> [a ![[B]] b](B.md)\n\n| [[B]] | [y](B.md) |\n| --- | --- |\n| [[B\\|shown]] | ![[B#H\\|part]] |\n\n[t](B.md \"title\") [t](B.md 'title') [t](B.md#H (title)) ![i](b.png \"pic\")\n[p](<B.md>) [p](<B.md#H> \"title\") ![i](<b.png>)\n\n[r]: B.md\nlast [[B]]";
        let (new, changes, links) = relink(text, b_to_c);
        assert_eq!(new, text.replace('B', "C").replace("b.png", "c.png"));
        assert_eq!(links, 21);
        let lines: Vec<usize> = changes.iter().map(|c| c.line).collect();
        assert_eq!(lines, [2, 4, 5, 6, 8, 10, 12, 13, 15, 16]);
        assert_eq!(
            (changes[2].before.as_str(), changes[2].after.as_str()),
            (
                "  - [ ] ![[B]] [md](B.md#H) ![i](b.png)  ",
                "  - [ ] ![[C]] [md](C.md#H) ![i](c.png)  "
            ),
            "a changed line comes without its line ending"
        );
        // A link's text holds no link: the wikilink inside is the link, and what is around
        // it is text, which a rename leaves alone.
        let (nested, _, links) = relink("[a [[B]] b](B.md)\n", b_to_c);
        assert_eq!((nested.as_str(), links), ("[a [[C]] b](B.md)\n", 1));
    }

    #[test]
    fn relink_leaves_what_the_tree_reads_as_no_link() {
        for text in [
            "    [[B]] in indented code\n",
            "````\n```\n[[B]] after a fence line too short to close its fence\n````\n",
            "> ```\n> [[B]] in a fence in a quote\n> ```\n",
            "<div>\n[[B]] in an HTML block\n</div>\n",
            "<!-- [[B]] -->\n",
            "``[[B]]`` between two backticks\n",
            "`code that\n[[B]] goes on`\n",
            "---\nlink: \"[md](B.md)\"\n---\n",
            "no bracket before](B.md)\n",
            "[a]([[B]])\n",
            "text\n[n]: B.md\n",
        ] {
            let (new, _, links) = relink(text, b_to_c);
            assert_eq!((new.as_str(), links), (text, 0), "{text:?}");
        }
    }

    #[test]
    fn relink_finds_the_links_the_line_reader_missed() {
        for (text, links) in [
            ("```js``` then [[B]]\n", 1),
            (
                "text\n    ```\n[[B]] after an indented line that is no fence\n",
                1,
            ),
            ("See [text that\ngoes on](B.md) here.\n", 1),
            ("[![alt](b.png)](B.md)\n", 2),
            ("[Source]: [[B]]\n", 1),
        ] {
            let (new, _, n) = relink(text, b_to_c);
            assert_eq!(n, links, "{text:?}");
            assert_eq!(new, text.replace('B', "C").replace("b.png", "c.png"));
        }
    }

    /// #206: the pipe before an alias is written `\|` in a table cell, where a bare `|` would
    /// end the cell. Obsidian writes an aliased link in a table so.
    #[test]
    fn relink_rewrites_a_wikilink_with_an_escaped_pipe_in_a_table() {
        let text = "| note | seen |\n| --- | --- |\n| [[B\\|bee]] | ![[B#H\\|part]] |\n";
        assert_eq!(links_and_tags(text).0, ["B", "B"]);
        let (new, _, links) = relink(text, b_to_c);
        assert_eq!(new, text.replace('B', "C"));
        assert_eq!(links, 2);
    }

    /// Outside a table `\|` is not an escaped pipe: the backslash is the target's.
    #[test]
    fn an_escaped_pipe_outside_a_table_stays_in_the_target() {
        let text = "See [[B\\|bee]].\n";
        assert_eq!(links_and_tags(text).0, ["B\\"]);
        let (new, _, links) = relink(text, b_to_c);
        assert_eq!((new.as_str(), links), (text, 0));
    }

    /// #206: a title in each of CommonMark's three spellings, one with parentheses in it.
    #[test]
    fn relink_rewrites_a_link_with_a_title() {
        let text = "[a](B.md \"title\") [b](B.md 'title') [c](B.md (title)) [d](B.md#H \"a (b) c\") ![e](b.png \"pic\")\n";
        assert_eq!(links_and_tags(text).0, ["B", "B", "B", "B"]);
        let (new, _, links) = relink(text, b_to_c);
        assert_eq!(new, text.replace('B', "C").replace("b.png", "c.png"));
        assert_eq!(links, 5);
    }

    /// #206: a destination in `<>`, which may hold spaces and parentheses.
    #[test]
    fn relink_rewrites_a_destination_in_angle_brackets() {
        let text = "[a](<B.md>) [b](<B.md#H> \"title\") ![c](<b.png>) [d](<My Note.md>) [e](<Notes (old)/My Note.md>)\n";
        assert_eq!(
            links_and_tags(text).0,
            ["B", "B", "My Note", "Notes (old)/My Note"]
        );
        let moved = |raw: &str, md: bool| match raw {
            "My Note.md" => Some("Your%20Note.md".to_string()),
            "Notes (old)/My Note.md" => Some("Notes%20(old)/Your%20Note.md".to_string()),
            _ => b_to_c(raw, md),
        };
        let (new, _, links) = relink(text, moved);
        assert_eq!(
            new,
            "[a](<C.md>) [b](<C.md#H> \"title\") ![c](<c.png>) [d](<Your%20Note.md>) [e](<Notes%20(old)/Your%20Note.md>)\n"
        );
        assert_eq!(links, 5);
    }

    /// A percent-encoded destination is read as the file it names, and a rename sees it as
    /// written.
    #[test]
    fn relink_rewrites_a_percent_encoded_destination() {
        let text = "[a](My%20Note.md#A%20Heading) ![p](b%20map.png)\n";
        assert_eq!(links_and_tags(text).0, ["My Note"]);
        let moved = |raw: &str, _: bool| match raw {
            "My%20Note.md" => Some("Your%20Note.md".to_string()),
            "b%20map.png" => Some("c%20map.png".to_string()),
            _ => None,
        };
        let (new, _, links) = relink(text, moved);
        assert_eq!(new, "[a](Your%20Note.md#A%20Heading) ![p](c%20map.png)\n");
        assert_eq!(links, 2);
    }

    /// A destination may hold balanced parentheses, as a file named `Note (2024).md` does when
    /// its link is percent-encoded.
    #[test]
    fn relink_rewrites_a_destination_with_parentheses() {
        let text = "[a](Notes%20(old)/B.md) and [w](https://en.wikipedia.org/wiki/B_(letter)).\n";
        assert_eq!(links_and_tags(text).0, ["Notes (old)/B"]);
        let moved = |raw: &str, _: bool| match raw {
            "Notes%20(old)/B.md" => Some("Notes%20(old)/C.md".to_string()),
            _ => None,
        };
        let (new, _, links) = relink(text, moved);
        assert_eq!(
            new,
            "[a](Notes%20(old)/C.md) and [w](https://en.wikipedia.org/wiki/B_(letter)).\n"
        );
        assert_eq!(links, 1);
    }

    fn sorted(mut targets: Vec<String>) -> Vec<String> {
        targets.sort();
        targets
    }

    /// Backlinks and the graph (`read`), tidy (`link_targets`) and a rename (`relink`) take the
    /// same links from every note of the corpus. A rename considers a link by reference where
    /// its target is written, in its definition, used or not.
    #[test]
    fn read_tidy_and_rename_agree_on_every_link_of_the_corpus() {
        let mut total = 0;
        for (name, text) in corpus_and_forms() {
            let read = sorted(parse(&text).0.into_iter().map(|r| r.target).collect());
            let tidy = link_targets(&text).into_iter().filter_map(clean_target);
            assert_eq!(sorted(tidy.collect()), read, "{name}: tidy and read");

            let considered = std::cell::RefCell::new(Vec::new());
            relink(&text, |raw, _| {
                considered.borrow_mut().extend(clean_target(raw));
                None
            });
            let by = |by: By| -> Vec<String> {
                let links = links(&text).into_iter();
                links
                    .filter(|l| l.by == by)
                    .filter_map(|l| l.note())
                    .collect()
            };
            let (used, defined) = (by(By::Reference), by(By::Definition));
            assert!(
                used.iter().all(|u| defined.contains(u)),
                "{name}: a link by reference whose definition a rename doesn't consider"
            );
            let rename = considered.into_inner().into_iter().chain(used);
            let read_and_defined = read.iter().cloned().chain(defined);
            assert_eq!(
                sorted(rename.collect()),
                sorted(read_and_defined.collect()),
                "{name}: rename and read"
            );
            total += read.len();
        }
        assert!(total > 20, "the corpus has {total} links");
    }

    /// Every step of reading `text`: its kind, and a token's text.
    fn steps(text: &str) -> Vec<(u8, Kind, &str)> {
        let mut out = Vec::new();
        cst::read(text, cst::Gates::INDEX, &mut |step| {
            out.push(match step {
                Step::Enter(kind, _) => (0, kind, ""),
                Step::Leave(kind, _) => (1, kind, ""),
                Step::Token(kind, text, _) => (2, kind, text),
            })
        });
        out
    }

    /// Whether the token `is` is the token `was` with nothing but the file it names changed:
    /// a `#heading`, a definition's title and its `<>` stay.
    fn names_another_file(kind: Kind, was: &str, is: &str) -> bool {
        let heading = |dest: &str| dest.find('#').map(|i| dest[i..].to_string());
        let around = |a: Range<usize>, b: Range<usize>| {
            was[..a.start] == is[..b.start]
                && was[a.end..] == is[b.end..]
                && heading(&was[a]) == heading(&is[b])
        };
        match kind {
            Kind::Target => true,
            Kind::LinkDest => match (cst::link_destination(was), cst::link_destination(is)) {
                (Some(a), Some(b)) => around(a, b),
                _ => false,
            },
            Kind::DefinitionDest => around(cst::destination(was), cst::destination(is)),
            _ => false,
        }
    }

    /// Notes with the forms of #206, read with the corpus: an aliased wikilink in a table, a
    /// title in each spelling, a destination in `<>`, parentheses in a destination, and blanks
    /// around one.
    const FORMS: &[&str] = &[
        "| note | part |\n| --- | --- |\n| [[B\\|bee]] | ![[B#H\\|part]] and [[B\\|b]] |\n\nOutside a table: [[B\\|bee]].\n",
        "[a](B.md \"title\") [b](B.md 'title') [c](B.md (title)) [d](B.md#H \"a (b) c\") ![e](b.png \"pic\")\n",
        "[a](<B.md>) [b](<B.md#H> \"title\") ![c](<b.png>) [d](<My Note.md>) [e](<Notes (old)/My Note.md>)\n\n[r]: <My Note.md> 'title'\n",
        "[a](Notes%20(old)/B.md), [b]( B.md ), [c](B.md and more) and [w](https://en.wikipedia.org/wiki/B_(letter)).\n",
        "A [link whose text\ngoes on](<My Note.md> \"title\") and | [[B\\|x]] | in a paragraph.\n",
    ];

    /// The corpus, and `FORMS`.
    fn corpus_and_forms() -> Vec<(String, String)> {
        let forms = FORMS.iter().map(|f| (format!("{f:?}"), f.to_string()));
        cst::corpus().into_iter().chain(forms).collect()
    }

    /// `new` is `old` with `links` link targets rewritten and nothing else: the two trees have
    /// the same shape, and every token is the same bytes but those targets.
    fn only_targets_differ(name: &str, old: &str, new: &str, links: usize) {
        let (was, is) = (steps(old), steps(new));
        assert_eq!(was.len(), is.len(), "{name}: the tree changed shape");
        let mut changed = 0;
        for (a, b) in was.iter().zip(&is) {
            assert_eq!((a.0, a.1), (b.0, b.1), "{name}: the tree changed shape");
            if a.2 != b.2 {
                assert!(
                    names_another_file(a.1, a.2, b.2),
                    "{name}: {:?} became {:?}, which is more than a link's target",
                    a.2,
                    b.2
                );
                changed += 1;
            }
        }
        assert_eq!(changed, links, "{name}: links rewritten");
    }

    #[test]
    fn a_relink_of_the_corpus_changes_only_the_targets_it_rewrites() {
        let mut total = 0;
        for (name, text) in corpus_and_forms() {
            assert_eq!(relink(&text, |_, _| None).0, text, "{name}: nothing moved");
            let (new, changes, links) = relink(&text, |raw, _| Some(format!("X{raw}")));
            only_targets_differ(&name, &text, &new, links);
            assert_eq!(new.len(), text.len() + links, "{name}");
            let (old_lines, new_lines): (Vec<&str>, Vec<&str>) =
                (text.split('\n').collect(), new.split('\n').collect());
            for c in &changes {
                let line = |lines: &[&str]| lines[c.line - 1].trim_end_matches('\r').to_string();
                assert_eq!(
                    (line(&old_lines), line(&new_lines)),
                    (c.before.clone(), c.after.clone()),
                    "{name}:{}",
                    c.line
                );
            }
            total += links;
        }
        assert!(total > 20, "the corpus has {total} links");
    }

    /// Renaming each note of the example workspace changes, in every note the plan touches,
    /// only the targets of the links it counts.
    #[test]
    fn a_rename_in_the_example_workspace_changes_only_link_targets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let ix = crate::index::Index::build(crate::workspace::scan(&root).unwrap());
        let mut total = 0;
        for note in 0..ix.workspace.notes.len() {
            let plan = crate::rename::plan(&ix, note, "Moved Elsewhere/Renamed Note.md").unwrap();
            for e in plan.edits.iter().filter(|e| e.file.ends_with(".md")) {
                only_targets_differ(&e.file, &e.old_text, &e.new_text, e.links);
                total += e.links;
            }
        }
        assert!(total > 20, "the renames rewrote {total} links");
    }

    /// What reading every note of a workspace costs: `read` (the tree's parse read forwards),
    /// `relink` when nothing moves, and making each note's tree. Run on
    /// `scripts/bench-items.py`'s workspace:
    /// `COLD_LAVA_BENCH_WS=DIR cargo test --release -p cold-lava-engine read_cost -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn read_cost() {
        use std::time::{Duration, Instant};
        let dir = std::env::var("COLD_LAVA_BENCH_WS").expect("COLD_LAVA_BENCH_WS");
        let notes: Vec<String> = walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .collect();
        let bytes: usize = notes.iter().map(String::len).sum();
        let time = |f: &dyn Fn(&str)| {
            let mut runs: Vec<Duration> = (0..9)
                .map(|_| {
                    let t = Instant::now();
                    notes.iter().for_each(|n| f(n));
                    t.elapsed()
                })
                .collect();
            runs.sort();
            (runs[4], runs[0], runs[8])
        };
        let read = time(&|n| {
            std::hint::black_box(super::read(n));
        });
        let relink = time(&|n| {
            std::hint::black_box(super::relink(n, |_, _| None));
        });
        let steps = time(&|n| {
            let mut k = 0usize;
            cst::read(n, cst::Gates::INDEX, &mut |_| k += 1);
            std::hint::black_box(k);
        });
        let blocks = time(&|n| {
            std::hint::black_box(cst::block_events(n, cst::Gates::INDEX));
        });
        let tree = time(&|n| {
            std::hint::black_box(cst::Tree::with(n, cst::Gates::INDEX));
        });
        println!(
            "read_cost: {} notes, {} KiB; median (min-max) of 9:",
            notes.len(),
            bytes / 1024
        );
        for (what, t) in [
            ("read", read),
            ("relink, nothing moving", relink),
            ("the parse's steps alone", steps),
            ("blocks alone", blocks),
            ("making the trees", tree),
        ] {
            println!("  {what}: {:?} ({:?}-{:?})", t.0, t.1, t.2);
        }
    }
}
