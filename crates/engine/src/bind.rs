//! Bindings: a placed shape's inputs bound to data, kept live, and what its regions and text
//! show now (docs/shapes.md, "Bindings, as built"). Data, no UI: a frontend binds a shape, hands
//! it the core's item news and events, and asks it what to draw.
//!
//! An input is bound to a `Source`: a gateway query (followed with `items/subscribe`), one note,
//! labels written out, or text of the drawing's own. What the user binds is saved with the
//! placed shape; what follows from it (the members, the regions' labels, the filled text) is
//! computed and never saved.

use std::collections::HashMap;
use std::sync::Arc;

use cold_lava_canvas as canvas;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::act::{Ask, InputData, Inputs, MemberData, Rebind};
use crate::core::{Core, Event};
use crate::gateway::{Filter, News, Query, Select, SubId};
use crate::shapes::{Loaded, Origin, Shapes, fill};
use crate::shows::{Cond, Member, Shows};

/// Whom a binding's subscriptions belong to: the canvas, not an extension. The frontend hands
/// news for this owner to its bindings (`Binding::heard`).
pub const OWNER: &str = "canvas";

/// What an input is bound to. Saved with the placed shape, as the user chose it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The items of an `items/query`, kept live with `items/subscribe`.
    Query(Box<Bound>),
    /// One note, workspace-relative with `.md`: a card's.
    Note(String),
    /// Labels written out.
    Items(Vec<String>),
    /// Text of the drawing's own: a sticky's.
    Text(String),
}

/// A query an input is bound to, and what it is called.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bound {
    #[serde(flatten)]
    pub select: Select,
    /// Its name on the shape (`${a.title}`); by default the query read out (`volcano #eruption`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A condition on the items' fields, in the language of a region's `where`
    /// (`vei >= 6 and country = Japan`): what the gateway's filter can't say.
    #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl Bound {
    /// The query read out, as its default title: its kind, then each condition.
    pub fn label(&self) -> String {
        let f = &self.select.filter;
        let mut out = vec![self.select.kind.clone()];
        out.extend(
            f.tag
                .as_ref()
                .map(|t| format!("#{}", t.trim_start_matches('#'))),
        );
        out.extend(f.folder.as_ref().map(|d| format!("in {d}")));
        out.extend(f.note.as_ref().map(|n| format!("in {n}")));
        if !f.status.is_empty() {
            out.push(format!("status {}", f.status.join(",")));
        }
        out.extend(f.property.as_ref().map(|p| format!("with {p}")));
        out.extend(f.text.as_ref().map(|t| format!("{t:?}")));
        out.extend(self.condition.as_ref().map(|c| format!("where {c}")));
        out.join(" ")
    }
}

