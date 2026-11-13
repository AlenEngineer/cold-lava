//! The workspace's style: how its notes are written (docs/extensions.md, "Workspace style").
//!
//! Each note is counted when it is read (`count`, in the scan): its list markers, link syntax and
//! paths, task fields, headings, frontmatter keys and lists, and the dates its fields hold. The
//! workspace's `Style` is the majority of those counts, then Obsidian's own settings where it has
//! one, then `[style]` in config.toml, then `.cold-lava/style.toml`: each later one wins. A tie, or
//! no notes that say, goes to the first option of each list, which is what Obsidian writes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::frontmatter::{self, Value};
use crate::problem::Problem;

/// The workspace's own overrides, beside its types.
pub const WORKSPACE_FILE: &str = ".cold-lava/style.toml";

macro_rules! options {
    ($(#[$doc:meta])* $name:ident { $($v:ident = $s:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $( #[serde(rename = $s)] $v ),+ }

        impl $name {
            /// Every option, Obsidian's first: a tie goes to the earlier one.
            pub const ALL: &[$name] = &[$($name::$v),+];

            pub fn name(self) -> &'static str {
                match self { $($name::$v => $s),+ }
            }
        }
    };
}

options!(
    /// A list item's marker.
    Marker { Dash = "-", Star = "*", Plus = "+" }
);
options!(
    /// `[[Note]]` or `[Note](Note.md)`.
    LinkSyntax { Wiki = "wikilink", Markdown = "markdown" }
);
options!(
    /// How a link names its target, as Obsidian's `newLinkFormat`: the name alone where it is
    /// unique, `./` or `../` from the linking note, or the path from the workspace's root.
    LinkPath { Shortest = "shortest", Relative = "relative", Absolute = "absolute" }
);
options!(
    /// A task's dates and other fields: `📅 2026-11-12` or `[due:: 2026-11-12]`.
    TaskFields { Emoji = "emoji", Dataview = "dataview" }
);
options!(
    /// `# Title`, or the title underlined with `===` or `---`.
    Headings { Atx = "atx", Setext = "setext" }
);
options!(
    /// A frontmatter list: `key:` then `  - item` lines (as Obsidian writes them), `- item` lines
    /// at the start of the line, or `key: [a, b]`.
    Lists { Indented = "indented", Flush = "flush", Flow = "flow" }
);
options!(
    /// A tag in the frontmatter's `tags`: `lava` or `#lava`.
    Tags { Bare = "bare", Hash = "hash" }
);
options!(
    /// A date's form, as a moment.js format. `DD/MM/YYYY` and `MM/DD/YYYY` tell apart only by a
    /// day past the 12th; a date that could be either counts for both.
    DateForm {
        Iso = "YYYY-MM-DD",
        Slashed = "YYYY/MM/DD",
        DayFirst = "DD/MM/YYYY",
        MonthFirst = "MM/DD/YYYY",
        Dotted = "DD.MM.YYYY",
    }
);

/// What one note does, counted when it is read. Each array counts its options in `ALL` order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub markers: [u32; 3],
    pub links: [u32; 2],
    pub paths: [u32; 3],
    pub task_fields: [u32; 2],
    pub headings: [u32; 2],
    pub lists: [u32; 3],
    pub tags: [u32; 2],
    /// Its frontmatter's top-level keys, in order.
    pub keys: Box<[Box<str>]>,
    /// The fields (frontmatter properties and Dataview fields) holding a date, with its form.
    pub dates: Box<[(Box<str>, DateForm)]>,
}

const TASK_GLYPHS: &[char] = &[
    '📅', '📆', '🗓', '⏳', '⌛', '🛫', '➕', '✅', '❌', '🔁', '⏫', '🔼', '🔽', '🔺', '⏬', '🆔',
    '⛔',
];

