//! What another program sends, held in fields that each name their bound: an owner's answers
//! to `file/read` and `file/edit` (`owner::given`, `owner::edits`) and an extension's answers
//! to `shape/region` and `shape/gesture` (`act::Reply`, the steps of `shapes`). Each field of
//! those types is a `Text`, a `List`, `Fields` or `Pairs` whose last parameters name its
//! bound: `place: Text<Place>` is a place whose bound is 1 to 1024 bytes.
//! `crates/cold-lava/tests/one_door.rs` fails on a field of those types that names no bound.
//!
//! **The type names the bound; it doesn't hold it.** Nothing is checked when one of these
//! values is built or read: `From`, `Default` and `Deserialize` take any value, and
//! `DerefMut` hands out what is inside. A reader applies a bound by asking the field
//! (`fits`, `most`): `Piece::check` and `Places::joins` do, for the bounds that apply today.
//! A bound is a check on what arrives, never an invariant of the type. What the engine makes
//! itself (`given::apply` from a change, the canvas owner's answer) is not checked as it is
//! built. It becomes an owner's answer, and an answer goes through `given::read` and
//! `Places` before any of it counts as items, so a piece out of bounds is left out there
//! like an owner's. An item edit is refused when its expected answer doesn't read
//! (`Core::edit_item`); a relink (a rename, a merge, a recipe) only compares its expected
//! answer with what the owner gave (`given::same`), so a piece its change puts out of
//! bounds drops out of both and is not refused (#265, A4b).
//!
//! The bounds are the table below, one type each, so a number is written once and every
//! field that has it names it. Three kinds: a bound that applies today (`Place`, `Target`,
//! the names, `Attributes`, and with the answer `LinkLabel` and `Betweens`); `Message`,
//! where what bounds a field is where it came from, by decision; and `Owed`, a bound the
//! owner decided on 2026-11-13 that is not applied yet. The fields that name `Owed` are
//! what is left to do (#265, A4b), and that is done when none does.
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
/// a list or a map. (`bind::Bound` is a placed shape's bound input, another thing.)
pub trait Limit {
    const LEAST: usize = 0;
    const MOST: usize;
}

/// What bounds a field is where it came from: 64 MiB. An extension's answer is
/// one message, so the host's frames bound it (`frame::MAX_MESSAGE`). The other ways these
/// types are filled are bounded by what they come from: the canvas owner's answer by the
/// canvas file it reads, the owners' cache by the answers the engine kept in it, a shape's
/// steps written in TOML by that file. The fields that name it: an answer's items and links
/// (the owner decided they are bounded only by the message), a Markdown piece's text, a
/// file's new text (`NewText`), what a region shows (an id and a label), a run's marks, a
/// piece's runs (their number is bounded with the piece, `given::MAX_RUNS`), and what names
/// a place (`in`, the ends of a link between pieces), which must name a piece taken before
/// it.
pub struct Message;
impl Limit for Message {
    const MOST: usize = 64 << 20;
}

/// A bound the owner decided on 2026-11-13 and that is not applied yet: until #265's A4b
/// applies it, only `Message` bounds the field. The fields that name it are A4b's work
/// list: a place in every form (1 to 1024 bytes, unique), a piece's text (1 MiB in total), a
/// string attribute (1 MiB), a link's words (4096 bytes), a row's cells (4096), a region's
/// answer (50,000 items), a gesture's answer (64 steps, names up to 256 bytes and values up to
/// 64 KiB). Also, from the proposal the owner accepted ("generous, as proposed"), numbers
/// the owner confirms before A4b merges: a file link's target (1 to 4096 bytes, as a link
/// run's) and the words around it (4096 bytes), a typed item's type and field names (256
/// bytes), its fields (256), and their values (a scalar, or a list of up to 4096).
///
/// A4b applies each where every answer passes, `given::read` and `Places`, not only as an
/// answer arrives: the canvas owner's answer and what `given::apply` makes never arrive, and
/// the rule is one for every form, the built-in canvas owner's included.
pub struct Owed;
impl Limit for Owed {
    const MOST: usize = Message::MOST;
}

/// A piece's place: 1 to 1024 bytes (`Piece::check`; the piece is left out).
pub struct Place;
impl Limit for Place {
    const LEAST: usize = 1;
    const MOST: usize = 1024;
}

/// A link run's target: 1 to 4096 bytes (`Piece::check`; the piece is left out).
pub struct Target;
impl Limit for Target {
    const LEAST: usize = 1;
    const MOST: usize = 4096;
}

/// A cell's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct CellName;
impl Limit for CellName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// An attribute's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct AttributeName;
impl Limit for AttributeName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// A label's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct LabelName;
impl Limit for LabelName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// A field run's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct FieldName;
impl Limit for FieldName {
    const LEAST: usize = 1;
    const MOST: usize = 256;
}

/// A piece's attributes: at most 256 (`Piece::check`; the piece is left out).
pub struct Attributes;
impl Limit for Attributes {
    const MOST: usize = 256;
}

/// The label of a link between two pieces: at most 4096 bytes. Applied with the answer
/// (`given::Places::joins`), since the link is left out alone and named by its two ends.
pub struct LinkLabel;
impl Limit for LinkLabel {
    const MOST: usize = 4096;
}

/// The links between pieces of an answer: at most 65,536. Applied with the answer
/// (`given::Places::joins`): the rest are left out, and that is said once.
pub struct Betweens;
impl Limit for Betweens {
    const MOST: usize = 65_536;
}

/// Whether `n` is within `B`.
pub fn fits<B: Limit>(n: usize) -> bool {
    (B::LEAST..=B::MOST).contains(&n)
}

/// A text whose bound, in bytes, is `B`. It doesn't hold the bound: `fits` applies it.
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

impl<B: Limit> Text<B> {
    /// Whether it is within its bound.
    pub fn fits(&self) -> bool {
        fits::<B>(self.0.len())
    }

    /// The most bytes its bound allows.
    pub fn most(&self) -> usize {
        B::MOST
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

/// A list whose bound, in entries, is `B`. It doesn't hold the bound: `fits` applies it.
pub struct List<T, B>(Vec<T>, PhantomData<fn() -> B>);

impl<T, B> List<T, B> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T, B: Limit> List<T, B> {
    /// Whether it is within its bound.
    pub fn fits(&self) -> bool {
        fits::<B>(self.0.len())
    }

    /// The most entries its bound allows.
    pub fn most(&self) -> usize {
        B::MOST
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
/// most `C` of them, each name within `K` and each value within `V`. A reader asks `most`
/// and `name_fits`; nothing reads `V` yet.
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

impl<C: Limit, K: Limit, V: Limit> Fields<C, K, V> {
    /// The most names `C` allows.
    pub fn most(&self) -> usize {
        C::MOST
    }

    /// Whether `name` is within `K`, and the most bytes `K` allows.
    pub fn name_fits(&self, name: &str) -> (bool, usize) {
        (fits::<K>(name.len()), K::MOST)
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
/// them, each name within `K` and each text within `V`. Nothing applies them yet: they are
/// owed (`Owed`).
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
        let list: List<Text<Place>, Message> = serde_json::from_value(json!(["x", "y"])).unwrap();
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
        let many: List<u8, Attributes> = vec![0; 257].into();
        assert!(!many.fits());
    }
}
