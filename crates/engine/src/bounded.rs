//! What another program sends, held in fields that each name their bound: an owner's answers
//! to `file/read` and `file/edit` (`owner::given`, `owner::edits`) and an extension's answers
//! to `shape/region` and `shape/gesture` (`act::Reply`, the steps of `shapes`). Each field of
//! those types is a `Text`, a `List`, `Fields` or `Pairs` whose last parameters name its
//! bound: `place: Text<Place>` is a place whose bound is 1 to 1024 bytes.
//! `crates/cold-lava/tests/one_door.rs` fails on a field of those types that names no bound.
//!
//! **The type names the bound; it doesn't hold it.** A value read from an answer is
//! checked as it is read (`Deserialize`), but nothing is checked when one is built: `From`
//! and `Default` take any value, and `DerefMut` hands out what is inside. A reader applies a
//! bound to a value it didn't read by asking the field (`fits`, `most`, `past`), as
//! `Piece::check` does. A bound is a check on what arrives, never an invariant of the type.
//! What the engine makes itself (`given::apply` from a change, the canvas owner's answer) is
//! not checked as it is built. It becomes an owner's answer, and every answer goes through
//! `given::Places` before any of it counts as items (`given::read`), which asks every bound
//! of an item again (`GivenItem::check`): so one rule holds for every form and every owner,
//! the canvas owner's included, which asks `Places` itself and gives nothing it would leave
//! out. An item edit is refused when its expected answer doesn't read (`Core::edit_item`),
//! and a relink (a rename, a merge, a recipe) when its change would leave out what the owner
//! gave (`Core::relink`), in the bound's words, before the owner is asked.
//!
//! The bounds are the table below, one type each, so a number is written once and every
//! field that has it names it. Two kinds: a bound of its own, decided by the owner
//! (2026-11-13), and `Message`, where what bounds a field is where it came from, by decision.
//! No other bound has `Message`'s number (a test says so), so a bound decided and not applied
//! can't hide behind it. Every bound is checked as a value is read from an answer
//! (`Deserialize`, in the bound's words, `Limit::past`), which leaves out the item it is in;
//! the ones an answer's reader applies with the whole answer say so (`ON_ARRIVAL`,
//! `WithAnswer`), since what they leave out is a link and not an item. A total across fields
//! (a piece's text, its runs, its depth) is a relation, checked with the piece.
//!
//! A leaf: it needs serde, serde_json and indexmap. The types are transparent on the wire,
//! so what they serialize is what a `String`, a `Vec` or a map serialized: an item's `rev`
//! hashes that JSON, and it doesn't move. The reader's words (which of serde's may be shown,
//! the cap on problems, what is kept of an item left out) are the reader's
//! (`owner::answer`).

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
    /// Whether a value past it is refused as it is read, which leaves out the item it is in
    /// (or, outside an item, refuses the answer). Else the reader of the whole answer applies
    /// it, since what is left out there is named by what is around it.
    const ON_ARRIVAL: bool = true;
    /// What is said of a value past it. It holds no byte of the value.
    fn past() -> String;
}

/// What bounds a field is where it came from: 64 MiB. An extension's answer is
/// one message, so the host's frames bound it (`frame::MAX_MESSAGE`). The other ways these
/// types are filled are bounded by what they come from: the canvas owner's answer by the
/// canvas file it reads, the owners' cache by the answers the engine kept in it, a shape's
/// steps written in TOML by that file. The fields that name it: an answer's items and links
/// (the owner decided they are bounded only by the message), a file's new text (`NewText`), what a region shows (an id and a label), a run's marks, a
/// piece's runs (their number is bounded with the piece, `given::MAX_RUNS`), what names a
/// place (`in`, the ends of a link between pieces), which must name a piece taken before it,
/// and the number of a step's options and a rebind's inputs.
pub struct Message;
impl Limit for Message {
    const MOST: usize = 64 << 20;

    fn past() -> String {
        format!(
            "it holds more than the {} MiB a message may",
            Self::MOST >> 20
        )
    }
}

/// The place of an item in every form (a piece, a Markdown piece, a typed item) and of a link
/// to a file: 1 to 1024 bytes (`GivenItem::check`; the item is left out). That one item of a
/// form stands at a place is a relation of the answer's items, `given::Places`.
pub struct Place;
impl Limit for Place {
    const LEAST: usize = 1;
    const MOST: usize = 1024;

    fn past() -> String {
        format!("its place is empty or longer than {} bytes", Self::MOST)
    }
}

/// A link's target, a link run's and a link to a file's: 1 to 4096 bytes (`Piece::check`;
/// the piece is left out; `Places::link`).
pub struct Target;
impl Limit for Target {
    const LEAST: usize = 1;
    const MOST: usize = 4096;

