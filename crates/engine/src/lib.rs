//! The vault engine: what cold-lava knows about a vault, with no user interface. See "Engine
//! and frontend" in docs/engine.md.

pub mod bookmarks;
pub mod core;
pub mod daily;
pub mod frontmatter;
pub mod index;
pub mod items;
pub mod markdown;
pub mod matcher;
pub mod merge;
pub mod paths;
pub mod rename;
pub mod search;
pub mod tasks;
pub mod templates;
pub mod trash;
pub mod types;
pub mod vault;
pub mod write;
