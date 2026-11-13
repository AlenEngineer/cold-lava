//! Behaviour: what a placed shape does when it is used (docs/shapes.md, "Behaviour, as built").
//! A gesture (`drop`, `click`, `key`, an `action`) picks the shape's `on` entry, and its steps
//! run on the workspace gateway: `edit` an item (`set-field`, `toggle`) or the workspace (a
//! recipe), `create` a note where Obsidian puts new notes, `rebind` the shape as another one.
//!
//! Steps go through a `Gateway`: the core itself for the user's shapes (built in, config,
//! workspace), attributed to `canvas`; for an extension's shape, the extension's `Host`, which
//! checks its manifest's permissions and attributes the edits to it. A shape can also hand a
//! region or a gesture to the extension that ships it (`extension = true`): the messages for
//! that, `shape/region` and `shape/gesture`, are here too.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::bind::{Binding, Entry, OWNER, Source};
use crate::bounded::{List, Message, Owed, Text};
use crate::core::Core;
use crate::files::{Create, Created, NewFile};
use crate::gateway::{Answer, Edit, Error, Op, Reason};
use crate::problem;
use crate::recipe;
use crate::shapes::{Behaviour, EditStep, Shapes, Step, Steps, Way, fill};
use crate::shows::{Member, text_of};
use crate::workspace::search_key;

/// Something done to a placed shape, as the frontend saw it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "gesture", rename_all = "kebab-case")]
pub enum Gesture {
    /// The member `item` dragged from one region to another.
    Drop {
        from: String,
        to: String,
        item: String,
    },
    /// A click on a region or a part, by id, and on one of a region's members.
    Click {
        on: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    /// A key pressed while the shape is selected.
    Key { key: String },
    /// One of the actions the shape offers, by name.
    Action { name: String },
}

impl Gesture {
    /// Reads `action:NAME`, `key:K`, `click:ON[:ITEM]`, `drop:FROM:TO:ITEM`, or JSON
    /// (`{"gesture": "drop", …}`): what `cold-lava shapes --do` takes.
    pub fn parse(s: &str) -> Result<Gesture, String> {
        let s = s.trim();
        if s.starts_with('{') {
            return serde_json::from_str(s).map_err(|e| e.to_string());
        }
        let parts: Vec<&str> = s.splitn(4, ':').collect();
        let own = |i: usize| parts[i].to_string();
        Ok(match parts[..] {
            ["action", _] => Gesture::Action { name: own(1) },
            ["key", _] => Gesture::Key { key: own(1) },
            ["click", _] => Gesture::Click {
                on: own(1),
                item: None,
            },
            ["click", _, _] => Gesture::Click {
                on: own(1),
                item: Some(own(2)),
            },
            ["drop", _, _, _] => Gesture::Drop {
                from: own(1),
                to: own(2),
                item: own(3),
            },
            _ => {
                return Err(format!(
                    "{s:?}: a gesture is action:NAME, key:K, click:ON[:ITEM] or drop:FROM:TO:ITEM"
                ));
            }
        })
    }

    /// Whether the `on` entry `b` answers it.
    fn starts(&self, b: &Behaviour) -> bool {
        match self {
            Gesture::Drop { from, to, .. } => {
                (b.drop.as_ref()).is_some_and(|d| d.from == *from && d.to == *to)
            }
            Gesture::Click { on, .. } => b.click.as_ref() == Some(on),
            Gesture::Key { key } => b.key.as_ref() == Some(key),
            Gesture::Action { name } => b.action.as_ref() == Some(name),
        }
    }

    /// The member it is about: the one dropped or clicked.
    fn item(&self) -> Option<&str> {
        match self {
            Gesture::Drop { item, .. } => Some(item),
            Gesture::Click { item, .. } => item.as_deref(),
            _ => None,
        }
    }
}

/// What a shape's steps ask of the workspace gateway. The core is one (`Direct`); a frontend's
/// gateway for an extension checks the extension's permissions first.
pub trait Gateway {
    /// `items/edit`
    fn edit_item(&mut self, edit: &Edit) -> Result<Answer, Error>;
    /// `workspace/create`
    fn create_file(&mut self, create: &Create) -> Result<Created, Error>;
    /// A recipe on the whole workspace, planned and applied; its title.
    fn run_recipe(&mut self, id: &str, options: recipe::Options) -> Result<String, Error>;
}

/// The core as the gateway of the user's own shapes: every edit attributed to `by`.
pub struct Direct<'a> {
    pub core: &'a mut Core,
    pub by: &'a str,
}