    fn past() -> String {
        format!(
            "a link's target is empty or longer than {} bytes",
            Self::MOST
        )
    }
}

/// A cell's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct CellName;
impl Limit for CellName {
    const LEAST: usize = 1;
    const MOST: usize = 256;

    fn past() -> String {
        format!("a cell's name is empty or longer than {} bytes", Self::MOST)
    }
}

/// An attribute's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct AttributeName;
impl Limit for AttributeName {
    const LEAST: usize = 1;
    const MOST: usize = 256;

    fn past() -> String {
        format!(
            "an attribute's name is empty or longer than {} bytes",
            Self::MOST
        )
    }
}

/// A label's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
pub struct LabelName;
impl Limit for LabelName {
    const LEAST: usize = 1;
    const MOST: usize = 256;

    fn past() -> String {
        format!(
            "a label's name is empty or longer than {} bytes",
            Self::MOST
        )
    }
}

/// A field's name, a field run's and a typed item's: 1 to 256 bytes (`GivenItem::check`; the
/// item is left out).
pub struct FieldName;
impl Limit for FieldName {
    const LEAST: usize = 1;
    const MOST: usize = 256;

    fn past() -> String {
        format!(
            "a field's name is empty or longer than {} bytes",
            Self::MOST
        )
    }
}

/// A piece's attributes: at most 256 (`Piece::check`; the piece is left out).
pub struct Attributes;
impl Limit for Attributes {
    const MOST: usize = 256;

    fn past() -> String {
        format!("it has more than {} attributes", Self::MOST)
    }
}

/// The label of a link between two pieces: at most 4096 bytes. Applied with the answer
/// (`given::Places::joins`), since the link is left out alone and named by its two ends.
pub struct LinkLabel;
impl Limit for LinkLabel {
    const MOST: usize = 4096;
    const ON_ARRIVAL: bool = false;

    fn past() -> String {
        format!("its label is longer than {} bytes", Self::MOST)
    }
}

/// The links between pieces of an answer: at most 65,536. Applied with the answer
/// (`given::Places::joins`): the rest are left out, and that is said once.
pub struct Betweens;
impl Limit for Betweens {
    const MOST: usize = 65_536;
    const ON_ARRIVAL: bool = false;

    fn past() -> String {
        format!(
            "more than {} links between pieces: the rest are left out",
            Self::MOST
        )
    }
}

/// A piece's text: at most 1 MiB, a run's on its own (the piece is left out) and all its runs
/// together, its cells' included (`Piece::check`): a plain or marked run's characters, a
/// link's words and a field's value. A Markdown piece is a piece: its text has the same bound
/// (`GivenItem::check`).
pub struct PieceText;
impl Limit for PieceText {
    const MOST: usize = 1 << 20;

    fn past() -> String {
        format!("its text is longer than {} MiB", Self::MOST >> 20)
    }
}

/// A value that is text: a piece's attribute, a field run's value, a typed item's field or
/// one of a list it holds. At most 1 MiB (`GivenItem::check`; the item is left out).
pub struct ValueText;
impl Limit for ValueText {
    const MOST: usize = 1 << 20;

    fn past() -> String {
        format!("a value is longer than {} MiB", Self::MOST >> 20)
    }
}

/// A link's words: a link run's, and what a link to a file says around it. At most 4096
/// bytes (`Piece::check`; `Places::link`).
pub struct LinkText;
impl Limit for LinkText {
    const MOST: usize = 4096;

    fn past() -> String {
        format!("a link's text is longer than {} bytes", Self::MOST)
    }
}

/// A row's cells: at most 4096 (`Piece::check`; the piece is left out).
pub struct Cells;
impl Limit for Cells {
    const MOST: usize = 4096;

    fn past() -> String {
        format!("it has more than {} cells", Self::MOST)
    }
}

/// A typed item's type: 1 to 256 bytes (`GivenItem::check`; the item is left out).
pub struct TypeName;
impl Limit for TypeName {
    const LEAST: usize = 1;
    const MOST: usize = 256;

    fn past() -> String {
        format!("its type is empty or longer than {} bytes", Self::MOST)
    }
}

/// A typed item's fields: at most 256 (`GivenItem::check`; the item is left out).
pub struct TypedFields;
impl Limit for TypedFields {
    const MOST: usize = 256;

    fn past() -> String {
        format!("it has more than {} fields", Self::MOST)
    }
}

/// A list a typed item's field holds: at most 4096 values, each text, a number, `true`,
/// `false` or `null` (`GivenItem::check`; the item is left out).
pub struct ValueList;
impl Limit for ValueList {
    const MOST: usize = 4096;

    fn past() -> String {
        format!("a field's list has more than {} values", Self::MOST)
    }
}

