//! The workspace engine: what cold-lava knows about a workspace, with no user interface. See "Engine
//! and frontend" in docs/engine.md.

pub mod bookmarks;
pub mod core;
pub mod daily;
pub mod edit;
pub mod fields;
pub mod files;
pub mod frontmatter;
pub mod gateway;
pub mod index;
pub mod items;
pub mod markdown;
pub mod matcher;
pub mod merge;
pub mod owner;
pub mod paths;
pub mod recipe;
pub mod rename;
pub mod resolver;
pub mod search;
pub mod tasks;
pub mod templates;
pub mod tidy;
pub mod trash;
pub mod types;
pub mod workspace;
pub mod write;