/// How a task's text writes its fields: Dataview when it has a `::`, else emoji when it has a
/// Tasks glyph, else it has none to say.
pub fn fields_of(task: &str) -> Option<TaskFields> {
    if task.contains("::") {
        Some(TaskFields::Dataview)
    } else if !task.is_ascii() && task.contains(TASK_GLYPHS) {
        Some(TaskFields::Emoji)
    } else {
        None
    }
}

/// Count what the note `text` does, reading its links for it.
pub fn count(text: &str) -> Counts {
    count_with(text, crate::owner::markdown::read(text).written)
}

/// Count what the note `text` does, with its links as the Markdown owner counted them. One pass
/// over its lines; code blocks are skipped.
pub fn count_with(text: &str, links: Links) -> Counts {
    let mut c = Counts {
        links: links.syntax,
        paths: links.paths,
        ..Counts::default()
    };
    let mut dates: Vec<(Box<str>, DateForm)> = Vec::new();
    let mut body_from = 0;
    if let Some(fm) = frontmatter::read(text) {
        body_from = fm.end_line;
        c.keys = fm.properties.iter().map(|(k, _)| Box::from(*k)).collect();
        for (key, value) in &fm.properties {
            let tags = matches!(*key, "tags" | "tag");
            match value {
                Value::List(items) => {
                    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |i| i + 1);
                    let first = items.first().map(|w| line_start(w.span.start));
                    let form = match first.map(|s| &text[s..]) {
                        None => None,
                        Some(l) if l.starts_with([' ', '\t']) => Some(Lists::Indented),
                        Some(l) if l.starts_with('-') => Some(Lists::Flush),
                        Some(_) => Some(Lists::Flow),
                    };
                    if let Some(form) = form {
                        c.lists[form as usize] += 1;
                    }
                    if tags {
                        for w in items {
                            c.tags[w.text.starts_with('#') as usize] += 1;
                        }
                    }
                }
                Value::Scalar(w) => {
                    if tags {
                        for t in w.text.split([' ', ',']).filter(|t| !t.is_empty()) {
                            c.tags[t.starts_with('#') as usize] += 1;
                        }
                    } else {
                        let v = w.text.trim_matches(['"', '\'']);
                        dates.extend(date_forms(v).map(|f| (Box::from(*key), f)));
                    }
                }
                _ => {}
            }
        }
    }
    let mut fence: Option<&str> = None;
    let mut previous = "";
    for line in text.lines().skip(body_from) {
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
            previous = "";
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
            previous = "";
            continue;
        }
        let t = t.trim_end();
        if let Some(rest) = t.strip_prefix('#') {
            let level = 1 + rest.len() - rest.trim_start_matches('#').len();
            let after = &rest[level - 1..];
            if level <= 6 && (after.is_empty() || after.starts_with([' ', '\t'])) {
                c.headings[Headings::Atx as usize] += 1;
            }
        } else if !t.is_empty()
            && (t.bytes().all(|b| b == b'=') || t.bytes().all(|b| b == b'-'))
            && line.len() - t.len() < 4
            && underlines(previous)
        {
            c.headings[Headings::Setext as usize] += 1;
        }
        if let Some((marker, rest)) = list_item(t) {
            c.markers[marker as usize] += 1;
            if let Some(fields) = task_text(rest).and_then(fields_of) {
                c.task_fields[fields as usize] += 1;
            }
        }
        if t.contains("::") {
            inline_dates(t, &mut dates);
        }
        previous = line;
    }
    c.dates = dates.into();
    c
}

/// How a note's links are written: the Markdown owner counts them as it finds them (its
/// `Read::written`), so the scan looks for links once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Links {
    pub syntax: [u32; 2],
    pub paths: [u32; 3],
}

impl Links {
    /// One link to a note, `md` when a Markdown link, with its target as written.
    pub fn add(&mut self, md: bool, target: &str) {
        self.syntax[md as usize] += 1;
        let target = target.trim();
        let path = if target.starts_with("./") || target.starts_with("../") {
            LinkPath::Relative
        } else if target.trim_start_matches('/').contains('/') {
            LinkPath::Absolute
        } else {
            LinkPath::Shortest
        };
        self.paths[path as usize] += 1;
    }
}

