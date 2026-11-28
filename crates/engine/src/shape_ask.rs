//! The messages of a shape's extension (docs/shapes.md, "Behaviour, as built"): what the host
//! asks it for a region or a gesture (`shape/region`, `shape/gesture`), what it replies, and
//! the answer the core keeps for the frontend (`Core::take_shape_answers`). The core, the
//! bindings and behaviour (`act`) all read them, and they read none of those.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::bounded::{GestureSteps, List, Message, RegionItems, Text};
use crate::problem;
use crate::shapes::{Behaviour, Shapes, Step, Steps};
use crate::shows::{Entry, Member};

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
    pub(crate) fn starts(&self, b: &Behaviour) -> bool {
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
    pub(crate) fn item(&self) -> Option<&str> {
        match self {
            Gesture::Drop { item, .. } => Some(item),
            Gesture::Click { item, .. } => item.as_deref(),
            _ => None,
        }
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
    Region {
        items: List<RegionEntry, RegionItems>,
    },
    /// To `shape/gesture`: steps, in the language of `on`'s `do`.
    Gesture {
        #[serde(rename = "do")]
        steps: List<Step, GestureSteps>,
    },
}

/// An answer to `shape/region` as the wire has it.
#[derive(Deserialize)]
struct RegionWire {
    items: List<RegionEntry, RegionItems>,
}

/// An answer to `shape/gesture` as the wire has it.
#[derive(Deserialize)]
struct GestureWire {
    #[serde(rename = "do")]
    steps: List<Step, GestureSteps>,
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