impl Source {
    /// Reads the short form `cold-lava shapes --bind` takes, or JSON (`{"note": "…"}`):
    ///
    /// - `type=volcano tag=eruption folder=Volcanoes title="Erupted" where vei >= 6`: a query
    ///   (`kind` for `type`, `tags` for `tag`; also `note`, `status` (comma-separated),
    ///   `property`, `text`), and after `where` a condition on its items' fields;
    /// - `note:Volcanoes/Krakatoa.md`: one note (`.md` added when it has no extension);
    /// - `items:Mount Fuji, Krakatoa`: labels, comma-separated;
    /// - `text:…`: text.
    pub fn parse(s: &str) -> Result<Source, String> {
        let s = s.trim();
        if s.starts_with('{') {
            return serde_json::from_str(s).map_err(|e| e.to_string());
        }
        if let Some(note) = s.strip_prefix("note:") {
            let note = note.trim();
            let has_extension = (note.rsplit('/').next()).is_some_and(|n| n.contains('.'));
            return Ok(Source::Note(match has_extension {
                true => note.to_string(),
                false => format!("{note}.md"),
            }));
        }
        if let Some(items) = s.strip_prefix("items:") {
            let items = items.split(',').map(str::trim).filter(|i| !i.is_empty());
            return Ok(Source::Items(items.map(String::from).collect()));
        }
        if let Some(text) = s.strip_prefix("text:") {
            return Ok(Source::Text(text.to_string()));
        }
        let mut bound = Bound::default();
        let (s, condition) = match s.strip_prefix("where ") {
            Some(c) => ("", Some(c)),
            None => match s.split_once(" where ") {
                Some((q, c)) => (q, Some(c)),
                None => (s, None),
            },
        };
        if let Some(c) = condition {
            Cond::parse(c).map_err(|e| format!("where {c}: {e}"))?;
            bound.condition = Some(c.trim().to_string());
        }
        for word in words(s)? {
            let Some((key, value)) = word.split_once('=') else {
                return Err(format!(
                    "{word:?}: a query is key=value words, like type=volcano"
                ));
            };
            let value = value.to_string();
            let f: &mut Filter = &mut bound.select.filter;
            match key {
                "type" | "kind" => bound.select.kind = value,
                "tag" | "tags" => f.tag = Some(value),
                "folder" => f.folder = Some(value),
                "note" => f.note = Some(value),
                "status" => f.status = value.split(',').map(String::from).collect(),
                "property" => f.property = Some(value),
                "text" => f.text = Some(value),
                "title" => bound.title = Some(value),
                _ => {
                    return Err(format!(
                        "{key} is not part of a query: type, tag, folder, note, status, property, text or title"
                    ));
                }
            }
        }
        if bound.select.kind.is_empty() {
            return Err("a query needs type=…, the kind or type of its items".into());
        }
        Ok(Source::Query(Box::new(bound)))
    }
}

impl Source {
    /// The short form `parse` reads back, as a form shows it to be edited: words for a query
    /// (`type=volcano tag=eruption title="Erupted" where vei >= 6`), `note:…`, `items:…`,
    /// `text:…`, or JSON for what the words can't say (a date range, a quote in a value, a comma
    /// in a label, a line break in text).
    pub fn short(&self) -> String {
        let json = || serde_json::to_string(self).unwrap_or_default();
        match self {
            Source::Note(path) => format!("note:{path}"),
            Source::Items(labels) if labels.iter().any(|l| l.contains(',')) => json(),
            Source::Items(labels) => format!("items:{}", labels.join(", ")),
            Source::Text(text) if text.contains('\n') => json(),
            Source::Text(text) => format!("text:{text}"),
            Source::Query(b) => {
                let f = &b.select.filter;
                let status = (!f.status.is_empty()).then(|| f.status.join(","));
                let pairs = [
                    ("type", Some(&b.select.kind)),
                    ("tag", f.tag.as_ref()),
                    ("folder", f.folder.as_ref()),
                    ("note", f.note.as_ref()),
                    ("status", status.as_ref()),
                    ("property", f.property.as_ref()),
                    ("text", f.text.as_ref()),
                    ("title", b.title.as_ref()),
                ];
                let values = || pairs.iter().filter_map(|(_, v)| *v);
                let quoted = |v: &str| v.contains('"') || v.contains(" where ");
                if f.date.is_some() || values().any(|v| quoted(v)) {
                    return json();
                }
                let mut words: Vec<String> = (pairs.iter())
                    .filter_map(|(k, v)| {
                        let v = (*v)?;
                        Some(match v.contains(char::is_whitespace) || v.is_empty() {
                            true => format!("{k}=\"{v}\""),
                            false => format!("{k}={v}"),
                        })
                    })
                    .collect();
                if let Some(c) = &b.condition {
                    words.push(format!("where {c}"));
                }
                words.join(" ")
            }
        }
    }
}

/// Words split at spaces, a `"…"` keeping its spaces (the quotes dropped).
fn words(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if quoted {
        return Err(format!("{s:?}: a quote isn't closed"));
    }
    if !word.is_empty() {
        out.push(word);
    }
    Ok(out)
}