impl<'a> Direct<'a> {
    /// The user's gateway, attributed to the canvas.
    pub fn user(core: &'a mut Core) -> Direct<'a> {
        Direct { core, by: OWNER }
    }
}

impl Gateway for Direct<'_> {
    fn edit_item(&mut self, edit: &Edit) -> Result<Answer, Error> {
        self.core.edit_item(self.by, edit)
    }

    fn create_file(&mut self, create: &Create) -> Result<Created, Error> {
        self.core.create_file(create)
    }

    fn run_recipe(&mut self, id: &str, options: recipe::Options) -> Result<String, Error> {
        self.core.run_recipe(id, options)
    }
}

/// What running a gesture's steps did, step by step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Done {
    pub steps: Vec<Did>,
    /// The shape this placed shape is to become, same place and size: the frontend keeps the
    /// placement and calls `Binding::rebind`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebind: Option<Rebind>,
}

/// One step's result.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "step", rename_all = "kebab-case")]
pub enum Did {
    /// `items/edit` on `item`: made, or waiting for the owner of its file (`later`).
    Edited { item: String, later: bool },
    /// A note made at `path`.
    Created { path: String },
    /// A recipe applied.
    Recipe { title: String },
}

/// A placed shape becoming another shape with these inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rebind {
    pub shape: String,
    pub inputs: IndexMap<String, Source>,
}

/// What a gesture leads to: steps that ran, or a question for the shape's extension, whose
/// answer's steps `Binding::run` runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Acted {
    Ran(Done),
    Ask(Ask),
}

impl Binding {
    /// Runs the steps of the `on` entry `gesture` starts. Refused when none does; handed to
    /// the extension when the entry says so (`placed` names this placed shape in its answer).
    /// `shapes` are the loaded shapes, which a `rebind` names.
    pub fn act(
        &self,
        gw: &mut dyn Gateway,
        shapes: &Shapes,
        gesture: &Gesture,
        placed: u64,
    ) -> Result<Acted, Error> {
        let d = &self.shape().definition;
        let Some(b) = d.on.iter().find(|b| gesture.starts(b)) else {
            let message = format!("{} does nothing on {}", self.shape().id, describe(gesture));
            return Err(Error::new(Reason::Refused, message));
        };
        if b.extension {
            return Ok(Acted::Ask(Ask::Gesture {
                placed,
                shape: self.shape().id.clone(),
                gesture: gesture.clone(),
                inputs: self.data(),
            }));
        }
        // The loader checked them already. `run` takes only `Steps`, so that no path can hand
        // it steps nobody checked, and an entry's are made here by the same rules.
        let steps = Steps::checked(b.written(), shapes).map_err(|problems| {
            let message = format!("{}: {}", self.shape().id, problem::joined(&problems));
            Error::new(Reason::Refused, message)
        })?;
        self.run(gw, gesture, &steps).map(Acted::Ran)
    }

    /// Runs `steps` in order for `gesture`, stopping at the first refused. Each one's text has
    /// its placeholders filled: the inputs' (`${b.tag}`), the gesture's member's
    /// (`${dropped.name}`, `${clicked}`), and a note made by an earlier step
    /// (`${created.path}`, `${created.title}`). They are `Steps`, so each one passed the
    /// check, a shape's own and an extension's alike.
    pub fn run(
        &self,
        gw: &mut dyn Gateway,
        gesture: &Gesture,
        steps: &Steps,
    ) -> Result<Done, Error> {
        let mut done = Done {
            steps: Vec::new(),
            rebind: None,
        };
        let mut created: Option<String> = None;
        for step in steps.iter() {
            let value = |name: &str| self.step_value(gesture, created.as_deref(), name);
            match step {
                Step::Edit(e) => done.steps.push(self.edit(gw, gesture, e, &value)?),
                Step::Create(c) => {
                    let text = fill(&c.text, value);
                    let name = match &c.name {
                        Some(n) => fill(n, value),
                        None => name_of(&text),
                    };
                    let at = NewFile::Named {
                        name,
                        extension: "md".into(),
                    };
                    let made = gw.create_file(&Create { at, text })?;
                    done.steps.push(Did::Created {
                        path: made.path.clone(),
                    });
                    created = Some(made.path);
                }
                Step::Rebind(r) => {
                    let inputs = (r.inputs.iter())
                        .map(|(input, v)| {
                            let source = match (v.as_str(), &created, self.source(v)) {
                                ("created", Some(path), _) => Source::Note(path.clone()),
                                ("created", None, _) => {
                                    let message = "rebind: no earlier step created a note";
                                    return Err(Error::new(Reason::Refused, message));
                                }
                                (_, _, Some(s)) => s.clone(),
                                _ => Source::Text(fill(v, value)),
                            };
                            Ok((input.clone(), source))
                        })
                        .collect::<Result<_, _>>()?;
                    done.rebind = Some(Rebind {
                        shape: r.shape.to_string(),
                        inputs,
                    });
                }
            }
        }
        Ok(done)
    }

