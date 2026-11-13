//! The reader of what another program answers, beside the types it reads into
//! (`owner::given`, `act::Reply`): which of serde's words may be shown (`said`), each item of
//! an owner's answer read on its own by its keys (`Kept`, `GivenItem::read`), a run read by its
//! keys (`Run`'s `Deserialize`), and what is kept of an item that didn't read (`LeftOut`).
//!
//! A value past its bound is refused by the bounded types as it is read (`crate::bounded`);
//! here an item that holds one, or that doesn't read for any other reason, is left out alone
//! with what was wrong, and the rest of the answer is read.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as Json;

use crate::alphabet::Role;
use crate::bounded::{
    FieldName, Fields, Message, PieceText, Place, Text, TypeName, TypedFields, ValueText,
};
use crate::owner::given::{FieldRun, GivenItem, Label, LinkRun, Marked, Piece, Run};
use crate::problem::{Problem, cut};
use crate::serde_words;
use crate::tasks::line_id;

/// What a run of a piece's text is, said when one is neither.
pub const A_RUN: &str = "a run is text, or a map with a link, a label, a field or a text";

/// What an answer to a shape is, said when one is neither.
pub const A_REPLY: &str =
    "an answer to shape/region has items, and an answer to shape/gesture has do";

/// The reader's own words that reach it through serde: each says nothing an answer wrote.
const OWN: [&str; 2] = [A_RUN, A_REPLY];

/// The most characters of serde's message that are shown: it may quote the answer.
const SAID: usize = 200;

/// Why something another program sent doesn't read, in words that say nothing but what it
/// wrote and what was expected: the reader's own (a bound's, `bounded::is_past`, or `OWN`),
/// and serde's where it is one of the forms that carry only what the answer wrote
/// (`serde_words::only_the_answer`), cut to 200 characters since it may quote the answer.
/// `None` for anything else.
pub fn said(e: &serde_json::Error) -> Option<String> {
    let message = serde_words::message_of(e);
    let ours = OWN.contains(&message.as_str()) || crate::bounded::is_past(&message);
    (ours || serde_words::only_the_answer(&message)).then(|| cut(&message, SAID))
}

/// An item of an answer that didn't read, as it is kept: its place (cut to 80 characters),
/// a hash of the JSON it was sent as, which is its identity (its keys' order included, so an
/// owner that gives it again with its keys reordered has changed it), and what was said of
/// it. It is kept in the owners' cache, and read back from it, as a part of the answer it
/// was left out of (`owner::cache`); the cache is the engine's own file, which bounds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeftOut {
    pub place: Text<Message>,
    pub key: Text<Message>,
    pub said: Text<Message>,
}

impl LeftOut {
    /// What is wrong with it, as reported.
    pub fn problem(&self) -> Problem {
        Problem::line(self.said.to_string())
    }
}

/// One entry of an answer's `items` as it is read: the item, or why it is left out. Only a
/// JSON value that doesn't parse refuses the answer.
///
/// An entry is held as a JSON value until its kind is picked: what bounds it then is the
/// message it came in (64 MiB) and serde_json's limit on how deep a value may nest (128).
/// It is then read into the types the guard follows (`Piece`, `MarkdownWire`, `TypedWire`),
/// where every bound applies.
pub(crate) enum Kept {
    Item(GivenItem),
    Left(LeftOut),
}

impl<'de> Deserialize<'de> for Kept {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Json::deserialize(d)?;
        let place = value
            .get("place")
            .and_then(Json::as_str)
            .map(|p| cut(p, 80));
        let key = line_id(&value.to_string());
        Ok(match GivenItem::read(value) {
            Ok(item) => Kept::Item(item),
            Err(why) => {
                // A place that isn't text (a number, a list) is none.
                let place = place.unwrap_or_else(|| "an item with no place".to_string());
                let said = format!("{place}: {}", cut(&why, SAID));
                Kept::Left(LeftOut {
                    place: place.into(),
                    key: key.into(),
                    said: said.into(),
                })
            }
        })
    }
}

/// A Markdown piece as the wire has it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkdownWire {
    place: Text<Place>,
    text: Text<PieceText>,
}

/// A typed item as the wire has it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypedWire {
    place: Text<Place>,
    #[serde(rename = "type")]
    kind: Text<TypeName>,
    #[serde(default)]
    fields: Fields<TypedFields, FieldName, ValueText>,
}