/// One bound input: where from, and what it holds now.
#[derive(Clone, Debug)]
struct Input {
    source: Source,
    members: Vec<Member>,
    sub: Option<SubId>,
}

impl Input {
    /// What `${input}` alone and `${input.title}` say.
    fn title(&self, name: &str) -> String {
        match &self.source {
            Source::Query(b) => b.title.clone().unwrap_or_else(|| b.label()),
            Source::Note(_) => self
                .members
                .first()
                .and_then(|m| m.text("title"))
                .unwrap_or_default(),
            Source::Items(_) => name.to_string(),
            Source::Text(t) => t.lines().next().unwrap_or_default().to_string(),
        }
    }

    /// `${input.field}`: the input's own (`title`, `count`, a query's `type`, `tag`, `folder`,
    /// `note`), else the field of its one member.
    fn value(&self, name: &str, field: Option<&str>) -> String {
        let query = match &self.source {
            Source::Query(b) => Some(b),
            _ => None,
        };
        let filter = query.map(|b| &b.select.filter);
        let own = match field {
            None => Some(match &self.source {
                Source::Text(t) => t.clone(),
                Source::Items(items) => items.join(", "),
                _ => self.title(name),
            }),
            Some("title") => Some(self.title(name)),
            Some("count") => Some(self.members.len().to_string()),
            Some("type" | "kind") => query.map(|b| b.select.kind.clone()),
            Some("tag") => filter
                .and_then(|f| f.tag.as_ref())
                .map(|t| t.trim_start_matches('#').to_string()),
            Some("folder") => filter.and_then(|f| f.folder.clone()),
            Some("note") if query.is_some() => filter.and_then(|f| f.note.clone()),
            Some(_) => None,
        };
        own.or_else(|| match (&self.members[..], field) {
            ([one], Some(f)) => one.text(f),
            _ => None,
        })
        .unwrap_or_default()
    }

    /// Reads its members from the core as it is now.
    fn read(&mut self, core: &Core) -> Result<(), String> {
        self.members = match &self.source {
            Source::Query(b) => {
                let q = Query {
                    select: b.select.clone(),
                    page: Default::default(),
                };
                let found = core.query_items(&q).map_err(|e| e.message)?;
                let condition = (b.condition.as_deref().map(Cond::parse))
                    .transpose()
                    .map_err(|e| format!("where: {e}"))?;
                (found.items.iter())
                    .map(Member::of_item)
                    .filter(|m| condition.as_ref().is_none_or(|c| c.holds(m)))
                    .collect()
            }
            Source::Note(path) => note(core, path).into_iter().collect(),
            Source::Items(items) => items.iter().map(|i| Member::label(i)).collect(),
            Source::Text(t) => vec![Member::label(t)],
        };
        Ok(())
    }
}

/// The note at `path` as a member: its `title` and `name` (the file's stem), `path`, `tags`
/// and `excerpt` (its first lines of text, after the frontmatter and a first heading or line that is its name).
fn note(core: &Core, path: &str) -> Option<Member> {
    let rel = path.strip_suffix(".md").unwrap_or(path);
    let ws = &core.index.workspace;
    let note = &ws.notes[ws
        .notes
        .binary_search_by(|n| n.rel.as_str().cmp(rel))
        .ok()?];
    let body = match crate::frontmatter::read(&note.text) {
        Some(f) => note.text.lines().skip(f.end_line).collect::<Vec<_>>(),
        None => note.text.lines().collect(),
    };
    let mut lines = body
        .into_iter()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .peekable();
    let titled = |l: &&str| l.starts_with("# ") || l.trim_start_matches('#').trim() == note.name;
    if lines.peek().is_some_and(titled) {
        lines.next();
    }
    let excerpt: Vec<&str> = lines.take(3).collect();
    let tags = note.tags.iter().cloned().map(Json::String).collect();
    let fields = IndexMap::from([
        ("title".to_string(), Json::String(note.name.clone())),
        ("name".to_string(), Json::String(note.name.clone())),
        ("path".to_string(), Json::String(format!("{rel}.md"))),
        ("tags".to_string(), Json::Array(tags)),
        ("excerpt".to_string(), Json::String(excerpt.join(" "))),
    ]);
    Some(Member {
        id: format!("{rel}.md"),
        rev: None,
        fields,
    })
}