    fn edit(
        &self,
        gw: &mut dyn Gateway,
        gesture: &Gesture,
        e: &EditStep,
        value: &dyn Fn(&str) -> String,
    ) -> Result<Did, Error> {
        if let Some(id) = &e.recipe {
            let options = (e.options.iter())
                .map(|(k, v)| (k.clone(), fill(v, value)))
                .collect();
            let title = gw.run_recipe(id, options)?;
            return Ok(Did::Recipe { title });
        }
        let named = e.item.as_ref().map(|i| i.as_str()).unwrap_or_default();
        let member = self.item_of(gesture, named).ok_or_else(|| {
            let message = format!("{named} names no item here");
            Error::new(Reason::Refused, message)
        })?;
        let rev = member.rev.clone().ok_or_else(|| {
            let message = format!("{} is not an item, so it can't be edited", member.id);
            Error::new(Reason::Refused, message)
        })?;
        let op = match &e.set_field {
            Some(s) => {
                let value = match s.way().map_err(|why| Error::new(Reason::Refused, why))? {
                    Way::Value(v) => Json::String(fill(v, value)),
                    Way::Add(v) => listed(member, &s.field, &fill(v, value), true)?,
                    Way::Remove(v) => listed(member, &s.field, &fill(v, value), false)?,
                };
                Op::SetField {
                    field: s.field.to_string(),
                    value,
                }
            }
            None => Op::Toggle,
        };
        let edit = Edit {
            id: member.id.clone(),
            rev,
            op,
        };
        let later = matches!(gw.edit_item(&edit)?, Answer::Later(_));
        Ok(Did::Edited {
            item: member.id.clone(),
            later,
        })
    }

    /// The member an edit's `item` names: the gesture's (`dropped`, `clicked`), or the one
    /// member of an input.
    fn item_of(&self, gesture: &Gesture, named: &str) -> Option<&Member> {
        match named {
            "dropped" | "clicked" | "item" => self.member(gesture.item()?),
            input => self.only_member(input),
        }
    }

    /// `${name}` in a step.
    fn step_value(&self, gesture: &Gesture, created: Option<&str>, name: &str) -> String {
        let (head, field) = match name.split_once('.') {
            Some((h, f)) => (h, Some(f)),
            None => (name, None),
        };
        match (head, field) {
            ("created", None | Some("path")) => created.unwrap_or_default().to_string(),
            ("created", Some("title" | "name")) => (created.unwrap_or_default())
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .trim_end_matches(".md")
                .to_string(),
            ("dropped" | "clicked" | "item", _) => {
                let member = gesture.item().and_then(|id| self.member(id));
                match (member, field) {
                    (Some(m), Some(f)) => m.text(f).unwrap_or_default(),
                    (Some(m), None) => m.default_label(),
                    (None, _) => String::new(),
                }
            }
            _ => self.value(name),
        }
    }
}

/// The list `field` of `m` with `value` added (once) or removed, compared folded.
fn listed(m: &Member, field: &str, value: &str, add: bool) -> Result<Json, Error> {
    let now: Vec<String> = match m.fields.get(field) {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Array(a)) => a.iter().filter_map(text_of).collect(),
        Some(_) => {
            let message = format!("{field} isn't a list, so nothing is added to or taken from it");
            return Err(Error::new(Reason::Refused, message));
        }
    };
    let same = |v: &String| {
        search_key(v.trim_start_matches('#')) == search_key(value.trim_start_matches('#'))
    };
    let mut list: Vec<String> = now.iter().filter(|v| add || !same(v)).cloned().collect();
    if add && !now.iter().any(same) {
        list.push(value.to_string());
    }
    Ok(Json::from(list))
}

/// A new note's name from its text: its first line, without a heading's `#`s, cut to a name a
/// file can have.
fn name_of(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Untitled");
    let name: String = (first.trim_start_matches('#').trim().chars())
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']'
            )
        })
        .take(80)
        .collect();
    match name.trim() {
        "" => "Untitled".to_string(),
        n => n.to_string(),
    }
}

fn describe(g: &Gesture) -> String {
    match g {
        Gesture::Drop { from, to, .. } => format!("a drop from {from} to {to}"),
        Gesture::Click { on, .. } => format!("a click on {on}"),
        Gesture::Key { key } => format!("the key {key}"),
        Gesture::Action { name } => format!("the action {name}"),
    }
}

