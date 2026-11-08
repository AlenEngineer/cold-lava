//! What a typed item carries per field, kept small: a workspace can hold tens of thousands of
//! typed items, and the schema's names, the same for every item of a type, are shared.

use std::fmt;
use std::ops::{Index, Range};
use std::sync::Arc;

use serde_json::Value as Json;

/// A field's name, shared by every item of its type.
pub type Name = Arc<str>;

/// A type's fields in its schema's order, and whether each holds a list.
pub type Schema = Arc<[(Name, bool)]>;

static NULL: Json = Json::Null;
static NO_VALUES: Json = Json::Array(Vec::new());

/// Every field of the schema, in its order: null (or [] for a list) when absent. Only the
/// values an item has are stored.
#[derive(Clone)]
pub struct Fields {
    schema: Schema,
    /// By field index, in order; never a field's empty value.
    values: Vec<(usize, Json)>,
}

impl Default for Fields {
    fn default() -> Fields {
        Fields::new(Arc::from(Vec::new()))
    }
}

impl Fields {
    /// Every field of `schema`, empty.
    pub fn new(schema: Schema) -> Fields {
        Fields {
            schema,
            values: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.schema.len()
    }

    pub fn is_empty(&self) -> bool {
        self.schema.is_empty()
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.schema.iter().position(|(n, _)| &**n == name)
    }

    fn at(&self, i: usize) -> &Json {
        match self.values.binary_search_by_key(&i, |v| v.0) {
            Ok(k) => &self.values[k].1,
            Err(_) if self.schema[i].1 => &NO_VALUES,
            Err(_) => &NULL,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Json> {
        Some(self.at(self.position(name)?))
    }

    /// Sets the schema's field `i`.
    pub fn set(&mut self, i: usize, value: Json) {
        let empty = value.is_null() || (self.schema[i].1 && value == NO_VALUES);
        match self.values.binary_search_by_key(&i, |v| v.0) {
            Ok(k) if empty => {
                self.values.remove(k);
            }
            Ok(k) => self.values[k].1 = value,
            Err(_) if empty => {}
            Err(k) => self.values.insert(k, (i, value)),
        }
    }

    /// Sets the field `name`, adding it after the schema's when it has none of that name.
    pub fn insert(&mut self, name: &str, value: Json) {
        let i = self.position(name).unwrap_or_else(|| {
            let mut schema = self.schema.to_vec();
            schema.push((Name::from(name), false));
            self.schema = schema.into();
            self.schema.len() - 1
        });
        self.set(i, value);
    }

    pub fn shrink_to_fit(&mut self) {
        self.values.shrink_to_fit();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Json)> {
        (0..self.schema.len()).map(|i| (&self.schema[i].0, self.at(i)))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Name> {
        self.schema.iter().map(|(n, _)| n)
    }

    pub fn values(&self) -> impl Iterator<Item = &Json> {
        (0..self.schema.len()).map(|i| self.at(i))
    }
}

impl Index<usize> for Fields {
    type Output = Json;
    fn index(&self, i: usize) -> &Json {
        self.at(i)
    }
}

impl Index<&str> for Fields {
    type Output = Json;
    fn index(&self, name: &str) -> &Json {
        self.get(name).expect("no field of that name")
    }
}

impl<'a> IntoIterator for &'a Fields {
    type Item = (&'a Name, &'a Json);
    type IntoIter = Box<dyn Iterator<Item = (&'a Name, &'a Json)> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// As maps: the same names with the same values, in any order.
impl PartialEq for Fields {
    fn eq(&self, other: &Fields) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl fmt::Debug for Fields {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Where each field's values were read, in the note's bytes: one range per value.
#[derive(Clone, Default)]
pub struct Spans {
    ranges: Vec<Range<usize>>,
    /// Each field's ranges in `ranges`.
    fields: Vec<(Name, u32, u32)>,
}

impl Spans {
    /// Sets a field's ranges.
    pub fn insert(&mut self, name: Name, ranges: impl IntoIterator<Item = Range<usize>>) {
        let start = self.ranges.len() as u32;
        self.ranges.extend(ranges);
        let end = self.ranges.len() as u32;
        match self.fields.iter_mut().find(|f| f.0 == name) {
            Some(f) => (f.1, f.2) = (start, end),
            None => self.fields.push((name, start, end)),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.ranges.shrink_to_fit();
        self.fields.shrink_to_fit();
    }

    pub fn get(&self, name: &str) -> Option<&[Range<usize>]> {
        let (_, start, end) = self.fields.iter().find(|f| &*f.0 == name)?;
        Some(&self.ranges[*start as usize..*end as usize])
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Name, &[Range<usize>])> {
        (self.fields.iter())
            .map(|(name, start, end)| (name, &self.ranges[*start as usize..*end as usize]))
    }
}

/// As maps: the same names with the same ranges, in any order.
impl PartialEq for Spans {
    fn eq(&self, other: &Spans) -> bool {
        self.fields.len() == other.fields.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl fmt::Debug for Spans {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}
