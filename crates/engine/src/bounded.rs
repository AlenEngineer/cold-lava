//! What another program sends, held in fields that each name their bound: an owner's answers
//! to `file/read` and `file/edit` (`owner::given`, `owner::edits`) and an extension's answers
//! to `shape/region` and `shape/gesture` (`act::Reply`, the steps of `shapes`). Each field of
//! those types is a `Text`, a `List`, `Fields` or `Pairs` whose last parameters name its
//! bound: `place: Text<Place>` is a place of 1 to 1024 bytes. A field that only the message
//! bounds says so, `Text<Message>`, so no field is unbounded without saying it.
//! `crates/cold-lava/tests/one_door.rs` fails on a bare `String`, `Vec`, map or JSON value in
//! one of those types.
//!
//! The bounds are the table below, one type each, so a number is written once and every
//! field that has it names it. They are today's: where a field has no bound of its own yet,
//! it is `Message`. Who applies a bound, and what a value past it costs (the item left out,
//! the answer refused), is said at the bound.
//!
//! A leaf: it needs serde, serde_json and indexmap. The types are transparent on the wire, so
//! what they serialize is what a `String`, a `Vec` or a map serialized: an item's `rev`
//! hashes that JSON, and it doesn't move.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json};

/// A bound: the least and the most a field may hold, in bytes for a text and in entries for
/// a list or a map.
pub trait Bound {
    const LEAST: usize = 0;
    const MOST: usize;
}

/// Bounded only by the message it came in: 64 MiB (the host's frames, `frame::MAX_MESSAGE`).
pub struct Message;
impl Bound for Message {
    const MOST: usize = 64 << 20;
}

/// A piece's place: 1 to 1024 bytes (`Piece::check`; the piece is left out).
pub struct Place;
impl Bound for Place {
    const LEAST: usize = 1;
    const MOST: usize = 1024;
}

/// A link run's target: 1 to 4096 bytes (`Piece::check`; the piece is left out).
pub struct Target;
impl Bound for Target {
    const LEAST: usize = 1;
    const MOST: usize = 4096;
}

/// A cell's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct CellName;
impl Bound for CellName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// An attribute's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct AttributeName;
impl Bound for AttributeName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// A label's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct LabelName;
impl Bound for LabelName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// A field run's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct FieldName;
impl Bound for FieldName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// The runs of a piece's text and its cells' together: at most 4096 (`Piece::check`; the
/// piece is left out).
pub struct Runs;
impl Bound for Runs {
    const MOST: usize = 4096;
}

/// A piece's attributes: at most 256 (`Piece::check`; the piece is left out).
pub struct Attributes;
impl Bound for Attributes {
    const MOST: usize = 256;
}

/// The label of a link between two pieces: at most 4096 bytes. Applied with the answer
/// (`given::Places::joins`), since the link is left out alone and named by its two ends.
pub struct LinkLabel;
impl Bound for LinkLabel {
    const MOST: usize = 4096;
}

/// The links between pieces of an answer: at most 65,536. Applied with the answer
/// (`given::Places::joins`): the rest are left out, and that is said once.
pub struct Betweens;
impl Bound for Betweens {
    const MOST: usize = 65_536;
}

/// Whether `n` is within `B`.
pub fn fits<B: Bound>(n: usize) -> bool {
    (B::LEAST..=B::MOST).contains(&n)
}

/// A text within the bound `B`, in bytes.
pub struct Text<B>(String, PhantomData<fn() -> B>);

impl<B> Text<B> {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// It with `more` after it. The engine's own change: what an answer holds is bounded as
    /// it is read, and a text the engine makes is checked where an answer's would be.
    pub fn push_str(&mut self, more: &str) {
        self.0.push_str(more);
    }
}

impl<B: Bound> Text<B> {
    /// Whether it is within its bound.
    pub fn fits(&self) -> bool {
        fits::<B>(self.0.len())
    }
}