/// Whether a line of `=` or `-` under `previous` makes it a heading: it is text, not a blank,
/// a list item, a quote, a heading or a table.
fn underlines(previous: &str) -> bool {
    let p = previous.trim();
    !p.is_empty()
        && previous.len() - previous.trim_start().len() < 4
        && !p.starts_with(['#', '>', '|'])
        && list_item(p).is_none()
        && !p.bytes().all(|b| b == b'-' || b == b'=')
}

/// A bullet list item's marker and the text after it; `* * *` and `---` are rules.
fn list_item(t: &str) -> Option<(Marker, &str)> {
    let marker = match t.as_bytes().first()? {
        b'-' => Marker::Dash,
        b'*' => Marker::Star,
        b'+' => Marker::Plus,
        _ => return None,
    };
    let rest = &t[1..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let m = t.as_bytes()[0];
    if t.bytes().filter(|&b| b == m).count() >= 3 && t.bytes().all(|b| b == m || b == b' ') {
        return None;
    }
    Some((marker, rest.trim_start()))
}

/// A task's text after its checkbox.
fn task_text(rest: &str) -> Option<&str> {
    let b = rest.as_bytes();
    (b.len() >= 3 && b[0] == b'[' && b[2] == b']').then(|| &rest[3..])
}

/// The Dataview fields on a line that hold a date: `key:: value` at its start, or `[key:: v]`
/// and `(key:: v)` anywhere.
fn inline_dates(t: &str, out: &mut Vec<(Box<str>, DateForm)>) {
    let mut from = 0;
    while let Some(i) = t[from..].find("::") {
        let at = from + i;
        from = at + 2;
        let rest = t[from..].trim_start();
        let Some(value) = rest.get(..10) else {
            continue;
        };
        let after = rest[10..].trim_start();
        let head = &t[..at];
        let open = head.rfind(['[', '(']);
        let closes = match open.map(|o| head.as_bytes()[o]) {
            Some(b'[') => after.starts_with(']'),
            Some(_) => after.starts_with(')'),
            None => after.is_empty(),
        };
        let key = head[open.map_or(0, |o| o + 1)..].trim();
        let named = !key.is_empty()
            && (key.chars()).all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' '));
        if closes && named {
            out.extend(date_forms(value).map(|f| (Box::from(key), f)));
        }
    }
}

/// The forms `v` is a date in: none, one, or both of `DD/MM/YYYY` and `MM/DD/YYYY`.
fn date_forms(v: &str) -> impl Iterator<Item = DateForm> {
    let b = v.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let s = v.get(r)?;
        s.bytes()
            .all(|d| d.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let ok = |d: u32, m: u32| (1..=31).contains(&d) && (1..=12).contains(&m);
    let mut forms = [None, None];
    if b.len() == 10 {
        let (sep4, sep2) = (b[4], b[2]);
        if (sep4 == b'-' || sep4 == b'/') && b[7] == sep4 && digits(0..4).is_some() {
            if let (Some(m), Some(d)) = (digits(5..7), digits(8..10))
                && ok(d, m)
            {
                forms[0] = Some(match sep4 {
                    b'-' => DateForm::Iso,
                    _ => DateForm::Slashed,
                });
            }
        } else if (sep2 == b'/' || sep2 == b'.')
            && b[5] == sep2
            && digits(6..10).is_some()
            && let (Some(a), Some(z)) = (digits(0..2), digits(3..5))
        {
            if sep2 == b'.' {
                forms[0] = ok(a, z).then_some(DateForm::Dotted);
            } else {
                forms[0] = ok(a, z).then_some(DateForm::DayFirst);
                forms[1] = ok(z, a).then_some(DateForm::MonthFirst);
            }
        }
    }
    forms.into_iter().flatten()
}

/// The counts of every note added up.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub notes: u32,
    pub counts: Counts,
    /// Per key: notes with it, and the sum of its places (0 first, 1000 last) in notes with
    /// two keys or more, with how many such notes.
    keys: BTreeMap<String, (u32, u64, u32)>,
    dates: BTreeMap<String, [u32; 5]>,
}