/// A shape's inputs as an extension is told them, by name.
pub type Inputs = IndexMap<String, InputData>;

/// An input as an extension is told it: its title and what it holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputData {
    pub title: String,
    pub members: Vec<MemberData>,
}

/// A member on the wire: its id, its version when it is an item, its fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberData {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    pub fields: serde_json::Map<String, Json>,
}

impl From<&Member> for MemberData {
    fn from(m: &Member) -> MemberData {
        MemberData {
            id: m.id.clone(),
            rev: m.rev.clone(),
            fields: (m.fields.iter())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

/// What the host asks the extension that ships a shape (host → extension, requests). `placed`
/// is the frontend's name for the placed shape, given back in the answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Ask {
    /// What a region with `extension = true` shows now, given the inputs; answered with
    /// `Answer::Region`. Asked again when an input changes.
    #[serde(rename = "shape/region")]
    Region {
        placed: u64,
        shape: String,
        region: String,
        inputs: Inputs,
    },
    /// What to do for a gesture whose `on` entry says `extension = true`; answered with
    /// `Answer::Gesture`, whose steps run through the extension's own permissions.
    #[serde(rename = "shape/gesture")]
    Gesture {
        placed: u64,
        shape: String,
        gesture: Gesture,
        inputs: Inputs,
    },
}

impl Ask {
    pub fn placed(&self) -> u64 {
        match self {
            Ask::Region { placed, .. } | Ask::Gesture { placed, .. } => *placed,
        }
    }

    /// The shape it is about, by id: `<extension>.<name>`.
    pub fn shape(&self) -> &str {
        match self {
            Ask::Region { shape, .. } | Ask::Gesture { shape, .. } => shape,
        }
    }

    /// Its method on the wire.
    pub fn method(&self) -> &'static str {
        match self {
            Ask::Region { .. } => "shape/region",
            Ask::Gesture { .. } => "shape/gesture",
        }
    }
}

/// An extension's answer to an `Ask`, or why there is none, as the core keeps it for the
/// frontend (`Core::take_shape_answers`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeAnswer {
    /// The extension that answered.
    pub by: String,
    pub ask: Ask,
    pub answer: Result<Answered, String>,
}

/// The extension's answer to an `Ask` as it wrote it, within its bounds. As it arrives, it
/// is checked for what needs no shapes (`arrived`); `read` makes it an `Answered`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Reply {
    /// To `shape/region`: the members it shows, each an id and a label.
    Region { items: List<RegionEntry, Owed> },
    /// To `shape/gesture`: steps, in the language of `on`'s `do`.
    Gesture {
        #[serde(rename = "do")]
        steps: List<Step, Owed>,
    },
}

/// An answer to `shape/region` as the wire has it.
#[derive(Deserialize)]
struct RegionWire {
    items: List<RegionEntry, Owed>,
}

/// An answer to `shape/gesture` as the wire has it.
#[derive(Deserialize)]
struct GestureWire {
    #[serde(rename = "do")]
    steps: List<Step, Owed>,
}

/// An answer is read by what it says, as an untagged reading did: `items` that read make it
/// a region's answer, else `do` that reads makes it a gesture's, whatever the other key
/// holds. When only one of the two is there and it doesn't read, what is wrong with it is
/// named (`missing field `label``); when neither reads, or neither is there, the answer
/// says what the two forms are (`owner::answer::A_REPLY`).
impl<'de> Deserialize<'de> for Reply {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let answer = serde_json::Map::<String, Json>::deserialize(d)?;
        let (items, steps) = (answer.contains_key("items"), answer.contains_key("do"));
        let answer = Json::Object(answer);
        let region = match items.then(|| serde_json::from_value::<RegionWire>(answer.clone())) {
            Some(Ok(r)) => return Ok(Reply::Region { items: r.items }),
            other => other,
        };
        let gesture = steps.then(|| serde_json::from_value::<GestureWire>(answer));
        match (region, gesture) {
            (_, Some(Ok(g))) => Ok(Reply::Gesture { steps: g.steps }),
            (Some(Err(e)), None) | (None, Some(Err(e))) => Err(D::Error::custom(e)),
            _ => Err(D::Error::custom(crate::owner::answer::A_REPLY)),
        }
    }
}

/// One member a region shows, as the extension answers it: what it is (left out for a label
/// of its own) and its label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionEntry {
    #[serde(default)]
    pub id: Text<Message>,
    pub label: Text<Message>,
}