impl<B> Deref for Text<B> {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl<B> AsRef<str> for Text<B> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<B> Clone for Text<B> {
    fn clone(&self) -> Self {
        Text(self.0.clone(), PhantomData)
    }
}

impl<B> Default for Text<B> {
    fn default() -> Self {
        Text(String::new(), PhantomData)
    }
}

impl<B> PartialEq for Text<B> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<B> Eq for Text<B> {}

impl<B> Hash for Text<B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<B> PartialEq<str> for Text<B> {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl<B> PartialEq<&str> for Text<B> {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl<B> PartialEq<String> for Text<B> {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

impl<B> fmt::Debug for Text<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<B> fmt::Display for Text<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<B> From<String> for Text<B> {
    fn from(text: String) -> Self {
        Text(text, PhantomData)
    }
}

impl<B> From<&str> for Text<B> {
    fn from(text: &str) -> Self {
        Text(text.to_string(), PhantomData)
    }
}

impl<B> Serialize for Text<B> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, B> Deserialize<'de> for Text<B> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Text::from)
    }
}

/// A list within the bound `B`, in entries.
pub struct List<T, B>(Vec<T>, PhantomData<fn() -> B>);

impl<T, B> List<T, B> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T, B: Bound> List<T, B> {
    /// Whether it is within its bound.
    pub fn fits(&self) -> bool {
        fits::<B>(self.0.len())
    }
}

impl<T, B> Deref for List<T, B> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T, B> DerefMut for List<T, B> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T: Clone, B> Clone for List<T, B> {
    fn clone(&self) -> Self {
        List(self.0.clone(), PhantomData)
    }
}

impl<T, B> Default for List<T, B> {
    fn default() -> Self {
        List(Vec::new(), PhantomData)
    }
}

impl<T: PartialEq, B> PartialEq for List<T, B> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq, B> Eq for List<T, B> {}

impl<T: PartialEq, B> PartialEq<Vec<T>> for List<T, B> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.0 == *other
    }
}

impl<T: PartialEq, B, const N: usize> PartialEq<[T; N]> for List<T, B> {
    fn eq(&self, other: &[T; N]) -> bool {
        self.0 == other
    }
}

impl<T: fmt::Debug, B> fmt::Debug for List<T, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<T, B> From<Vec<T>> for List<T, B> {
    fn from(list: Vec<T>) -> Self {
        List(list, PhantomData)
    }
}

impl<T, B> FromIterator<T> for List<T, B> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        List(iter.into_iter().collect(), PhantomData)
    }
}

impl<T, B> IntoIterator for List<T, B> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T, B> IntoIterator for &'a mut List<T, B> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

impl<'a, T, B> IntoIterator for &'a List<T, B> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Serialize, B> Serialize for List<T, B> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, B> Deserialize<'de> for List<T, B> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(d).map(List::from)
    }
}

/// Named JSON values (a piece's attributes, a typed item's fields), in the order given: at
/// most `C` of them, each name within `K` and each value within `V`.
pub struct Fields<C, K, V>(Map<String, Json>, Bounds<C, K, V>);

/// The bounds of a map: of its count, its names and its values.
type Bounds<C, K, V> = PhantomData<fn() -> (C, K, V)>;

impl<C, K, V> Fields<C, K, V> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_map(self) -> Map<String, Json> {
        self.0
    }
}

impl<C, K, V> Deref for Fields<C, K, V> {
    type Target = Map<String, Json>;

    fn deref(&self) -> &Map<String, Json> {
        &self.0
    }
}

impl<C, K, V> DerefMut for Fields<C, K, V> {
    fn deref_mut(&mut self) -> &mut Map<String, Json> {
        &mut self.0
    }
}

impl<C, K, V> Clone for Fields<C, K, V> {
    fn clone(&self) -> Self {
        Fields(self.0.clone(), PhantomData)
    }
}

impl<C, K, V> Default for Fields<C, K, V> {
    fn default() -> Self {
        Fields(Map::new(), PhantomData)
    }
}