impl Tally {
    pub fn of<'a>(notes: impl IntoIterator<Item = &'a Counts>) -> Tally {
        let mut t = Tally::default();
        for n in notes {
            t.add(n);
        }
        t
    }

    fn add(&mut self, n: &Counts) {
        self.notes += 1;
        let c = &mut self.counts;
        let sum = |a: &mut [u32], b: &[u32]| a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
        sum(&mut c.markers, &n.markers);
        sum(&mut c.links, &n.links);
        sum(&mut c.paths, &n.paths);
        sum(&mut c.task_fields, &n.task_fields);
        sum(&mut c.headings, &n.headings);
        sum(&mut c.lists, &n.lists);
        sum(&mut c.tags, &n.tags);
        let last = n.keys.len().saturating_sub(1) as u64;
        for (i, k) in n.keys.iter().enumerate() {
            let e = self.keys.entry(k.to_string()).or_default();
            e.0 += 1;
            if let Some(place) = (i as u64 * 1000).checked_div(last) {
                e.1 += place;
                e.2 += 1;
            }
        }
        for (k, f) in &n.dates {
            self.dates.entry(k.to_string()).or_default()[*f as usize] += 1;
        }
    }
}

/// Where a choice came from, the later ones winning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// No note says, or they tie: the first option.
    Default,
    /// The notes' majority.
    Detected,
    /// An Obsidian setting, named by its file and key.
    Obsidian(&'static str),
    /// `[style]` in config.toml.
    Config,
    /// `.cold-lava/style.toml`.
    Workspace,
}

impl Serialize for Origin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Origin::Default => f.write_str("default"),
            Origin::Detected => f.write_str("detected"),
            Origin::Obsidian(setting) => write!(f, "obsidian {setting}"),
            Origin::Config => f.write_str("config.toml [style]"),
            Origin::Workspace => f.write_str(WORKSPACE_FILE),
        }
    }
}

/// One choice of the style, where it came from, and the counts behind it.
#[derive(Clone, Debug, Serialize)]
pub struct Choice<T> {
    pub value: T,
    pub from: Origin,
    pub counts: IndexMap<&'static str, u32>,
}

macro_rules! majority {
    ($ty:ident, $counts:expr) => {{
        let counts: &[u32] = &$counts;
        let best = (0..$ty::ALL.len()).fold(0, |b, i| if counts[i] > counts[b] { i } else { b });
        Choice {
            value: $ty::ALL[best],
            from: if counts[best] > 0 && counts.iter().filter(|&&n| n == counts[best]).count() == 1
            {
                Origin::Detected
            } else {
                Origin::Default
            },
            counts: ($ty::ALL.iter().map(|o| (o.name(), counts[*o as usize]))).collect(),
        }
    }};
}

impl<T> Choice<T> {
    fn set(&mut self, value: Option<T>, from: Origin) {
        if let Some(v) = value {
            self.value = v;
            self.from = from;
        }
    }
}

/// The frontmatter's keys in the order notes write them: each key's mean place among its note's
/// keys (notes with one key give none), then the most used, then by name.
#[derive(Clone, Debug, Serialize)]
pub struct KeyOrder {
    pub value: Vec<String>,
    pub from: Origin,
    /// How many notes have each key.
    pub counts: BTreeMap<String, u32>,
}

/// One field's date form.
#[derive(Clone, Debug, Serialize)]
pub struct DateChoice {
    pub value: String,
    pub from: Origin,
    pub counts: IndexMap<&'static str, u32>,
}

