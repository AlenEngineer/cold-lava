//! What a region shows: an expression over a placed shape's inputs (docs/shapes.md, "Bindings,
//! as built"). `a & b` is what both inputs have, `a | b` what either has, `a - b` what `a` has
//! and `b` doesn't, matched on the shape's `key`.
//!
//! After the sets come clauses, each at most once and in this order: `where` (a condition on
//! the members' fields), `sort by FIELD [asc|desc]`, `limit N`, and `label FIELD` (what a
//! member is called in the region). It is a small language, not a programming language:
//!
//! ```text
//! shows   = sets [ "where" cond ] [ "sort" "by" FIELD [ "asc" | "desc" ] ] [ "limit" N ]
//!           [ "label" FIELD ]
//! sets    = names of inputs with & | - and ( ), as an area is written over parts
//! cond    = all { "or" all }
//! all     = one { "and" one }
//! one     = "not" one | "(" cond ")" | FIELD [ op value ]
//! op      = "=" | "!=" | "<" | "<=" | ">" | ">="
//! value   = "quoted text" | a word: 6, 2.5, 2026-11-10, done
//! ```
//!
//! The set expression reads like a region's `area`, so the canvas crate's area parser reads it,
//! with the inputs standing in for parts; what it gives is a tree over the inputs' indices.

use std::cmp::Ordering;
use std::collections::HashSet;

use chrono::NaiveDate;

use cold_lava_canvas::{Area, AreaError, Kind, Part, Rect};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value as Json;

use crate::items::{Data, Item};
use crate::workspace::search_key;

/// One thing in an input: an item of a query, a note, or a label written out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Member {
    /// An item's id, a note's path, or the label itself.
    pub id: String,
    /// The item's version, for an edit of it; `None` for what isn't an item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Its fields by name, as items carry them.
    pub fields: IndexMap<String, Json>,
}

impl Member {
    /// An item as a member: its fields, its text for a list item or a heading, and its note.
    pub fn of_item(item: &Item) -> Member {
        let mut fields = IndexMap::new();
        match &item.data {
            Data::Typed(t) => {
                for (k, v) in &t.fields {
                    fields.insert(k.to_string(), v.clone());
                }
            }
            Data::List { text } | Data::Heading { text, .. } => {
                fields.insert("text".into(), Json::String(text.clone()));
            }
            Data::Block => {}
        }
        fields
            .entry("note".into())
            .or_insert_with(|| Json::String(item.note.clone()));
        Member {
            id: item.id.clone(),
            rev: Some(item.rev.clone()),
            fields,
        }
    }

    /// A label written out: its `name` is the label.
    pub fn label(label: &str) -> Member {
        let fields = IndexMap::from([("name".to_string(), Json::String(label.to_string()))]);
        Member {
            id: label.to_string(),
            rev: None,
            fields,
        }
    }

    /// `field` as text: a list's values joined with ", "; `None` when it has no value.
    pub fn text(&self, field: &str) -> Option<String> {
        text_of(self.fields.get(field)?)
    }

    /// How it is labelled when nothing says: its `name`, `title` or `text`, else its first text
    /// field, else its id.
    pub fn default_label(&self) -> String {
        (["name", "title", "text"].iter())
            .find_map(|f| self.text(f))
            .or_else(|| {
                self.fields
                    .values()
                    .find_map(|v| v.as_str().map(String::from))
            })
            .unwrap_or_else(|| self.id.clone())
    }

    /// What makes it the same as a member of another input: its `key` field, folded with
    /// `search_key`; its id without a key, or when it has no value for it.
    fn key(&self, key: Option<&str>) -> String {
        match key.and_then(|k| self.text(k)) {
            Some(v) => format!("={}", search_key(&v)),
            None => format!("#{}", self.id),
        }
    }
}

/// A value as text: a string as it is, a number or a bool as written, a list joined with ", ".
pub fn text_of(v: &Json) -> Option<String> {
    match v {
        Json::Null => None,
        Json::String(s) => Some(s.clone()),
        Json::Array(a) if a.is_empty() => None,
        Json::Array(a) => Some(a.iter().filter_map(text_of).collect::<Vec<_>>().join(", ")),
        v => Some(v.to_string()),
    }
}