/// What a region shows, in an answer to `shape/region`: at most 50,000 items (the answer is
/// refused).
pub struct RegionItems;
impl Limit for RegionItems {
    const MOST: usize = 50_000;

    fn past() -> String {
        format!("it shows more than {} items", Self::MOST)
    }
}

/// The steps of an answer to `shape/gesture`: at most 64 (the answer is refused).
pub struct GestureSteps;
impl Limit for GestureSteps {
    const MOST: usize = 64;

    fn past() -> String {
        format!("it has more than {} steps", Self::MOST)
    }
}

/// A name in a step (an input, a recipe, a field, a shape, an option's or an input's name):
/// at most 256 bytes. A shape's steps written in TOML are read into the same types.
pub struct StepName;
impl Limit for StepName {
    const MOST: usize = 256;

    fn past() -> String {
        format!("a name in a step is longer than {} bytes", Self::MOST)
    }
}

/// A value in a step (a field's value, a new note's text and name, an option's or an input's
/// value): at most 64 KiB.
pub struct StepValue;
impl Limit for StepValue {
    const MOST: usize = 64 << 10;

    fn past() -> String {
        format!("a value in a step is longer than {} KiB", Self::MOST >> 10)
    }
}

/// `B`, applied as the answer is read into items rather than as it arrives
/// (`Limit::ON_ARRIVAL`): a link to a file's bounds (`given::Places::link`). An item past a
/// bound is left out alone as it arrives, but a link that doesn't read as it arrives
/// refuses the whole answer, so a link past one is left out alone where the answer is read.
///
/// Why two ways to say "applied with the answer": `LinkLabel` and `Betweens` are bounds of
/// their own, with their own words, that no field applies on arrival, so their `ON_ARRIVAL`
/// says it. A link to a file's place, target and words are bounds other fields apply on
/// arrival (`Place`, `Target`, `LinkText`): `WithAnswer` names the same number and words
/// once, instead of a second type with a copy of them. The fields that name it are listed by
/// `tests/one_door.rs`, so a new one is a reviewed decision.
pub struct WithAnswer<B>(PhantomData<fn() -> B>);
impl<B: Limit> Limit for WithAnswer<B> {
    const LEAST: usize = B::LEAST;
    const MOST: usize = B::MOST;
    const ON_ARRIVAL: bool = false;

    fn past() -> String {
        B::past()
    }
}

/// Whether `n` is within `B`.
pub fn fits<B: Limit>(n: usize) -> bool {
    (B::LEAST..=B::MOST).contains(&n)
}

/// `n` as it is read: an error in `B`'s words when `B` applies on arrival and `n` is past it.
fn within<B: Limit, E: serde::de::Error>(n: usize) -> Result<(), E> {
    match B::ON_ARRIVAL && !fits::<B>(n) {
        true => Err(E::custom(B::past())),
        false => Ok(()),
    }
}

/// Each bound's most and what it says of a value past it. Every bound is here: a test counts
/// them, and another checks that no bound but `Message` has `Message`'s number.
const PAST: [(usize, fn() -> String); 21] = [
    (Message::MOST, Message::past),
    (Place::MOST, Place::past),
    (Target::MOST, Target::past),
    (CellName::MOST, CellName::past),
    (AttributeName::MOST, AttributeName::past),
    (LabelName::MOST, LabelName::past),
    (FieldName::MOST, FieldName::past),
    (Attributes::MOST, Attributes::past),
    (LinkLabel::MOST, LinkLabel::past),
    (Betweens::MOST, Betweens::past),
    (PieceText::MOST, PieceText::past),
    (ValueText::MOST, ValueText::past),
    (LinkText::MOST, LinkText::past),
    (Cells::MOST, Cells::past),
    (TypeName::MOST, TypeName::past),
    (TypedFields::MOST, TypedFields::past),
    (ValueList::MOST, ValueList::past),
    (RegionItems::MOST, RegionItems::past),
    (GestureSteps::MOST, GestureSteps::past),
    (StepName::MOST, StepName::past),
    (StepValue::MOST, StepValue::past),
];