/// The workspace's style: how new text is spelled.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Style {
    /// How many notes were counted.
    pub notes: u32,
    pub list_marker: Choice<Marker>,
    pub links: Choice<LinkSyntax>,
    pub link_path: Choice<LinkPath>,
    pub task_fields: Choice<TaskFields>,
    pub headings: Choice<Headings>,
    pub frontmatter_lists: Choice<Lists>,
    pub tags: Choice<Tags>,
    pub key_order: KeyOrder,
    /// Per field (a property or a Dataview field) that holds dates.
    pub dates: BTreeMap<String, DateChoice>,
    /// Mistakes in the override files, which are skipped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Problem>,
}

impl Default for Style {
    /// Obsidian's defaults, with nothing counted and no settings read.
    fn default() -> Style {
        Style::from_tally(&Tally::default())
    }
}

/// Overrides, from `[style]` in config.toml or `.cold-lava/style.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Overrides {
    pub list_marker: Option<Marker>,
    pub links: Option<LinkSyntax>,
    pub link_path: Option<LinkPath>,
    pub task_fields: Option<TaskFields>,
    pub headings: Option<Headings>,
    pub frontmatter_lists: Option<Lists>,
    pub tags: Option<Tags>,
    pub key_order: Option<Vec<String>>,
    /// Field → moment.js date format.
    pub dates: Option<BTreeMap<String, String>>,
}

impl Overrides {
    /// `text` as `.cold-lava/style.toml` has it, or why it doesn't read: where it is in the
    /// file, and on one line the message alone, as `--json` and the gateway have always said
    /// it.
    pub fn parse(text: &str) -> Result<Overrides, Problem> {
        toml::from_str(text).map_err(|e| Problem::toml_file(&e, text).brief())
    }

    /// `[style]` as config.toml has it, or why it doesn't read (as `parse`).
    pub fn from_table(table: toml::Table) -> Result<Overrides, Problem> {
        table.try_into().map_err(|e| Problem::toml(&e).brief())
    }
}

static CONFIGURED: OnceLock<Overrides> = OnceLock::new();

/// `[style]` from config.toml, which the binary reads (the engine never does).
pub fn set_configured(overrides: Overrides) {
    let _ = CONFIGURED.set(overrides);
}

/// A style worked out the first time it is asked for, so a command that writes one note counts
/// the workspace's notes only when it adds text that needs them.
pub struct Lazy<'a> {
    style: OnceLock<Style>,
    make: Box<dyn Fn() -> Style + 'a>,
}

impl<'a> Lazy<'a> {
    pub fn new(make: impl Fn() -> Style + 'a) -> Lazy<'a> {
        Lazy {
            style: OnceLock::new(),
            make: Box::new(make),
        }
    }

    pub fn get(&self) -> &Style {
        self.style.get_or_init(&self.make)
    }
}

impl std::convert::From<Style> for Lazy<'_> {
    fn from(style: Style) -> Self {
        Lazy {
            style: OnceLock::from(style),
            make: Box::new(Style::default),
        }
    }
}

/// Spelling new text: every insertion asks these (docs/extensions.md, "Workspace style, as
/// built"). Text that is there keeps its own spelling; the callers decide that first.
impl Style {
    /// A link to `target` (a note's name or path, or a file's with its extension), with the
    /// target as given.
    pub fn link(&self, target: &str) -> String {
        match self.links.value {
            LinkSyntax::Wiki => format!("[[{target}]]"),
            LinkSyntax::Markdown => {
                let name = target.rsplit('/').next().unwrap_or(target);
                let file = match crate::workspace::attachment_kind(target) {
                    Some(_) => target.to_string(),
                    None => format!("{target}.md"),
                };
                let name = name.strip_suffix(".md").unwrap_or(name);
                format!(
                    "[{name}]({})",
                    crate::workspace::encode_target_like(&file, "")
                )
            }
        }
    }