/// A placed shape's binding, live: the shape, its bound inputs and what they hold.
#[derive(Clone, Debug)]
pub struct Binding {
    shape: Loaded,
    /// The shape's inputs that are bound, in its order.
    inputs: IndexMap<String, Input>,
    /// Each region's `shows`, read, in the shape's order.
    regions: Vec<Option<Shows>>,
    /// What the shape's extension answered for its regions with `extension = true`, by id.
    computed: HashMap<String, Vec<Entry>>,
}

/// What a placed shape shows now: each region's members, and each text part's text.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Shown {
    pub regions: Vec<RegionShown>,
    pub texts: Vec<TextShown>,
}

/// A region and what it shows, in order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RegionShown {
    pub id: String,
    pub items: Vec<Entry>,
}

/// One member as a region shows it: what it is, and its label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// A member's id; an extension may leave it out for a label of its own.
    #[serde(default)]
    pub id: String,
    pub label: String,
}

/// A text or a label of the shape, by its index among the shape's parts, filled in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextShown {
    pub part: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub text: String,
}

impl Binding {
    /// Binds the shape `shape` with `sources` by input name: each query input is followed
    /// from now on. An input left out stays unbound (`${a.title}` shows as `a.title`).
    pub fn new(
        core: &mut Core,
        shapes: &Shapes,
        shape: &str,
        sources: impl IntoIterator<Item = (String, Source)>,
    ) -> Result<Binding, String> {
        let sources: IndexMap<String, Source> = sources.into_iter().collect();
        let loaded = shapes
            .get(shape)
            .ok_or_else(|| format!("no shape is called {shape}"))?;
        let d = &loaded.definition;
        for name in sources.keys() {
            if !d.inputs.contains(name) {
                let has = match d.inputs.is_empty() {
                    true => "none".to_string(),
                    false => d.inputs.join(", "),
                };
                return Err(format!("{shape} has no input {name} (its inputs: {has})"));
            }
        }
        let regions = (d.regions.iter())
            .map(|r| {
                r.shows
                    .as_deref()
                    .map(|s| Shows::parse(s, &d.inputs))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        let mut binding = Binding {
            shape: loaded.clone(),
            inputs: IndexMap::new(),
            regions,
            computed: HashMap::new(),
        };
        for name in &d.inputs {
            let Some(source) = sources.get(name) else {
                continue;
            };
            let mut input = Input {
                source: source.clone(),
                members: Vec::new(),
                sub: None,
            };
            let read = input.read(core).and_then(|()| match &input.source {
                Source::Query(b) => (core.subscribe_items(OWNER, b.select.clone()))
                    .map(|sub| input.sub = Some(sub))
                    .map_err(|e| e.message),
                _ => Ok(()),
            });
            if let Err(e) = read {
                binding.close(core);
                return Err(format!("input {name}: {e}"));
            }
            binding.inputs.insert(name.clone(), input);
        }
        Ok(binding)
    }

    /// The shape it binds.
    pub fn shape(&self) -> &Loaded {
        &self.shape
    }

    /// What each bound input is bound to.
    pub fn sources(&self) -> IndexMap<String, Source> {
        (self.inputs.iter())
            .map(|(n, i)| (n.clone(), i.source.clone()))
            .collect()
    }

    /// Stops following its queries.
    pub fn close(&mut self, core: &mut Core) {
        for input in self.inputs.values_mut() {
            if let Some(sub) = input.sub.take() {
                core.unsubscribe_items(OWNER, sub);
            }
        }
    }

    /// News for one of its subscriptions (`items/changed`, `items/reset`): the input is read
    /// again. Whether it was one of its own.
    pub fn heard(&mut self, core: &Core, news: &News) -> bool {
        let sub = news.subscription();
        let input = self.inputs.values_mut().find(|i| i.sub == Some(sub));
        input.is_some_and(|i| i.read(core).is_ok())
    }

    /// A core event: a bound note that changed is read again, and everything after a rescan.
    /// Whether anything it shows may have changed.
    pub fn noticed(&mut self, core: &Core, event: &Event) -> bool {
        let mut changed = false;
        for input in self.inputs.values_mut() {
            let again = match (&input.source, event) {
                (_, Event::Rescanned) => true,
                (
                    Source::Note(path),
                    Event::NoteCreated(rel) | Event::NoteChanged(rel) | Event::NoteRemoved(rel),
                ) => path.strip_suffix(".md") == Some(rel.as_str()),
                _ => false,
            };
            if again {
                changed |= input.read(core).is_ok();
            }
        }
        changed
    }

    /// `${name}`: an input's value (`${a}`, `${a.title}`, `${note.excerpt}`); an unbound
    /// input's placeholder shows its name, as an unbound shape does.
    pub fn value(&self, name: &str) -> String {
        let (input, field) = match name.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (name, None),
        };
        match self.inputs.get(input) {
            Some(i) => i.value(input, field),
            None => name.to_string(),
        }
    }

    /// The canvas shape with its text filled in, to draw.
    pub fn canvas(&self) -> Arc<canvas::Shape> {
        Arc::new(self.shape.shape.map_text(|v| fill(v, |n| self.value(n))))
    }

    /// What it shows now: each region's members in order with their labels, and each text.
    pub fn shown(&self) -> Shown {
        let d = &self.shape.definition;
        let empty: Vec<Member> = Vec::new();
        let members: Vec<&[Member]> = (d.inputs.iter())
            .map(|n| self.inputs.get(n).map_or(&empty[..], |i| &i.members[..]))
            .collect();
        let regions = (d.regions.iter().zip(&self.regions))
            .map(|(r, shows)| RegionShown {
                id: r.id.clone(),
                items: match (r.extension, shows) {
                    (true, _) => self.computed.get(&r.id).cloned().unwrap_or_default(),
                    (false, None) => Vec::new(),
                    (false, Some(s)) => (s.eval(&members, d.key.as_deref()).into_iter())
                        .map(|m| Entry {
                            id: m.id.clone(),
                            label: s.label(m),
                        })
                        .collect(),
                },
            })
            .collect();
        let canvas = self.canvas();
        let texts = (canvas.parts().iter().enumerate())
            .filter_map(|(i, p)| match &p.kind {
                canvas::Kind::Text { value, .. } | canvas::Kind::Label { value, .. } => {
                    Some(TextShown {
                        part: i,
                        id: p.id.clone(),
                        text: value.clone(),
                    })
                }
                _ => None,
            })
            .collect();
        Shown { regions, texts }
    }

    /// Becomes the shape `to.shape` bound to `to.inputs` (a `rebind` step's result): the old
    /// subscriptions end once the new binding is made, and it stays as it was if that fails.
    pub fn rebind(&mut self, core: &mut Core, shapes: &Shapes, to: &Rebind) -> Result<(), String> {
        let new = Binding::new(core, shapes, &to.shape, to.inputs.clone())?;
        self.close(core);
        *self = new;
        Ok(())
    }

    /// The extension that ships the shape: its gesture steps run as it, within its permissions,
    /// and it answers for regions and gestures marked `extension = true`. `None` for the user's
    /// shapes (built in, config, workspace), whose steps run as `canvas`.
    pub fn extension(&self) -> Option<&str> {
        match &self.shape.origin {
            Origin::Extension { name } => Some(name),
            _ => None,
        }
    }

    /// `shape/region` for each region its extension computes, to ask after binding and again
    /// whenever an input changes; `placed` names this placed shape in the answers.
    pub fn region_asks(&self, placed: u64) -> Vec<Ask> {
        (self.shape.definition.regions.iter())
            .filter(|r| r.extension)
            .map(|r| Ask::Region {
                placed,
                shape: self.shape.id.clone(),
                region: r.id.clone(),
                inputs: self.data(),
            })
            .collect()
    }

    /// The extension's answer for `region`: what it shows until the next one.
    pub fn answered(&mut self, region: &str, items: Vec<Entry>) {
        if (self.shape.definition.regions.iter()).any(|r| r.id == region && r.extension) {
            self.computed.insert(region.to_string(), items);
        }
    }

    /// The inputs as an extension is told them.
    pub(crate) fn data(&self) -> Inputs {
        (self.inputs.iter())
            .map(|(name, i)| {
                let members = i.members.iter().map(MemberData::from).collect();
                let title = i.title(name);
                (name.clone(), InputData { title, members })
            })
            .collect()
    }

    /// The member with id `id`, in any input.
    pub(crate) fn member(&self, id: &str) -> Option<&Member> {
        (self.inputs.values())
            .flat_map(|i| &i.members)
            .find(|m| m.id == id)
    }

    /// The one member of the input `name`.
    pub(crate) fn only_member(&self, name: &str) -> Option<&Member> {
        match &self.inputs.get(name)?.members[..] {
            [one] => Some(one),
            _ => None,
        }
    }

    /// What the input `name` is bound to.
    pub(crate) fn source(&self, name: &str) -> Option<&Source> {
        self.inputs.get(name).map(|i| &i.source)
    }
}

impl Shown {
    /// Each region's labels, in the shape's order: what `draw::preview` fits into them.
    pub fn contents(&self) -> Vec<Vec<String>> {
        (self.regions.iter())
            .map(|r| r.items.iter().map(|e| e.label.clone()).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Layer, load};
    use std::path::PathBuf;

    fn workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-bind-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        std::fs::write(
            dir.join(".cold-lava/types.toml"),
            "version = 1\n[types.volcano]\non = \"note\"\nwhere.tag = [\"volcano\"]\n\
             fields.name = { note = \"name\" }\nfields.vei = { property = \"vei\", type = \"number\" }\n",
        )
        .unwrap();
        for (name, tags, vei) in [
            ("Krakatoa", "volcano, ring-of-fire", 6),
            ("Mount Fuji", "volcano, ring-of-fire", 5),
            ("Yellowstone", "volcano", 8),
            ("Vesuvius", "volcano", 5),
        ] {
            let text = format!("---\ntags: [{tags}]\nvei: {vei}\n---\n# {name}\n\nAbout {name}.\n");
            std::fs::write(dir.join(format!("{name}.md")), text).unwrap();
        }
        dir
    }