/// Whether `message` is what a bound says of a value past it (`Limit::past`): words of the
/// engine's own, which hold nothing an answer wrote.
pub fn is_past(message: &str) -> bool {
    PAST.iter().any(|(_, past)| past() == message)
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

    /// What its bound says of a value past it.
    pub fn past(&self) -> String {
        B::past()
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

/// Refused past its bound (`Limit::ON_ARRIVAL`), in the bound's words.
impl<'de, B: Limit> Deserialize<'de> for Text<B> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        within::<B, D::Error>(text.len())?;
        Ok(Text::from(text))
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

    /// What its bound says of a list past it.
    pub fn past(&self) -> String {
        B::past()
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

/// Refused past its bound (`Limit::ON_ARRIVAL`), in the bound's words.
impl<'de, T: Deserialize<'de>, B: Limit> Deserialize<'de> for List<T, B> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let list = Vec::<T>::deserialize(d)?;
        within::<B, D::Error>(list.len())?;
        Ok(List::from(list))
    }
}

/// Named JSON values (a piece's attributes, a typed item's fields), in the order given: at
/// most `C` of them, each name within `K` and each value that is text within `V`. All three
/// are checked as an answer is read (`Deserialize`); a reader of a value it didn't read
/// asks `most` and `name_fits`.
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

    /// What `C` says of too many names.
    pub fn past(&self) -> String {
        C::past()
    }

    /// What `K` says of a name past it.
    pub fn name_past(&self) -> String {
        K::past()
    }

    /// Whether `value` is within `V`: text by its bytes; any other value is.
    pub fn value_fits(&self, value: &Json) -> bool {
        match value {
            Json::String(text) => fits::<V>(text.len()),
            _ => true,
        }
    }

    /// What `V` says of a value past it.
    pub fn value_past(&self) -> String {
        V::past()
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

/// Refused past one of its bounds (`Limit::ON_ARRIVAL`), in the bound's words: its count,
/// a name, or a value that is text.
impl<'de, C: Limit, K: Limit, V: Limit> Deserialize<'de> for Fields<C, K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = Map::<String, Json>::deserialize(d)?;
        within::<C, D::Error>(map.len())?;
        for (name, value) in &map {
            within::<K, D::Error>(name.len())?;
            if let Json::String(text) = value {
                within::<V, D::Error>(text.len())?;
            }
        }
        Ok(Fields::from(map))
    }
}

/// Named texts (a step's options, a rebind's inputs), in the order given: at most `C` of
/// them, each name within `K` and each text within `V`, checked as an answer is read
/// (`Deserialize`).
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

/// Refused past one of its bounds (`Limit::ON_ARRIVAL`), in the bound's words: its count, a
/// name or a text.
impl<'de, C: Limit, K: Limit, V: Limit> Deserialize<'de> for Pairs<C, K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = IndexMap::<String, String>::deserialize(d)?;
        within::<C, D::Error>(map.len())?;
        for (name, text) in &map {
            within::<K, D::Error>(name.len())?;
            within::<V, D::Error>(text.len())?;
        }
        Ok(Pairs::from(map))
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

    /// A value past its bound is refused as it is read, in the bound's own words, which
    /// `is_past` knows; a bound the answer's reader applies is not.
    #[test]
    fn a_value_past_its_bound_is_refused_as_it_is_read() {
        let refused = |value: Json| -> Option<String> {
            let e = serde_json::from_value::<Text<Place>>(value).err()?;
            Some(e.to_string())
        };
        assert_eq!(refused(json!("p")), None);
        let past = Some("its place is empty or longer than 1024 bytes".to_string());
        assert_eq!(refused(json!("")), past);
        assert_eq!(refused(json!("p".repeat(1025))), past);
        assert!(is_past(past.as_deref().unwrap()));
        let many = serde_json::from_value::<List<u8, Attributes>>(json!(vec![0; 257]));
        assert_eq!(
            many.unwrap_err().to_string(),
            "it has more than 256 attributes"
        );
        let mut names = Map::new();
        names.insert("x".repeat(257), json!(1));
        let names = Json::Object(names);
        let fields = serde_json::from_value::<Fields<Attributes, AttributeName, Message>>(names);
        assert_eq!(
            fields.unwrap_err().to_string(),
            "an attribute's name is empty or longer than 256 bytes"
        );
        let long = json!("l".repeat(4097));
        assert!(serde_json::from_value::<Text<LinkLabel>>(long.clone()).is_ok());
        let later = serde_json::from_value::<Text<WithAnswer<Target>>>(long).unwrap();
        assert!(!later.fits());
        assert_eq!(later.past(), Target::past());
        assert!(!is_past("its place is empty"));
    }

    /// Every bound says what it says of a value past it, so `is_past` knows its words.
    #[test]
    fn every_bound_is_in_the_table_of_what_bounds_say() {
        let source = include_str!("bounded.rs");
        // Spelled in two parts, so this line is not one of them.
        let bounds = source.matches(&["impl", " Limit for "].concat()).count();
        assert_eq!(bounds, PAST.len());
    }

    /// A bound is `Message` or one of its own: a bound that borrows `Message`'s number is
    /// a bound decided and not applied, which A4b (#265) ended.
    #[test]
    fn no_bound_but_message_has_its_number() {
        let message = PAST.iter().filter(|(most, _)| *most == Message::MOST);
        assert_eq!(message.count(), 1);
    }
}
