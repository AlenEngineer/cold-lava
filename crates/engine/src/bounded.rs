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
//! not checked as it is built. It becomes an owner's answer, and an answer goes through
//! `given::read` and `Places` before any of it counts as items, so a piece out of bounds is
//! left out there like an owner's. An item edit is refused when its expected answer doesn't
//! read (`Core::edit_item`). A relink (a rename, a merge, a recipe) only compares its
//! expected answer with what the owner gave (`given::same`): a piece its change puts out of
//! bounds is left out of the owner's answer as it arrives, and is in the expected one's
//! pieces, so the two differ and the relink is refused; A4b says so in the bound's words.
//!
//! The bounds are the table below, one type each, so a number is written once and every
//! field that has it names it. Three kinds: a bound that applies today (`Place`, `Target`,
//! the names, `Attributes`, and with the answer `LinkLabel` and `Betweens`); `Message`,
//! where what bounds a field is where it came from, by decision; and `Owed`, a bound the
//! owner decided on 2026-11-13 that is not applied yet. The fields that name `Owed` are
//! what is left to do (#265, A4b), and that is done when none does. Every bound is checked
//! as a value is read from an answer (`Deserialize`, in the bound's words, `Limit::past`),
//! which leaves out the item it is in; the two an answer's reader applies with the whole
//! answer say so (`ON_ARRIVAL`). So nothing past a bound that applies reaches the core, its
//! cache or the read-back.
//!
//! Here too are the reader's words: what serde says that may be shown (`said`), the
//! problems an answer may have reported (`capped`), a value cut for a message (`cut`), and
//! what is kept of an item that was left out (`LeftOut`).
//!
//! A leaf: it needs serde, serde_json, indexmap and the engine's `Problem`. The types are
//! transparent on the wire, so what they serialize is what a `String`, a `Vec` or a map
//! serialized: an item's `rev` hashes that JSON, and it doesn't move.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json};

use crate::problem::Problem;

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
/// (the owner decided they are bounded only by the message), a Markdown piece's text, a
/// file's new text (`NewText`), what a region shows (an id and a label), a run's marks, a
/// piece's runs (their number is bounded with the piece, `given::MAX_RUNS`), and what names
/// a place (`in`, the ends of a link between pieces), which must name a piece taken before
/// it.
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

    fn past() -> String {
        Message::past()
    }
}

/// A piece's place: 1 to 1024 bytes (`Piece::check`; the piece is left out).
pub struct Place;
impl Limit for Place {
    const LEAST: usize = 1;
    const MOST: usize = 1024;

    fn past() -> String {
        format!("its place is empty or longer than {} bytes", Self::MOST)
    }
}

/// A link run's target: 1 to 4096 bytes (`Piece::check`; the piece is left out).
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

/// A field run's name: 1 to 256 bytes (`Piece::check`; the piece is left out).
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

/// What each bound says of a value past it, so the reader knows its own words in serde's
/// errors (`said`). Every bound is here: a test counts them.
const PAST: [fn() -> String; 11] = [
    Message::past,
    Owed::past,
    Place::past,
    Target::past,
    CellName::past,
    AttributeName::past,
    LabelName::past,
    FieldName::past,
    Attributes::past,
    LinkLabel::past,
    Betweens::past,
];

/// What a run of a piece's text is, said when one is neither.
pub const A_RUN: &str = "a run is text, or a map with a link, a label, a field or a text";

/// What an answer to a shape is, said when one is neither.
pub const A_REPLY: &str =
    "an answer to shape/region has items, and an answer to shape/gesture has do";

/// The reader's own words that reach it through serde: each says nothing an answer wrote.
const OWN: [&str; 2] = [A_RUN, A_REPLY];

/// The problems an answer may have reported: the first 100, then how many more.
pub const PROBLEMS: usize = 100;

/// `problems` with only the first `PROBLEMS` kept, and a last one that says how many more
/// there were.
pub fn capped(mut problems: Vec<Problem>) -> Vec<Problem> {
    if problems.len() > PROBLEMS {
        let more = problems.len() - PROBLEMS;
        problems.truncate(PROBLEMS);
        problems.push(Problem::line(format!("and {more} more")));
    }
    problems
}