/// A region's `shows`, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Shows {
    /// Over the inputs: `Area::Part(i)` is input `i`.
    sets: Area,
    filter: Option<Cond>,
    sort: Option<Sort>,
    limit: Option<usize>,
    /// The field a member is labelled by; its default label without one.
    label: Option<String>,
}

/// `sort by FIELD [desc]`.
#[derive(Clone, Debug, PartialEq)]
struct Sort {
    field: String,
    desc: bool,
}

/// A condition on a member's fields: `where status = "done" and vei >= 6`.
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// The field has a value.
    Has(String),
    Compare {
        field: String,
        op: Op,
        value: String,
    },
    Not(Box<Cond>),
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Words that start a clause, in the order the clauses come.
const CLAUSES: [&str; 4] = ["where", "sort", "limit", "label"];

/// Words the language reads itself, which a field can't be called.
const KEYWORDS: [&str; 10] = [
    "where", "sort", "limit", "label", "and", "or", "not", "by", "asc", "desc",
];

impl Shows {
    /// Reads `text` over the shape's `inputs`; the error is one line.
    pub fn parse(text: &str, inputs: &[String]) -> Result<Shows, String> {
        let at = (word_starts(text))
            .find(|(_, w)| CLAUSES.contains(w))
            .map_or(text.len(), |(at, _)| at);
        let stand_ins: Vec<Part> = (inputs.iter())
            .map(|i| Part::new(Some(i), Kind::Rect(Rect::UNIT)))
            .collect();
        let sets = Area::parse(&text[..at], &stand_ins).map_err(|e| match e {
            AreaError::UnknownPart(n) => format!("{n} is not one of its inputs"),
            e => e.to_string(),
        })?;
        let mut p = Parser::new(&text[at..])?;
        let mut shows = Shows {
            sets,
            filter: None,
            sort: None,
            limit: None,
            label: None,
        };
        let mut next_clause = 0;
        while let Some(tok) = p.next() {
            let clause = match &tok {
                Tok::Word(w) => CLAUSES.iter().position(|c| c == w),
                _ => None,
            };
            let Some(clause) = clause.filter(|c| *c >= next_clause) else {
                return Err(format!(
                    "unexpected {}: after the sets come where, sort by, limit and label, each once and in that order",
                    tok.show()
                ));
            };
            next_clause = clause + 1;
            match CLAUSES[clause] {
                "where" => shows.filter = Some(p.cond()?),
                "sort" => {
                    p.expect("by")?;
                    let field = p.field()?;
                    let desc = match p.peek_word() {
                        Some("desc") => true,
                        Some("asc") => false,
                        _ => {
                            shows.sort = Some(Sort { field, desc: false });
                            continue;
                        }
                    };
                    p.pos += 1;
                    shows.sort = Some(Sort { field, desc });
                }
                "limit" => {
                    let n = p.word("a number after limit")?;
                    let n = n.parse().map_err(|_| format!("limit {n}: not a count"))?;
                    shows.limit = Some(n);
                }
                _ => shows.label = Some(p.field()?),
            }
        }
        Ok(shows)
    }

    /// The members it shows, given each input's (in the order of the shape's inputs), matched
    /// across inputs on `key`. A member comes from the left of `&` and `-`, and from the left
    /// of `|` before the right's; then `where` keeps those it holds for, `sort` orders them
    /// (stable; those without the field last), and `limit` keeps the first ones.
    pub fn eval<'m>(&self, inputs: &[&'m [Member]], key: Option<&str>) -> Vec<&'m Member> {
        let mut out = sets(&self.sets, inputs, key);
        if let Some(c) = &self.filter {
            out.retain(|m| c.holds(m));
        }
        if let Some(s) = &self.sort {
            out.sort_by(
                |a, b| match (sort_key(a, &s.field), sort_key(b, &s.field)) {
                    (Some(a), Some(b)) if s.desc => b.cmp(&a),
                    (Some(a), Some(b)) => a.cmp(&b),
                    (a, b) => a.is_none().cmp(&b.is_none()),
                },
            );
        }
        if let Some(n) = self.limit {
            out.truncate(n);
        }
        out
    }

    /// What `m` is called in the region: its `label` field, else its default label.
    pub fn label(&self, m: &Member) -> String {
        (self.label.as_deref())
            .and_then(|f| m.text(f))
            .unwrap_or_else(|| m.default_label())
    }
}