impl<C, K, V> PartialEq for Fields<C, K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<C, K, V> fmt::Debug for Fields<C, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<C, K, V> From<Map<String, Json>> for Fields<C, K, V> {
    fn from(map: Map<String, Json>) -> Self {
        Fields(map, PhantomData)
    }
}

impl<'a, C, K, V> IntoIterator for &'a Fields<C, K, V> {
    type Item = (&'a String, &'a Json);
    type IntoIter = serde_json::map::Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<C, K, V> Serialize for Fields<C, K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, C, K, V> Deserialize<'de> for Fields<C, K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Map::<String, Json>::deserialize(d).map(Fields::from)
    }
}

/// Named texts (a step's options, a rebind's inputs), in the order given: at most `C` of
/// them, each name within `K` and each text within `V`.
pub struct Pairs<C, K, V>(IndexMap<String, String>, Bounds<C, K, V>);

impl<C, K, V> Pairs<C, K, V> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<C, K, V> Deref for Pairs<C, K, V> {
    type Target = IndexMap<String, String>;

    fn deref(&self) -> &IndexMap<String, String> {
        &self.0
    }
}

impl<C, K, V> DerefMut for Pairs<C, K, V> {
    fn deref_mut(&mut self) -> &mut IndexMap<String, String> {
        &mut self.0
    }
}

impl<C, K, V> Clone for Pairs<C, K, V> {
    fn clone(&self) -> Self {
        Pairs(self.0.clone(), PhantomData)
    }
}

impl<C, K, V> Default for Pairs<C, K, V> {
    fn default() -> Self {
        Pairs(IndexMap::new(), PhantomData)
    }
}

impl<C, K, V> PartialEq for Pairs<C, K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<C, K, V> fmt::Debug for Pairs<C, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<C, K, V> From<IndexMap<String, String>> for Pairs<C, K, V> {
    fn from(map: IndexMap<String, String>) -> Self {
        Pairs(map, PhantomData)
    }
}

impl<C, K, V> FromIterator<(String, String)> for Pairs<C, K, V> {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Pairs(iter.into_iter().collect(), PhantomData)
    }
}

impl<'a, C, K, V> IntoIterator for &'a Pairs<C, K, V> {
    type Item = (&'a String, &'a String);
    type IntoIter = indexmap::map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<C, K, V> Serialize for Pairs<C, K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, C, K, V> Deserialize<'de> for Pairs<C, K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        IndexMap::<String, String>::deserialize(d).map(Pairs::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The types are transparent: what they read and write is what a `String`, a `Vec` and
    /// a map read and wrote, byte for byte, the order of a map's names included.
    #[test]
    fn the_bounded_types_are_what_they_hold_on_the_wire() {
        let text: Text<Message> = serde_json::from_value(json!("a\u{1b}b")).unwrap();
        assert_eq!(serde_json::to_string(&text).unwrap(), "\"a\\u001bb\"");
        let list: List<Text<Place>, Runs> = serde_json::from_value(json!(["x", "y"])).unwrap();
        assert_eq!(serde_json::to_string(&list).unwrap(), r#"["x","y"]"#);
        let map = json!({"z": 1, "a": [true, null], "m": {"k": "v"}});
        let fields: Fields<Attributes, AttributeName, Message> =
            serde_json::from_value(map.clone()).unwrap();
        assert_eq!(
            serde_json::to_string(&fields).unwrap(),
            serde_json::to_string(&map).unwrap()
        );
        let pairs: Pairs<Message, Message, Message> =
            serde_json::from_value(json!({"to": "b", "from": "a"})).unwrap();
        assert_eq!(
            serde_json::to_string(&pairs).unwrap(),
            r#"{"to":"b","from":"a"}"#
        );
    }

    #[test]
    fn a_value_fits_its_bound_or_doesnt() {
        assert!(!Text::<Place>::from("").fits());
        assert!(Text::<Place>::from("p".repeat(1024)).fits());
        assert!(!Text::<Place>::from("p".repeat(1025)).fits());
        assert!(Text::<Message>::from("").fits());
        let runs: List<u8, Runs> = vec![0; 4097].into();
        assert!(!runs.fits());
    }
}