impl GivenItem {
    /// One item of an answer, of the kind its keys say: a `role` makes it a piece (and one
    /// with a `type` too is read as a piece, which has no `type`), a `type` a typed item,
    /// anything else a Markdown piece. A list is a piece written as a list, which serde reads
    /// as it reads a map and an untagged reading took. It is within its bounds, each of them,
    /// and a piece says nothing two ways (`GivenItem::check`). Else what is wrong with it, in
    /// the reader's words, or serde's where those say only what the answer wrote (`said`).
    pub(crate) fn read(value: Json) -> Result<GivenItem, String> {
        let (role, typed) = match &value {
            Json::Object(entry) => (entry.get("role").cloned(), entry.contains_key("type")),
            Json::Array(_) => (Some(Json::Null), false),
            _ => return Err("an item is a map: a piece, a typed item or a Markdown piece".into()),
        };
        if let Some(Json::String(name)) = &role
            && Role::from_name(name).is_none()
        {
            let roles: Vec<&str> = Role::ALL.iter().map(|r| r.name()).collect();
            return Err(format!(
                "its role {:?} is not a role: one of {}",
                cut(name, 80),
                roles.join(", ")
            ));
        }
        let (kind, item) = match (role.is_some(), typed) {
            (true, _) => (
                "a piece",
                serde_json::from_value::<Piece>(value).map(GivenItem::Piece),
            ),
            (false, true) => (
                "a typed item",
                serde_json::from_value::<TypedWire>(value).map(|t| GivenItem::Typed {
                    place: t.place,
                    kind: t.kind,
                    fields: t.fields,
                }),
            ),
            (false, false) => (
                "a Markdown piece",
                serde_json::from_value::<MarkdownWire>(value).map(|m| GivenItem::Markdown {
                    place: m.place,
                    text: m.text,
                }),
            ),
        };
        let item =
            item.map_err(|e| said(&e).unwrap_or_else(|| format!("it doesn't read as {kind}")))?;
        item.check()?;
        Ok(item)
    }
}

/// A run is read by its keys: a string is plain; a map with `link` a link, with `label` a
/// label, with `field` a field, and with `text` alone marked characters. So a mistake in one
/// is named (`unknown field `colour``) and not answered with "it matches no kind". A list is
/// a run written as a list, read as an untagged reading took it: the first of marked, link,
/// label and field it reads as.
impl<'de> Deserialize<'de> for Run {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let run = Json::deserialize(d)?;
        let key = |k: &str| run.get(k).is_some();
        let kind = match &run {
            Json::String(_) => "plain",
            Json::Array(_) => "list",
            Json::Object(_) if key("link") => "link",
            Json::Object(_) if key("label") => "label",
            Json::Object(_) if key("field") => "field",
            Json::Object(_) if key("text") => "marked",
            _ => return Err(D::Error::custom(A_RUN)),
        };
        let read = match kind {
            "plain" => serde_json::from_value(run).map(Run::Plain),
            "list" => {
                let as_list = (serde_json::from_value::<Marked>(run.clone()).map(Run::Marked))
                    .or_else(|_| serde_json::from_value::<LinkRun>(run.clone()).map(Run::Link))
                    .or_else(|_| serde_json::from_value::<Label>(run.clone()).map(Run::Label))
                    .or_else(|_| serde_json::from_value::<FieldRun>(run).map(Run::Field));
                return as_list.map_err(|_| D::Error::custom(A_RUN));
            }
            "link" => serde_json::from_value(run).map(Run::Link),
            "label" => serde_json::from_value(run).map(Run::Label),
            "field" => serde_json::from_value(run).map(Run::Field),
            _ => serde_json::from_value(run).map(Run::Marked),
        };
        read.map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::Error as _;

    /// Only the reader's own words and serde's forms that say what the answer wrote are
    /// shown, cut to 200 characters: a message that names a Rust type is not.
    #[test]
    fn what_serde_says_is_shown_only_when_it_says_what_the_answer_wrote() {
        let custom = |m: &str| said(&serde_json::Error::custom(m));
        assert_eq!(custom(A_RUN).as_deref(), Some(A_RUN));
        assert_eq!(custom(A_REPLY).as_deref(), Some(A_REPLY));
        let past = "its place is empty or longer than 1024 bytes";
        assert_eq!(custom(past).as_deref(), Some(past));
        assert_eq!(
            custom("data did not match any variant of untagged enum Run"),
            None
        );
        let unknown = serde_json::Error::unknown_field("colour", &["place", "text"]);
        assert_eq!(
            said(&unknown).as_deref(),
            Some("unknown field `colour`, expected `place` or `text`")
        );
        let long = serde_json::from_value::<u8>(Json::String("x".repeat(500))).unwrap_err();
        let shown = said(&long).unwrap();
        assert_eq!(shown.chars().count(), SAID + 1, "{shown}");
        assert!(shown.ends_with('…'));
    }
}