/// The first `most` characters of what another program sent, for a message, with `…` where
/// it is cut: every value an answer gave that a message quotes is cut so.
pub fn cut(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// An item of an answer that didn't read, as it is kept: where it was (its place, cut), a
/// hash of what it said, to tell whether an owner gave it again unchanged, and what was
/// said of it. Nothing of it is the answer's but the place, cut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeftOut {
    pub place: String,
    pub key: String,
    pub said: String,
}

impl LeftOut {
    /// What is wrong with it, as reported.
    pub fn problem(&self) -> Problem {
        Problem::line(self.said.clone())
    }
}

/// Why something another program sent doesn't read, in words that say nothing but what it
/// wrote and what was expected: the reader's own (a bound's, `PAST`, or `OWN`), and serde's
/// where it is one of the forms that carry only what the answer wrote and a kind of value
/// (`says_only_the_answer`). `None` for anything else: a message of serde's names one of
/// cold-lava's Rust types in more ways than anyone can list (`untagged enum NAME`,
/// `expected struct NAME`, …), so the forms kept are the listed ones and a form nobody has
/// seen yet says nothing. serde_json writes where it was in a text (` at line N column M`),
/// which a value never has; it is left out when it is there.
pub fn said(e: &serde_json::Error) -> Option<String> {
    let whole = e.to_string();
    let at = format!(" at line {} column {}", e.line(), e.column());
    let message = match e.line() {
        0 => whole.as_str(),
        _ => whole.strip_suffix(&at).unwrap_or(&whole),
    };
    let ours = OWN.contains(&message) || PAST.iter().any(|past| past() == message);
    (ours || says_only_the_answer(message)).then(|| message.to_string())
}

/// Whether serde's `message` is one of the forms that carry only what the answer wrote (a
/// field's or a variant's name, a value) and what a value should have been. The shapes are
/// serde 1.0.229's own (`serde_core::de::Error`'s defaults and `OneOf`):
///
/// - `missing field `NAME``, `duplicate field `NAME``;
/// - `unknown field `NAME`, expected LIST`, `unknown field `NAME`, there are no fields`, and
///   the same for `unknown variant` (`there are no variants`), where LIST is serde's list of
///   the names the answer may use: `` `a` ``, `` `a` or `b` `` or `` one of `a`, `b`, `c` ``
///   (every enum read from an answer renames its variants for the wire, and the list is of
///   those names);
/// - `invalid type: …, expected KIND`, `invalid value: …, expected KIND`,
///   `invalid length N, expected KIND`, where KIND, what serde writes after the last
///   `, expected ` (after the answer's own value, which may hold those words), is a kind of
///   value (`is_a_kind`).
///
/// serde writes NAME, the answer's own, between backticks without escaping it, so NAME is
/// what stands between the first backtick and the next, and the form is kept only when what
/// follows it is exactly serde's own continuation: a name that holds a backtick ends early,
/// and the rest of it is not serde's.
fn says_only_the_answer(message: &str) -> bool {
    for form in ["missing field `", "duplicate field `"] {
        if let Some(rest) = message.strip_prefix(form) {
            return rest
                .split_once('`')
                .is_some_and(|(_, after)| after.is_empty());
        }
    }
    for (form, none) in [
        ("unknown field `", ", there are no fields"),
        ("unknown variant `", ", there are no variants"),
    ] {
        if let Some(rest) = message.strip_prefix(form) {
            let Some((_, after)) = rest.split_once('`') else {
                return false;
            };
            return after == none
                || after
                    .strip_prefix(", expected ")
                    .is_some_and(is_serdes_list);
        }
    }
    let valued = ["invalid type: ", "invalid value: ", "invalid length "];
    if !valued.iter().any(|form| message.starts_with(form)) {
        return false;
    }
    message
        .rsplit_once(", expected ")
        .is_some_and(|(_, kind)| is_a_kind(kind))
}

/// Whether `list` is serde's list of names (`OneOf`): `` `a` ``, `` `a` or `b` ``, or
/// `` one of `a`, `b`, `c` ``, each name between backticks and holding none.
fn is_serdes_list(list: &str) -> bool {
    let one = |name: &str| {
        let inner = name.strip_prefix('`').and_then(|n| n.strip_suffix('`'));
        inner.is_some_and(|n| !n.is_empty() && !n.contains('`'))
    };
    match list.strip_prefix("one of ") {
        Some(names) => names.split(", ").count() > 2 && names.split(", ").all(one),
        None => match list.split_once(" or ") {
            Some((a, b)) => one(a) && one(b),
            None => one(list),
        },
    }
}