impl Cond {
    /// Reads a condition alone: a query input's `where`.
    pub fn parse(text: &str) -> Result<Cond, String> {
        let mut p = Parser::new(text)?;
        let cond = p.cond()?;
        match p.next() {
            None => Ok(cond),
            Some(t) => Err(format!("unexpected {} after the condition", t.show())),
        }
    }

    /// Whether `m` meets it. A comparison needs a value: a member without one never meets it
    /// (`!=` included). On a list, `=` and the orderings hold when one of its values does, and
    /// `!=` when none is equal.
    pub fn holds(&self, m: &Member) -> bool {
        match self {
            Cond::Has(f) => m.text(f).is_some_and(|t| !t.is_empty()),
            Cond::Not(c) => !c.holds(m),
            Cond::And(a, b) => a.holds(m) && b.holds(m),
            Cond::Or(a, b) => a.holds(m) || b.holds(m),
            Cond::Compare { field, op, value } => {
                let values: Vec<&Json> = match m.fields.get(field) {
                    None | Some(Json::Null) => return false,
                    Some(Json::Array(a)) if a.is_empty() => return false,
                    Some(Json::Array(a)) => a.iter().collect(),
                    Some(v) => vec![v],
                };
                use Ordering::{Equal, Greater, Less};
                let any = |want: &[Ordering]| {
                    (values.iter()).any(|v| order(v, value).is_some_and(|o| want.contains(&o)))
                };
                match op {
                    Op::Eq => any(&[Equal]),
                    Op::Ne => !any(&[Equal]),
                    Op::Lt => any(&[Less]),
                    Op::Le => any(&[Less, Equal]),
                    Op::Gt => any(&[Greater]),
                    Op::Ge => any(&[Greater, Equal]),
                }
            }
        }
    }
}

/// A value against a literal: as numbers when both read as one, as dates when both are
/// `YYYY-MM-DD`, else as text folded with `search_key` (a tag's leading `#` aside).
fn order(v: &Json, literal: &str) -> Option<Ordering> {
    if let (Some(a), Ok(b)) = (number(v), literal.parse::<f64>()) {
        return a.partial_cmp(&b);
    }
    let text = text_of(v)?;
    if let (Some(a), Some(b)) = (date(&text), date(literal)) {
        return Some(a.cmp(&b));
    }
    let fold = |s: &str| search_key(s.trim_start_matches('#'));
    Some(fold(&text).cmp(&fold(literal)))
}

