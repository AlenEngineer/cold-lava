//! What a frontend configured for the workspaces it opens: the Markdown flavour, `[style]`, and
//! the layers of types, attribute readings and shapes that come between the built-in ones and a
//! workspace's own. The frontend reads its own settings (the engine never reads config.toml)
//! and hands this to each `Core` it opens and each workspace it scans, which keep it as theirs:
//! two workspaces open at once can be read with different ones (#180). Data only; the readers
//! of each part are where they were (`markdown::flavour_for`, `style::Style::of`,
//! `types::for_workspace`, `owner::markdown::attributes::for_workspace`,
//! `shapes::for_workspace`).

use crate::markdown::Flavour;
use crate::owner::markdown::attributes;
use crate::{shapes, style, types};

#[derive(Clone, Default)]
pub struct Configured {
    /// `None` lets the workspace decide (`markdown::flavour_for`).
    pub flavour: Option<Flavour>,
    pub style: style::Overrides,
    /// Extensions' types, then config.toml's.
    pub types: Vec<types::Layer>,
    pub attributes: Vec<attributes::Layer>,
    /// Extensions' shapes, then config.toml's.
    pub shapes: Vec<shapes::Layer>,
}
