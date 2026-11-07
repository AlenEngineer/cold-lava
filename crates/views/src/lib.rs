//! The frontend framework, to be extracted from the terminal UI as its parts prove themselves.
//!
//! This crate will hold what every cold-lava frontend shares: the view vocabulary of the view
//! gateway (markdown, list and tree, table, scene, pane, image), the view host that lays views
//! out and gives them focus, document layout with embeds in place, and the scene renderer's
//! frontend-neutral parts. The terminal UI and the GUI both draw from it; extensions describe
//! views in its terms and never draw.
//!
//! It is empty on purpose: see "The frontend: one, opinionated" in `docs/engine.md`. Pieces move
//! here from `crates/cold-lava` once a second frontend needs them, not before.