impl From<RegionEntry> for Entry {
    fn from(shown: RegionEntry) -> Entry {
        Entry {
            id: shown.id.into_string(),
            label: shown.label.into_string(),
        }
    }
}

impl Reply {
    /// It as it arrives for `ask`: refused when it isn't in the form the ask has, taken when
    /// its steps pass the rules a step meets on its own (`Steps::alone`), which need no
    /// shapes, and else refused for what they get wrong with only its rebinds kept, so the
    /// refusal can also name a shape that isn't loaded, as a loaded shape's steps are
    /// refused for both (`Replied::read`). The core keeps nothing else
    /// (`Core::shape_answered`).
    pub fn arrived(self, ask: &Ask) -> Result<Arrived, String> {
        let problems = match (ask, &self) {
            (Ask::Region { .. }, Reply::Region { .. }) => Vec::new(),
            (Ask::Gesture { .. }, Reply::Gesture { steps }) => Steps::alone(steps),
            (Ask::Region { .. }, _) => return Err("it answered shape/region with steps".into()),
            (Ask::Gesture { .. }, _) => return Err("it answered shape/gesture with items".into()),
        };
        if problems.is_empty() {
            return Ok(Arrived::Taken(self));
        }
        let Reply::Gesture { steps } = self else {
            return Err(cant_run(problems));
        };
        let rebinds = steps.into_iter().filter(|s| matches!(s, Step::Rebind(_)));
        Ok(Arrived::Refused {
            problems,
            rebinds: rebinds.collect(),
        })
    }

    /// It as the answer to `ask`: only in the form the ask has, and its steps only when they
    /// pass the rules a loaded shape's steps pass, among the loaded `shapes`. Else why it
    /// isn't taken.
    pub fn read(self, ask: &Ask, shapes: &Shapes) -> Result<Answered, String> {
        match (ask, self) {
            (Ask::Region { .. }, Reply::Region { items }) => Ok(Answered::Region {
                items: items.into_iter().map(Entry::from).collect(),
            }),
            (Ask::Gesture { .. }, Reply::Gesture { steps }) => {
                Steps::checked(steps.into_vec(), shapes)
                    .map(|steps| Answered::Gesture { steps })
                    .map_err(cant_run)
            }
            (Ask::Region { .. }, _) => Err("it answered shape/region with steps".into()),
            (Ask::Gesture { .. }, _) => Err("it answered shape/gesture with items".into()),
        }
    }
}

/// Why an answer's steps don't run: the first `problem::PROBLEMS` of what is wrong, then how
/// many more, on one line.
fn cant_run(problems: Vec<crate::problem::Problem>) -> String {
    format!(
        "it answered shape/gesture with steps that can't run: {}",
        problem::joined(&crate::problem::capped(problems))
    )
}

/// A reply as it arrived (`Reply::arrived`): taken, or refused for what its steps get wrong
/// on their own, with its rebinds kept for the shapes to be asked when it is taken.
#[derive(Clone, Debug, PartialEq)]
pub enum Arrived {
    Taken(Reply),
    Refused {
        problems: Vec<crate::problem::Problem>,
        rebinds: Vec<Step>,
    },
}

/// What the extension `by` replied to `ask`, or why it didn't, as the core keeps it until a
/// frontend takes it with its loaded shapes.
#[derive(Clone, Debug, PartialEq)]
pub struct Replied {
    pub by: String,
    pub ask: Ask,
    pub reply: Result<Arrived, String>,
}

impl Replied {
    /// The answer it makes among the loaded `shapes`. A reply refused as it arrived is
    /// refused for that and for every rebind to a shape that isn't loaded, in the order a
    /// loaded shape's steps are (`Steps::checked`).
    pub fn read(self, shapes: &Shapes) -> ShapeAnswer {
        let answer = (self.reply).and_then(|arrived| match arrived {
            Arrived::Taken(reply) => reply.read(&self.ask, shapes),
            Arrived::Refused {
                mut problems,
                rebinds,
            } => {
                problems.extend(Steps::unknown(&rebinds, shapes));
                Err(cant_run(problems))
            }
        });
        ShapeAnswer {
            by: self.by,
            ask: self.ask,
            answer,
        }
    }
}

/// An extension's answer to an `Ask`, read: in the ask's form, its steps checked.
#[derive(Clone, Debug, PartialEq)]
pub enum Answered {
    /// To `shape/region`: the members it shows, each an id and a label.
    Region { items: Vec<Entry> },
    /// To `shape/gesture`: the steps to run.
    Gesture { steps: Steps },
}

#[cfg(test)]
mod tests;