fn number(v: &Json) -> Option<f64> {
    match v {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// What `sort by` orders on: numbers before text, a list by its first value.
#[derive(Debug, PartialEq)]
enum SortKey {
    Number(f64),
    Text(String),
}

impl Eq for SortKey {}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (SortKey::Number(a), SortKey::Number(b)) => a.total_cmp(b),
            (SortKey::Number(_), SortKey::Text(_)) => Ordering::Less,
            (SortKey::Text(_), SortKey::Number(_)) => Ordering::Greater,
            (SortKey::Text(a), SortKey::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn sort_key(m: &Member, field: &str) -> Option<SortKey> {
    let v = match m.fields.get(field)? {
        Json::Array(a) => a.first()?,
        v => v,
    };
    match number(v) {
        Some(n) => Some(SortKey::Number(n)),
        None => text_of(v).map(|t| SortKey::Text(search_key(&t))),
    }
}

/// Each run of non-space characters with the byte where it starts.
fn word_starts(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let rest = &text[at..];
        let start = at + rest.find(|c: char| !c.is_whitespace())?;
        let end = (text[start..].find(char::is_whitespace)).map_or(text.len(), |e| start + e);
        at = end;
        Some((start, &text[start..end]))
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Quoted(String),
    Op(Op),
    Open,
    Close,
}

impl Tok {
    fn show(&self) -> String {
        match self {
            Tok::Word(w) => format!("`{w}`"),
            Tok::Quoted(q) => format!("{q:?}"),
            Tok::Op(op) => format!("`{}`", op.text()),
            Tok::Open => "`(`".into(),
            Tok::Close => "`)`".into(),
        }
    }
}

impl Op {
    fn text(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }
}

/// A field, a value or a keyword: letters, digits and `_ - . / # :` (`volcano/strato`,
/// `2026-11-10`, `-3`).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '#' | ':')
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(text: &str) -> Result<Parser, String> {
        let mut tokens = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let tok = match c {
                c if c.is_whitespace() => continue,
                '(' => Tok::Open,
                ')' => Tok::Close,
                '"' => {
                    let mut q = String::new();
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some(c) => q.push(c),
                            None => return Err(format!("a quote isn't closed: \"{q}")),
                        }
                    }
                    Tok::Quoted(q)
                }
                '=' => Tok::Op(Op::Eq),
                '!' if chars.next_if_eq(&'=').is_some() => Tok::Op(Op::Ne),
                '<' if chars.next_if_eq(&'=').is_some() => Tok::Op(Op::Le),
                '>' if chars.next_if_eq(&'=').is_some() => Tok::Op(Op::Ge),
                '<' => Tok::Op(Op::Lt),
                '>' => Tok::Op(Op::Gt),
                c if is_word(c) => {
                    let mut w = String::from(c);
                    while let Some(c) = chars.next_if(|c| is_word(*c)) {
                        w.push(c);
                    }
                    Tok::Word(w)
                }
                c => return Err(format!("unexpected `{c}`")),
            };
            tokens.push(tok);
        }
        Ok(Parser { tokens, pos: 0 })
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += usize::from(t.is_some());
        t
    }

    fn peek_word(&self) -> Option<&str> {
        match self.tokens.get(self.pos) {
            Some(Tok::Word(w)) => Some(w),
            _ => None,
        }
    }

    fn word(&mut self, what: &str) -> Result<String, String> {
        match self.next() {
            Some(Tok::Word(w)) => Ok(w),
            Some(t) => Err(format!("expected {what}, found {}", t.show())),
            None => Err(format!("expected {what} at the end")),
        }
    }

    fn expect(&mut self, word: &str) -> Result<(), String> {
        match self.word(&format!("`{word}`"))? {
            w if w == word => Ok(()),
            w => Err(format!("expected `{word}`, found `{w}`")),
        }
    }

    fn field(&mut self) -> Result<String, String> {
        let w = self.word("a field")?;
        match KEYWORDS.contains(&w.as_str()) {
            true => Err(format!("expected a field, found `{w}`")),
            false => Ok(w),
        }
    }

    fn cond(&mut self) -> Result<Cond, String> {
        let mut c = self.all()?;
        while self.peek_word() == Some("or") {
            self.pos += 1;
            c = Cond::Or(Box::new(c), Box::new(self.all()?));
        }
        Ok(c)
    }

    fn all(&mut self) -> Result<Cond, String> {
        let mut c = self.one()?;
        while self.peek_word() == Some("and") {
            self.pos += 1;
            c = Cond::And(Box::new(c), Box::new(self.one()?));
        }
        Ok(c)
    }

    fn one(&mut self) -> Result<Cond, String> {
        if self.peek_word() == Some("not") {
            self.pos += 1;
            return Ok(Cond::Not(Box::new(self.one()?)));
        }
        if self.tokens.get(self.pos) == Some(&Tok::Open) {
            self.pos += 1;
            let c = self.cond()?;
            return match self.next() {
                Some(Tok::Close) => Ok(c),
                _ => Err("a bracket isn't closed".into()),
            };
        }
        let field = self.field()?;
        let Some(Tok::Op(op)) = self.tokens.get(self.pos).cloned() else {
            return Ok(Cond::Has(field));
        };
        self.pos += 1;
        let value = match self.next() {
            Some(Tok::Word(w) | Tok::Quoted(w)) => w,
            Some(t) => {
                let found = t.show();
                return Err(format!(
                    "expected a value after {field} {}, found {found}",
                    op.text()
                ));
            }
            None => return Err(format!("expected a value after {field} {}", op.text())),
        };
        Ok(Cond::Compare { field, op, value })
    }
}