    /// A tag as a frontmatter value: `lava` or `#lava`.
    pub fn tag(&self, tag: &str) -> String {
        let tag = tag.trim_start_matches('#');
        match self.tags.value {
            Tags::Bare => tag.to_string(),
            Tags::Hash => format!("#{tag}"),
        }
    }

    /// The lines of a new frontmatter list `key` holding `item` (already quoted if YAML needs
    /// it), each ending in `nl`.
    pub fn new_list(&self, key: &str, item: &str, nl: &str) -> String {
        match self.frontmatter_lists.value {
            Lists::Indented => format!("{key}:{nl}  - {item}{nl}"),
            Lists::Flush => format!("{key}:{nl}- {item}{nl}"),
            Lists::Flow => format!("{key}: [{item}]{nl}"),
        }
    }

    /// Of a frontmatter's `keys`, in order, the one a new `key` goes before: the first that the
    /// workspace writes after it. `None` puts it last, as for a key the workspace doesn't order.
    pub fn key_before<'k>(&self, keys: &[&'k str], key: &str) -> Option<&'k str> {
        let order = &self.key_order.value;
        let rank = |k: &str| order.iter().position(|o| o == k);
        let new = rank(key)?;
        keys.iter()
            .copied()
            .find(|k| rank(k).is_some_and(|r| r > new))
    }

    /// The majority of the counts, without settings.
    pub fn from_tally(t: &Tally) -> Style {
        let c = &t.counts;
        let mut placed: Vec<(&String, u64, u32)> = (t.keys.iter())
            .filter(|(_, (_, _, n))| *n > 0)
            .map(|(k, (uses, sum, n))| (k, sum / u64::from(*n), *uses))
            .collect();
        placed.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(a.0.cmp(b.0)));
        let dates = (t.dates.iter())
            .map(|(k, counts)| {
                let c = majority!(DateForm, *counts);
                let choice = DateChoice {
                    value: c.value.name().to_string(),
                    from: c.from,
                    counts: c.counts,
                };
                (k.clone(), choice)
            })
            .collect();
        Style {
            notes: t.notes,
            list_marker: majority!(Marker, c.markers),
            links: majority!(LinkSyntax, c.links),
            link_path: majority!(LinkPath, c.paths),
            task_fields: majority!(TaskFields, c.task_fields),
            headings: majority!(Headings, c.headings),
            frontmatter_lists: majority!(Lists, c.lists),
            tags: majority!(Tags, c.tags),
            key_order: KeyOrder {
                from: if placed.is_empty() {
                    Origin::Default
                } else {
                    Origin::Detected
                },
                value: placed.into_iter().map(|(k, _, _)| k.clone()).collect(),
                counts: t
                    .keys
                    .iter()
                    .map(|(k, (n, _, _))| (k.clone(), *n))
                    .collect(),
            },
            dates,
            errors: Vec::new(),
        }
    }

    /// The style of the workspace at `root` from its notes' counts: their majority, then
    /// Obsidian's settings, then config.toml's `[style]`, then `.cold-lava/style.toml`.
    pub fn of<'a>(root: &Path, notes: impl IntoIterator<Item = &'a Counts>) -> Style {
        let mut s = Style::from_tally(&Tally::of(notes));
        s.obsidian(root);
        if let Some(o) = CONFIGURED.get() {
            s.apply(o, Origin::Config);
        }
        match std::fs::read_to_string(root.join(WORKSPACE_FILE)) {
            Ok(text) => match Overrides::parse(&text) {
                Ok(o) => s.apply(&o, Origin::Workspace),
                Err(e) => s.errors.push(e.at(WORKSPACE_FILE)),
            },
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                s.errors
                    .push(Problem::line(e.to_string()).at(WORKSPACE_FILE));
            }
            Err(_) => {}
        }
        s
    }

    /// The style of the workspace at `root`, reading its notes for it alone: for a command that
    /// writes one note and doesn't scan the workspace.
    pub fn of_root(root: &Path) -> Style {
        let counts: Vec<Counts> = crate::workspace::note_paths(root)
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .map(|text| count(&text))
            .collect();
        Style::of(root, &counts)
    }

    /// Obsidian's settings where the workspace has them: `useMarkdownLinks` and `newLinkFormat`
    /// in `.obsidian/app.json`, the Tasks plugin's `taskFormat`.
    fn obsidian(&mut self, root: &Path) {
        let app: serde_json::Value = std::fs::read_to_string(root.join(".obsidian/app.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let links = app.get("useMarkdownLinks").and_then(|v| v.as_bool());
        let links = links.map(|md| match md {
            true => LinkSyntax::Markdown,
            false => LinkSyntax::Wiki,
        });
        self.links.set(links, Origin::Obsidian("useMarkdownLinks"));
        let path = match app.get("newLinkFormat").and_then(|v| v.as_str()) {
            Some("shortest") => Some(LinkPath::Shortest),
            Some("relative") => Some(LinkPath::Relative),
            Some("absolute") => Some(LinkPath::Absolute),
            _ => None,
        };
        self.link_path.set(path, Origin::Obsidian("newLinkFormat"));
        let tasks = crate::tasks::TasksSettings::load(root);
        let format = tasks.format_set.then_some(match tasks.dataview {
            true => TaskFields::Dataview,
            false => TaskFields::Emoji,
        });
        self.task_fields
            .set(format, Origin::Obsidian("Tasks plugin taskFormat"));
    }

    fn apply(&mut self, o: &Overrides, from: Origin) {
        self.list_marker.set(o.list_marker, from);
        self.links.set(o.links, from);
        self.link_path.set(o.link_path, from);
        self.task_fields.set(o.task_fields, from);
        self.headings.set(o.headings, from);
        self.frontmatter_lists.set(o.frontmatter_lists, from);
        self.tags.set(o.tags, from);
        if let Some(keys) = &o.key_order {
            self.key_order.value = keys.clone();
            self.key_order.from = from;
        }
        for (key, format) in o.dates.iter().flatten() {
            let d = self.dates.entry(key.clone()).or_insert_with(|| DateChoice {
                value: String::new(),
                from,
                counts: IndexMap::new(),
            });
            d.value = format.clone();
            d.from = from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(notes: &[&str]) -> Style {
        let counts: Vec<Counts> = notes.iter().map(|n| count(n)).collect();
        Style::from_tally(&Tally::of(&counts))
    }

    #[test]
    fn counts_what_a_note_does() {
        let note = "---\ntitle: Lava\ntags:\n  - volcano\n  - \"#hot\"\ncreated: 2026-11-08\n---\n\
                    # Lava\n\nIntro\n=====\n\n* one\n* [ ] Pour [due:: 2026-11-12]\n- [ ] Cool 📅 2026-11-13\n\
                    + three\n* * *\n\n```\n- not counted [[Nope]]\n```\n\
                    See [[Basalt]], [Rock](Rocks/Rock.md) and [[./Near]].\nseen:: 12/31/2026\n";
        let c = count(note);
        assert_eq!(c.markers, [1, 2, 1], "- * +, the rule and the fence aside");
        assert_eq!(c.task_fields, [1, 1]);
        assert_eq!(c.headings, [1, 1]);
        assert_eq!(c.links, [2, 1]);
        assert_eq!(c.paths, [1, 1, 1], "shortest, relative, absolute");
        assert_eq!(c.lists, [1, 0, 0]);
        assert_eq!(c.tags, [1, 1]);
        assert_eq!(&*c.keys, ["title".into(), "tags".into(), "created".into()]);
        let dates: Vec<(&str, DateForm)> = c.dates.iter().map(|(k, f)| (&**k, *f)).collect();
        assert_eq!(
            dates,
            [
                ("created", DateForm::Iso),
                ("due", DateForm::Iso),
                ("seen", DateForm::MonthFirst)
            ]
        );
    }

    #[test]
    fn the_majority_wins_and_a_tie_goes_to_obsidians_default() {
        let s = style(&["* a\n* b\n- c\n", "[x](x.md) [[y]]\n"]);
        assert_eq!(
            (s.list_marker.value, s.list_marker.from),
            (Marker::Star, Origin::Detected)
        );
        assert_eq!(
            (s.links.value, s.links.from),
            (LinkSyntax::Wiki, Origin::Default)
        );
        assert_eq!(s.task_fields.from, Origin::Default, "no task has fields");
        assert_eq!(s.list_marker.counts["*"], 2);
    }

    #[test]
    fn keys_go_in_the_order_notes_write_them() {
        let s = style(&[
            "---\ntitle: a\ntags: [x]\ncreated: 2026-01-01\n---\n",
            "---\ntitle: b\ncreated: 2026-01-02\n---\n",
            "---\naliases: [c]\ntitle: c\n---\n",
            "---\nlonely: yes\n---\n",
        ]);
        assert_eq!(s.key_order.value, ["aliases", "title", "tags", "created"]);
        assert_eq!(
            s.key_order.counts["lonely"], 1,
            "counted, though it gives no order"
        );
        assert_eq!(s.dates["created"].value, "YYYY-MM-DD");
    }

    #[test]
    fn day_and_month_tell_apart_past_the_twelfth() {
        let forms = |v: &str| date_forms(v).collect::<Vec<_>>();
        assert_eq!(forms("31/12/2026"), [DateForm::DayFirst]);
        assert_eq!(forms("12/31/2026"), [DateForm::MonthFirst]);
        assert_eq!(
            forms("01/02/2026"),
            [DateForm::DayFirst, DateForm::MonthFirst]
        );
        assert_eq!(forms("08.11.2026"), [DateForm::Dotted]);
        assert_eq!(forms("2026/11/08"), [DateForm::Slashed]);
        assert_eq!(forms("2026-13-08"), []);
        assert_eq!(forms("someday"), []);
    }

    #[test]
    fn settings_then_overrides_win() {
        let dir = std::env::temp_dir().join(format!("cold-lava-style-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian/plugins/obsidian-tasks-plugin")).unwrap();
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        let counts = [count("* [ ] a [due:: 2026-11-12]\n[x](x.md)\n")];
        let s = Style::of(&dir, &counts);
        assert_eq!(s.links.value, LinkSyntax::Markdown);
        assert_eq!(s.task_fields.value, TaskFields::Dataview);
        std::fs::write(
            dir.join(".obsidian/app.json"),
            r#"{"useMarkdownLinks": false, "newLinkFormat": "relative"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r#"{"taskFormat": "tasksPluginEmoji"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(WORKSPACE_FILE),
            "list-marker = \"+\"\nfrontmatter-lists = \"flow\"\n",
        )
        .unwrap();
        let s = Style::of(&dir, &counts);
        assert_eq!(
            (s.links.value, s.links.from),
            (LinkSyntax::Wiki, Origin::Obsidian("useMarkdownLinks"))
        );
        assert_eq!(s.link_path.value, LinkPath::Relative);
        assert_eq!(s.task_fields.value, TaskFields::Emoji);
        assert_eq!(
            (s.list_marker.value, s.list_marker.from),
            (Marker::Plus, Origin::Workspace)
        );
        assert_eq!(s.frontmatter_lists.value, Lists::Flow);
        std::fs::write(dir.join(WORKSPACE_FILE), "list-marker = \"x\"\n").unwrap();
        let s = Style::of(&dir, &counts);
        assert_eq!(s.list_marker.value, Marker::Star, "a mistake is skipped");
        assert_eq!(s.errors.len(), 1, "{:?}", s.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