    fn sources(pairs: &[(&str, &str)]) -> IndexMap<String, Source> {
        (pairs.iter())
            .map(|(n, s)| (n.to_string(), Source::parse(s).unwrap()))
            .collect()
    }

    fn labels(shown: &Shown, region: &str) -> Vec<String> {
        let r = shown.regions.iter().find(|r| r.id == region).unwrap();
        r.items.iter().map(|e| e.label.clone()).collect()
    }

    fn built_in() -> Shapes {
        load([Layer::built_in()])
    }

    #[test]
    fn a_venn_bound_to_two_queries_shows_what_they_share() {
        let dir = workspace("venn");
        let mut core = Core::open(&dir).unwrap();
        let shapes = built_in();
        let bound = sources(&[
            ("a", "type=volcano tag=ring-of-fire title=\"Ring of Fire\""),
            ("b", "type=volcano tag=volcano"),
        ]);
        let b = Binding::new(&mut core, &shapes, "venn2", bound).unwrap();
        let shown = b.shown();
        assert_eq!(labels(&shown, "only-a"), Vec::<String>::new());
        assert_eq!(labels(&shown, "both"), ["Krakatoa", "Mount Fuji"]);
        assert_eq!(labels(&shown, "only-b"), ["Vesuvius", "Yellowstone"]);
        let texts: Vec<&str> = shown.texts.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            ["Ring of Fire", "volcano #volcano"],
            "titles: given, else the query"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_edited_item_changes_what_the_regions_show() {
        let dir = workspace("live");
        let mut core = Core::open(&dir).unwrap();
        let shapes = built_in();
        let bound = sources(&[
            ("a", "type=volcano tag=ring-of-fire"),
            ("b", "items:vesuvius"),
        ]);
        let mut b = Binding::new(&mut core, &shapes, "venn2", bound).unwrap();
        assert_eq!(labels(&b.shown(), "only-a"), ["Krakatoa", "Mount Fuji"]);
        assert_eq!(labels(&b.shown(), "only-b"), ["vesuvius"]);

        // Vesuvius joins the Ring of Fire, as another program writes it.
        let path = dir.join("Vesuvius.md");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("[volcano]", "[volcano, ring-of-fire]")).unwrap();
        core.note_changed_now(&path);
        let news = core.take_item_news();
        assert!(!news.is_empty(), "the binding's query hears of it");
        for (owner, n) in &news {
            assert_eq!(owner, OWNER);
            assert!(b.heard(&core, n));
        }
        let shown = b.shown();
        assert_eq!(labels(&shown, "only-a"), ["Krakatoa", "Mount Fuji"]);
        assert_eq!(
            labels(&shown, "both"),
            ["Vesuvius"],
            "matched on name, folded"
        );
        assert_eq!(labels(&shown, "only-b"), Vec::<String>::new());

        b.close(&mut core);
        std::fs::write(&path, text).unwrap();
        core.note_changed_now(&path);
        assert!(
            core.take_item_news().is_empty(),
            "closed: no longer followed"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_card_shows_its_note_and_follows_it() {
        let dir = workspace("card");
        let mut core = Core::open(&dir).unwrap();
        let shapes = built_in();
        let mut b = Binding::new(
            &mut core,
            &shapes,
            "card",
            sources(&[("note", "note:Krakatoa")]),
        )
        .unwrap();
        assert_eq!(b.value("note.title"), "Krakatoa");
        assert_eq!(b.value("note.excerpt"), "About Krakatoa.");
        let texts: Vec<String> = b.shown().texts.into_iter().map(|t| t.text).collect();
        assert_eq!(texts, ["Krakatoa", "About Krakatoa."]);

        let path = dir.join("Krakatoa.md");
        std::fs::write(&path, "# Krakatoa\n\nBetween Java and Sumatra.\n").unwrap();
        let events = core.note_changed_now(&path);
        assert!(events.iter().any(|e| b.noticed(&core, e)));
        assert_eq!(b.value("note.excerpt"), "Between Java and Sumatra.");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ring_of_fire_against_vei_6_and_up() {
        let dir = workspace("vei");
        let mut core = Core::open(&dir).unwrap();
        let shapes = built_in();
        let bound = sources(&[
            ("a", "type=volcano tag=ring-of-fire"),
            ("b", "type=volcano where vei >= 6"),
        ]);
        let mut b = Binding::new(&mut core, &shapes, "venn2", bound).unwrap();
        let shown = b.shown();
        assert_eq!(labels(&shown, "only-a"), ["Mount Fuji"]);
        assert_eq!(labels(&shown, "both"), ["Krakatoa"]);
        assert_eq!(labels(&shown, "only-b"), ["Yellowstone"]);
        assert_eq!(b.value("b.title"), "volcano where vei >= 6");

        // Vesuvius's VEI is raised to 6: the condition is applied again as the item changes.
        let path = dir.join("Vesuvius.md");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("vei: 5", "vei: 6")).unwrap();
        core.note_changed_now(&path);
        for (_, n) in core.take_item_news() {
            b.heard(&core, &n);
        }
        assert_eq!(labels(&b.shown(), "only-b"), ["Vesuvius", "Yellowstone"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn placeholders_name_the_input_and_its_field() {
        let dir = workspace("fill");
        let mut core = Core::open(&dir).unwrap();
        let shapes = built_in();
        let b = Binding::new(
            &mut core,
            &shapes,
            "venn2",
            sources(&[("b", "type=volcano tag=ring-of-fire")]),
        )
        .unwrap();
        assert_eq!(b.value("b.tag"), "ring-of-fire");
        assert_eq!(b.value("b.type"), "volcano");
        assert_eq!(b.value("b.count"), "2");
        assert_eq!(b.value("b"), "volcano #ring-of-fire");
        assert_eq!(b.value("a.title"), "a.title", "unbound");
        let sticky = Binding::new(
            &mut core,
            &shapes,
            "sticky",
            sources(&[("text", "text:Buy ash")]),
        )
        .unwrap();
        assert_eq!(sticky.shown().texts[0].text, "Buy ash");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn binding_mistakes_are_one_line() {
        let dir = workspace("wrong");
        let mut core = Core::open(&dir).unwrap();
        let shapes = built_in();
        let mut e = |shape: &str, pairs: &[(&str, &str)]| {
            Binding::new(&mut core, &shapes, shape, sources(pairs)).unwrap_err()
        };
        assert_eq!(e("hexagon", &[]), "no shape is called hexagon");
        assert_eq!(
            e("venn2", &[("c", "items:x")]),
            "venn2 has no input c (its inputs: a, b)"
        );
        assert_eq!(
            e("venn2", &[("a", "type=vulcano")]),
            "input a: no item kind or type called vulcano"
        );
        assert!(Source::parse("type=volcano colour=red").is_err());
        assert!(Source::parse("tag=x").is_err(), "a query needs a type");
        assert_eq!(
            Source::parse("type=volcano where vei >=").unwrap_err(),
            "where vei >=: expected a value after vei >="
        );
        assert_eq!(
            Source::parse(r#"{"note": "A.md"}"#).unwrap(),
            Source::Note("A.md".into())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_short_form_reads_back_as_the_same_source() {
        for short in [
            "type=volcano tag=eruption title=Erupted",
            "type=volcano tag=volcano/strato folder=Volcanoes title=\"Strato volcanoes\" where vei >= 6",
            "type=task status=todo,in_progress",
            "note:Volcanoes/Krakatoa.md",
            "items:Mount Fuji, Krakatoa",
            "text:Pack ash masks",
        ] {
            let source = Source::parse(short).unwrap();
            assert_eq!(source.short(), short);
            assert_eq!(Source::parse(&source.short()).unwrap(), source);
        }
        for odd in [
            Source::Text("two\nlines".into()),
            Source::Items(vec!["a, b".into()]),
            Source::parse(r#"{"query": {"kind": "volcano", "title": "say \"hi\""}}"#).unwrap(),
        ] {
            assert!(odd.short().starts_with('{'), "{}", odd.short());
            assert_eq!(Source::parse(&odd.short()).unwrap(), odd);
        }
    }

    #[test]
    fn sources_on_the_wire() {
        let q = Source::parse("type=volcano tag=eruption title=Erupted").unwrap();
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            serde_json::json!({"query": {"kind": "volcano", "filter": {"tag": "eruption"}, "title": "Erupted"}})
        );
        for s in [
            q,
            Source::Note("Krakatoa.md".into()),
            Source::Items(vec!["a".into()]),
            Source::Text("hi".into()),
        ] {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), s);
        }
    }
}