fn sets<'m>(area: &Area, inputs: &[&'m [Member]], key: Option<&str>) -> Vec<&'m Member> {
    let keys = |ms: &[&Member]| -> HashSet<String> { ms.iter().map(|m| m.key(key)).collect() };
    match area {
        Area::Part(i) => inputs.get(*i).map_or(Vec::new(), |ms| ms.iter().collect()),
        Area::And(a, b) => {
            let right = keys(&sets(b, inputs, key));
            let left = sets(a, inputs, key);
            left.into_iter()
                .filter(|m| right.contains(&m.key(key)))
                .collect()
        }
        Area::Minus(a, b) => {
            let right = keys(&sets(b, inputs, key));
            let left = sets(a, inputs, key);
            left.into_iter()
                .filter(|m| !right.contains(&m.key(key)))
                .collect()
        }
        Area::Or(a, b) => {
            let mut out = sets(a, inputs, key);
            let mut seen = keys(&out);
            for m in sets(b, inputs, key) {
                if seen.insert(m.key(key)) {
                    out.push(m);
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<Member> {
        names.iter().map(|n| Member::label(n)).collect()
    }

    fn shown(
        expr: &str,
        a: &[Member],
        b: &[Member],
        c: &[Member],
        key: Option<&str>,
    ) -> Vec<String> {
        let inputs = ["a", "b", "c"].map(String::from);
        let shows = Shows::parse(expr, &inputs).unwrap();
        (shows.eval(&[a, b, c], key).iter())
            .map(|m| m.default_label())
            .collect()
    }

    #[test]
    fn set_operations_match_on_the_key_with_folding() {
        let a = labels(&["Kīlauea", "Krakatoa", "Fuji"]);
        let b = labels(&["kilauea", "Vesuvius", "FUJI"]);
        let c = labels(&["Fuji"]);
        let name = Some("name");
        assert_eq!(shown("a & b", &a, &b, &c, name), ["Kīlauea", "Fuji"]);
        assert_eq!(shown("a - b", &a, &b, &c, name), ["Krakatoa"]);
        assert_eq!(shown("b - a", &a, &b, &c, name), ["Vesuvius"]);
        assert_eq!(
            shown("a | b", &a, &b, &c, name),
            ["Kīlauea", "Krakatoa", "Fuji", "Vesuvius"]
        );
        assert_eq!(shown("a & b - c", &a, &b, &c, name), ["Kīlauea"]);
        assert_eq!(shown("(a | b) & c", &a, &b, &c, name), ["Fuji"]);
        // Without a key, a member is the same only as itself: labels differing in case differ.
        assert_eq!(shown("a & b", &a, &b, &c, None), Vec::<String>::new());
        assert_eq!(shown("a & c", &a, &b, &c, None), ["Fuji"]);
    }

    #[test]
    fn a_shows_naming_no_input_is_refused() {
        let inputs = ["a".to_string()];
        assert_eq!(
            Shows::parse("a & z", &inputs).unwrap_err(),
            "z is not one of its inputs"
        );
        assert!(Shows::parse("a &", &inputs).is_err());
    }

    /// Volcanoes as items carry them: a name, a number, a date, a list of tags; Parícutin has no
    /// VEI.
    fn volcanoes() -> Vec<Member> {
        let v = |name: &str, vei: Option<i64>, last: &str, tags: &[&str]| {
            let mut m = Member::label(name);
            m.fields
                .insert("vei".into(), vei.map_or(Json::Null, Json::from));
            m.fields.insert("last".into(), Json::from(last));
            m.fields.insert("tags".into(), Json::from(tags.to_vec()));
            m
        };
        vec![
            v(
                "Krakatoa",
                Some(6),
                "2024-04-01",
                &["ring-of-fire", "caldera"],
            ),
            v("Vesuvius", Some(5), "1944-03-18", &["strato"]),
            v(
                "Mount Fuji",
                Some(5),
                "1707-12-16",
                &["ring-of-fire", "strato"],
            ),
            v("Parícutin", None, "1952-02-25", &["cinder-cone"]),
            v("Yellowstone", Some(8), "0001-01-01", &[]),
        ]
    }

    /// What `a SHOWS` lists over the volcanoes, by label.
    fn over(shows: &str) -> Vec<String> {
        let all = volcanoes();
        let s = Shows::parse(&format!("a {shows}"), &["a".to_string()]).unwrap();
        s.eval(&[&all], Some("name"))
            .into_iter()
            .map(|m| s.label(m))
            .collect()
    }

    #[test]
    fn where_compares_numbers_dates_and_folded_text() {
        assert_eq!(over("where vei >= 6"), ["Krakatoa", "Yellowstone"]);
        assert_eq!(over("where vei > 5"), ["Krakatoa", "Yellowstone"]);
        assert_eq!(over("where vei < 6"), ["Vesuvius", "Mount Fuji"]);
        assert_eq!(over("where vei <= 5"), ["Vesuvius", "Mount Fuji"]);
        assert_eq!(over("where vei = 5"), ["Vesuvius", "Mount Fuji"]);
        assert_eq!(
            over("where vei != 5"),
            ["Krakatoa", "Yellowstone"],
            "no value never compares"
        );
        assert_eq!(
            over("where last >= 1900-01-01"),
            ["Krakatoa", "Vesuvius", "Parícutin"]
        );
        assert_eq!(over("where name = \"mount fuji\""), ["Mount Fuji"]);
        assert_eq!(
            over("where name = paricutin"),
            ["Parícutin"],
            "accents folded"
        );
        assert_eq!(over("where name < l"), ["Krakatoa"]);
    }

    #[test]
    fn where_reads_lists_and_combines_conditions() {
        assert_eq!(
            over("where tags = ring-of-fire"),
            ["Krakatoa", "Mount Fuji"]
        );
        assert_eq!(over("where tags = #strato"), ["Vesuvius", "Mount Fuji"]);
        assert_eq!(
            over("where tags != strato"),
            ["Krakatoa", "Parícutin"],
            "an empty list has no value"
        );
        assert_eq!(
            over("where vei"),
            ["Krakatoa", "Vesuvius", "Mount Fuji", "Yellowstone"]
        );
        assert_eq!(over("where not vei"), ["Parícutin"]);
        assert_eq!(
            over("where tags = strato and vei = 5 or name = Yellowstone"),
            ["Vesuvius", "Mount Fuji", "Yellowstone"],
            "and binds tighter than or"
        );
        assert_eq!(
            over("where tags = strato and (vei = 6 or name = Vesuvius)"),
            ["Vesuvius"]
        );
    }

    #[test]
    fn sort_limit_and_label() {
        assert_eq!(
            over("sort by vei"),
            [
                "Vesuvius",
                "Mount Fuji",
                "Krakatoa",
                "Yellowstone",
                "Parícutin"
            ],
            "stable, numbers in order, no value last"
        );
        assert_eq!(
            over("sort by vei desc"),
            [
                "Yellowstone",
                "Krakatoa",
                "Vesuvius",
                "Mount Fuji",
                "Parícutin"
            ]
        );
        assert_eq!(over("sort by name asc limit 2"), ["Krakatoa", "Mount Fuji"]);
        assert_eq!(
            over("where vei >= 5 sort by last desc limit 2 label last"),
            ["2024-04-01", "1944-03-18"]
        );
        assert_eq!(
            over("limit 1 label nothing"),
            ["Krakatoa"],
            "no such field: its name"
        );
        assert_eq!(over("limit 0"), Vec::<String>::new());
    }

    #[test]
    fn sets_then_clauses() {
        let all = volcanoes();
        let strato: Vec<Member> = all[1..3].to_vec();
        let inputs = ["a".to_string(), "b".to_string()];
        let s = Shows::parse("a - b where vei sort by vei desc", &inputs).unwrap();
        let shown: Vec<String> = (s.eval(&[&all, &strato], Some("name")).iter())
            .map(|m| m.default_label())
            .collect();
        assert_eq!(shown, ["Yellowstone", "Krakatoa"]);
    }

    #[test]
    fn mistakes_are_one_line() {
        let a = ["a".to_string()];
        let e = |s: &str| Shows::parse(s, &a).unwrap_err();
        assert_eq!(e("a where"), "expected a field at the end");
        assert_eq!(e("a where vei >="), "expected a value after vei >=");
        assert_eq!(e("a where (vei = 1"), "a bracket isn't closed");
        assert_eq!(e("a where name = \"x"), "a quote isn't closed: \"x");
        assert_eq!(e("a sort vei"), "expected `by`, found `vei`");
        assert_eq!(e("a limit many"), "limit many: not a count");
        assert_eq!(e("a where and"), "expected a field, found `and`");
        assert_eq!(e("a where vei ~ 3"), "unexpected `~`");
        assert_eq!(
            e("a label name where vei"),
            "unexpected `where`: after the sets come where, sort by, limit and label, each once and in that order"
        );
        assert_eq!(
            Cond::parse("vei >= 6 6").unwrap_err(),
            "unexpected `6` after the condition"
        );
    }
}
