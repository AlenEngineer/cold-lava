//! The frontend framework, extracted from the terminal UI as its parts prove themselves.
//!
//! This crate holds what every cold-lava frontend shares. Today that is the view vocabulary
//! of the view gateway, as data: what a list (`list`), a table (`table`), an image (`image`)
//! and a scene (`scene`) say, and the messages that carry them (`view`). Every type derives
//! serde and tests pin its JSON. It also holds `shown`, the one function that makes text from
//! outside safe to show, which every frontend calls where text leaves for the screen. The extension host (`cold-lava-host`) names these types, so a
//! frontend and an extension agree on them; drawing them, and their keys, stay with each
//! frontend. It has no UI dependency: no ratatui, crossterm, portable-pty or vt100.
//!
//! Still to come: the view host that lays views out and gives them focus, document layout
//! with embeds in place, and the scene renderer's frontend-neutral parts. See "The frontend:
//! one, opinionated" in `docs/engine.md`. Pieces move here from `crates/cold-lava` once a
//! second frontend needs them, not before.

pub mod image;
pub mod list;
pub mod scene;
pub mod shown;
pub mod table;
pub mod view;
