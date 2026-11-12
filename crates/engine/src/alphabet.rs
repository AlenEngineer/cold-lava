//! The alphabet: the closed set of things a file can be said to hold, whatever its format
//! (docs/extensions.md, "The alphabet"; the design is docs/neutral-model.md). Pieces with a
//! role, the text inside them, the relations between them, and open attributes on each.
//!
//! So far the alphabet is named, not emitted: the code holds the roles and their version, a
//! type's `on` is written in them (`types::On`), and items keep their own kinds (`items::Data`).
//! Text, relations and attributes are defined in the docs only, until an owner gives them.

/// The alphabet's version, sent in `initialize` beside `apiVersion`. A letter (a role, a mark, a
/// relation) is added rarely, by the engine only, and each addition raises this.
pub const VERSION: u32 = 1;

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

#[cfg(test)]
mod tests {
    use super::*;

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