/// Whether serde's `expected` part is a kind of value, not a Rust item: made only of the words
/// serde and serde_json write for kinds of value in what an answer is read into (serde's
/// `de::impls` and `Unexpected`, serde_json's value deserializer: `a string`, `a map`, `a
/// sequence`, `an array of length 2`, `string or map`, `map with a single key`, `option`,
/// `unit`), and a number's type (`u8`, `f64`). A word it doesn't know, a struct, an enum or a
/// variant reads "as one"; so does serde's jargon (`variant identifier`), which names nothing
/// but helps no one.
fn is_a_kind(kind: &str) -> bool {
    const NUMBERS: [&str; 14] = [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64",
    ];
    const WORDS: [&str; 24] = [
        "a",
        "an",
        "or",
        "of",
        "with",
        "string",
        "map",
        "sequence",
        "array",
        "boolean",
        "character",
        "byte",
        "borrowed",
        "empty",
        "single",
        "key",
        "unit",
        "option",
        "integer",
        "number",
        "null",
        "length",
        "elements",
        "element",
    ];
    let word = |w: &str| WORDS.contains(&w) || w.chars().all(|c| c.is_ascii_digit());
    NUMBERS.contains(&kind) || (!kind.is_empty() && kind.split(' ').all(word))
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
    /// `said` knows; a bound the answer's reader applies is not.
    #[test]
    fn a_value_past_its_bound_is_refused_as_it_is_read() {
        let refused = |value: Json| -> Option<String> {
            let e = serde_json::from_value::<Text<Place>>(value).err()?;
            said(&e)
        };
        assert_eq!(refused(json!("p")), None);
        let past = Some("its place is empty or longer than 1024 bytes".to_string());
        assert_eq!(refused(json!("")), past);
        assert_eq!(refused(json!("p".repeat(1025))), past);
        let many = serde_json::from_value::<List<u8, Attributes>>(json!(vec![0; 257]));
        assert_eq!(
            said(&many.unwrap_err()).unwrap(),
            "it has more than 256 attributes"
        );
        let mut names = Map::new();
        names.insert("x".repeat(257), json!(1));
        let names = Json::Object(names);
        let fields = serde_json::from_value::<Fields<Attributes, AttributeName, Message>>(names);
        assert_eq!(
            said(&fields.unwrap_err()).unwrap(),
            "an attribute's name is empty or longer than 256 bytes"
        );
        let long = json!("l".repeat(4097));
        assert!(serde_json::from_value::<Text<LinkLabel>>(long).is_ok());
    }

    /// Every bound says what it says of a value past it, so the reader knows its words.
    #[test]
    fn every_bound_is_in_the_table_of_what_bounds_say() {
        let source = include_str!("bounded.rs");
        // Spelled in two parts, so this line is not one of them.
        let bounds = source.matches(&["impl", " Limit for "].concat()).count();
        assert_eq!(bounds, PAST.len());
    }

    /// Only the reader's own words and serde's forms that say what the answer wrote are
    /// shown: a message that names a Rust type is not.
    #[test]
    fn what_serde_says_is_shown_only_when_it_says_what_the_answer_wrote() {
        use serde::de::Error as _;
        let custom = |m: &str| said(&serde_json::Error::custom(m));
        assert_eq!(custom(A_RUN).as_deref(), Some(A_RUN));
        assert_eq!(custom(A_REPLY).as_deref(), Some(A_REPLY));
        assert_eq!(
            custom("data did not match any variant of untagged enum Run"),
            None
        );
        let unknown = serde_json::Error::unknown_field("colour", &["place", "text"]);
        assert_eq!(
            said(&unknown).as_deref(),
            Some("unknown field `colour`, expected `place` or `text`")
        );
    }

    /// The first 100 problems are kept, then how many more there were.
    #[test]
    fn problems_are_capped_at_a_hundred() {
        let some = |n: usize| (0..n).map(|i| Problem::line(format!("p{i}"))).collect();
        assert_eq!(capped(some(100)).len(), 100);
        let many = capped(some(50_000));
        assert_eq!(many.len(), 101);
        assert_eq!(many[99].one_line(), "p99");
        assert_eq!(many[100].one_line(), "and 49900 more");
    }
}
