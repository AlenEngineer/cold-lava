//! The scene view's words (docs/extensions.md, "Scenes"): nodes and the links between them,
//! as a sender describes them. A frontend lays them out, draws them and moves around in them
//! its own way.

use serde::{Deserialize, Serialize};

/// What a scene shows: nodes, the links between them and the colour groups they are in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// The first words of the frame's title (the workspace's name).
    pub title: String,
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    /// The legend; a node names its group by its place here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// What the sender shows, for the frame's top right ("tags", "by folder").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// What the sender knows it by; positions and the selection follow it across updates.
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Look::is_note")]
    pub look: Look,
    /// Its colour group, a place in `Scene::groups`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<usize>,
    /// The workspace file it stands for (relative), which search also matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// What the status line says about it when it is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// How a node is drawn: a note, a link to nothing yet, or a tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Look {
    #[default]
    Note,
    Ghost,
    Tag,
}

impl Look {
    fn is_note(&self) -> bool {
        *self == Look::Note
    }
}

/// A link from one node to another, by id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub label: String,
    /// `#rrggbb`.
    pub color: String,
}
