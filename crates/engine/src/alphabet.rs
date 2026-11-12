//! The alphabet: the closed set of things a file can be said to hold, whatever its format
//! (docs/extensions.md, "The alphabet"; the design is docs/neutral-model.md). Pieces with a
//! role, the text inside them, the relations between them, and open attributes on each.
//!
//! The code holds the roles, the marks and their version. A type's `on` is written in the roles
//! (`types::On`), an owner in another process gives pieces in them (`owner::given::Piece`), and
//! items keep their own kinds (`items::Data`).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The alphabet's version, sent in `initialize` beside `apiVersion`. A letter (a role, a mark, a
/// relation) is added rarely, by the engine only, and each addition raises this. 1 named the
/// alphabet. 2 is the first an owner can speak: `file/read` takes pieces, their text as runs
/// with marks, and the relations written `in` and `cells`. 3 writes one more relation, a link
/// between two pieces (`{from, to, label}` in `links`): no letter is new, and the number goes
/// up because a host of version 2 refuses an answer that holds one.
pub const VERSION: u32 = 3;

/// What a piece of writing is. Closed: a new kind of thing is a type over one of these, with
/// attributes, never a tenth role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The whole file.
    Document,
    /// A title and what is under it.
    Section,
    /// A run of prose.
    Text,
    /// One entry in a list; it may hold more.
    Item,
    /// One record, in cells.
    Row,
    /// Set apart from the flow: a quote, a callout.
    Aside,
    /// Text kept exactly.
    Verbatim,
    /// Something from elsewhere, shown here.
    Embed,
    /// Kept, not understood.
    Opaque,
}

impl Role {
    pub const ALL: [Role; 9] = [
        Role::Document,
        Role::Section,
        Role::Text,
        Role::Item,
        Role::Row,
        Role::Aside,
        Role::Verbatim,
        Role::Embed,
        Role::Opaque,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Role::Document => "document",
            Role::Section => "section",
            Role::Text => "text",
            Role::Item => "item",
            Role::Row => "row",
            Role::Aside => "aside",
            Role::Verbatim => "verbatim",
            Role::Embed => "embed",
            Role::Opaque => "opaque",
        }
    }

    pub fn from_name(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == name)
    }

    /// What the type language called it before the roles were named, when it named it at all.
    /// Still read wherever a role is (`on`, `where.inside`), and still what a definition's JSON
    /// says as `on`, so a type file or a client written before keeps working.
    pub fn alias(self) -> Option<&'static str> {
        match self {
            Role::Document => Some("note"),
            Role::Section => Some("heading"),
            Role::Text => Some("paragraph"),
            Role::Item => Some("list-item"),
            Role::Row => Some("table-row"),
            Role::Aside => Some("quote"),
            Role::Verbatim => Some("code"),
            Role::Embed | Role::Opaque => None,
        }
    }
}

/// On the wire a role is its name.
impl Serialize for Role {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Role, D::Error> {
        let name = String::deserialize(d)?;
        Role::from_name(&name).ok_or_else(|| {
            let names: Vec<&str> = Role::ALL.iter().map(|r| r.name()).collect();
            serde::de::Error::custom(format!("{name} is not a role: one of {}", names.join(", ")))
        })
    }
}

/// What a run of text can be marked as. Closed, like the roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    Emphasis,
    Strong,
    Struck,
    Highlighted,
    Code,
    Math,
}

impl Mark {
    pub const ALL: [Mark; 6] = [
        Mark::Emphasis,
        Mark::Strong,
        Mark::Struck,
        Mark::Highlighted,
        Mark::Code,
        Mark::Math,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mark::Emphasis => "emphasis",
            Mark::Strong => "strong",
            Mark::Struck => "struck",
            Mark::Highlighted => "highlighted",
            Mark::Code => "code",
            Mark::Math => "math",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A role and a mark are their names on the wire, and a name outside the alphabet is
    /// refused.
    #[test]
    fn roles_and_marks_are_their_names_on_the_wire() {
        for role in Role::ALL {
            let wire = serde_json::to_value(role).unwrap();
            assert_eq!(wire, serde_json::json!(role.name()));
            assert_eq!(serde_json::from_value::<Role>(wire).unwrap(), role);
        }
        for mark in Mark::ALL {
            let wire = serde_json::to_value(mark).unwrap();
            assert_eq!(wire, serde_json::json!(mark.name()));
            assert_eq!(serde_json::from_value::<Mark>(wire).unwrap(), mark);
        }
        let slide = serde_json::from_value::<Role>(serde_json::json!("slide"));
        assert!(slide.unwrap_err().to_string().contains("not a role"));
        assert!(serde_json::from_value::<Mark>(serde_json::json!("underline")).is_err());
    }

    /// The reference's table of roles is a copy of `Role::ALL`: same roles, same order, same
    /// older names, and the version it states is this one.
    #[test]
    fn the_reference_lists_these_roles_and_this_version() {
        let docs = include_str!("../../../docs/extensions.md");
        let (_, section) = docs
            .split_once("\n### The alphabet, as built\n")
            .expect("docs/extensions.md has the section");
        let section = section.split("\n### ").next().unwrap();
        let rows: Vec<(String, String)> = section
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| {
                let mut cells = l.split('|').map(str::trim);
                let role = cells.next()?.trim_end_matches('`').to_string();
                let alias = cells.nth(1)?.trim_matches('`').to_string();
                Role::from_name(&role).map(|_| (role, alias))
            })
            .take(Role::ALL.len())
            .collect();
        let ours: Vec<(String, String)> = Role::ALL
            .iter()
            .map(|r| {
                (
                    r.name().to_string(),
                    r.alias().unwrap_or("none").to_string(),
                )
            })
            .collect();
        assert_eq!(rows, ours);
        assert!(
            section.contains(&format!("The alphabet's version is **{VERSION}**")),
            "the section states version {VERSION}"
        );
    }
}
