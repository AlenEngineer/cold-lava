//! Who an extension is and what it may ask for: its manifest, from `initialize`, with the
//! kinds of file it opens, the notes it shows, its commands, types and shapes, and the
//! permissions the host checks.

use serde::{Deserialize, Serialize};

/// Who an extension is, from `initialize`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Its id, for messages about it: "nvim".
    pub name: String,
    /// Its name for people: "Neovim".
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// One line on what it does.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub roles: Vec<Role>,
    /// Keys its program understands, as (key, what it does), for the status line and the help.
    pub hints: Vec<(String, String)>,
    /// What it adds to the menu's commands, for keys, the palette, the help and config.toml.
    pub commands: Vec<CommandSpec>,
    /// Types it defines, each with its id: `<extension name>.<type>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<Spec>,
    /// Shapes it defines, each with its id: `<extension name>.<shape>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Spec>,
    /// What it may ask of the workspace gateway. Without the field, read only.
    #[serde(default = "read_only")]
    pub permissions: Vec<Permission>,
    /// Kinds of file it opens, by extension.
    #[serde(default, rename = "file-types", skip_serializing_if = "Vec::is_empty")]
    pub file_types: Vec<FileType>,
    /// Notes it shows instead of the reader.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shows: Vec<Shows>,
}

/// A kind of file an extension owns: the menu hands it every file whose name ends in
/// `.<extension>` (`file/open`) and tells it when one changes (`file/changed`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileType {
    /// Lowercase, without the dot: "csv".
    pub extension: String,
    /// For people: "CSV table".
    pub title: String,
    /// One of its commands, which makes a new file of this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

/// Notes an extension shows instead of the reader (`[[shows]]`): those that meet `where` as a
/// note shape's `where` matches a note (frontmatter properties, tags, folder). Opening one hands
/// it to the extension (`file/open`); Markdown still reads its items and links and writes its
/// edits, so the extension shows the note, it doesn't own it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shows {
    /// For people: "Kanban board".
    pub title: String,
    #[serde(rename = "where")]
    pub conditions: crate::types::Where,
}

impl Shows {
    /// Why it can't be a claim: it must name a note's conditions, and only those.
    pub fn problem(&self) -> Option<&'static str> {
        let w = &self.conditions;
        let block = !(w.header.is_empty()
            && w.section.is_empty()
            && w.language.is_empty()
            && w.inside.is_empty()
            && w.file_type.is_empty());
        if block {
            Some("its where may only name property, tag and folder")
        } else if w.is_empty() {
            Some("its where must name a property, a tag or a folder: it can't show every note")
        } else {
            None
        }
    }

    /// Its conditions as people read them: `kanban-plugin: basic, #board, in Projects`.
    pub fn describe(&self) -> String {
        let w = &self.conditions;
        let props = w.property.iter().map(|(k, v)| format!("{k}: {v}"));
        let tags = (w.tag.iter()).map(|t| format!("#{}", t.trim_start_matches('#')));
        let folders = w.folder.iter().map(|f| format!("in {f}"));
        props
            .chain(tags)
            .chain(folders)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A part of the workspace gateway an extension may use, named in its manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// `items/query`, `items/subscribe`, `items/unsubscribe`, `types/list`.
    #[serde(rename = "items.read")]
    ItemsRead,
    /// `items/edit`.
    #[serde(rename = "items.edit")]
    ItemsEdit,
    /// `workspace/read` and `workspace/files` for kinds of file it doesn't own.
    #[serde(rename = "files.read")]
    FilesRead,
    /// `workspace/write` and `workspace/create` for kinds of file it doesn't own, and
    /// `workspace/create-folder`.
    #[serde(rename = "files.write")]
    FilesWrite,
    /// `workspace/delete` and `workspace/restore`, of any file or folder. `files.write` doesn't
    /// include it, and owning a kind of file doesn't either.
    #[serde(rename = "files.delete")]
    FilesDelete,
}

impl Permission {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Permission::ItemsRead => "items.read",
            Permission::ItemsEdit => "items.edit",
            Permission::FilesRead => "files.read",
            Permission::FilesWrite => "files.write",
            Permission::FilesDelete => "files.delete",
        }
    }
}

pub(crate) fn read_only() -> Vec<Permission> {
    vec![Permission::ItemsRead]
}

/// A command an extension registers. Its id is `<extension name>.<something>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    /// Its default keys, written as in config.toml: "g", "ctrl+o".
    #[serde(default)]
    pub keys: Vec<String>,
}

/// A type or a shape an extension defines (`[[types]]`, `[[shapes]]`): its id,
/// `<extension name>.<name>`, and the rest of its table, a definition in the same language as
/// anyone's (docs/extensions.md, "Types"; docs/shapes.md, "Shape definitions"). The engine reads
/// the definition, so a broken one leaves out that type or shape, not the extension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    #[serde(flatten)]
    pub definition: toml::Table,
}

/// A part of cold-lava an extension provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Opens notes for editing: gets the `editor/*` requests.
    Editor,
}
