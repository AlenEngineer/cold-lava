//! cold-lava's canvas engine, headless (step 2 of `docs/shapes.md`).
//!
//! It computes what a frontend draws and never draws it: shapes made of parts in their own unit
//! box, placed by the user (move, stretch, rotate); regions combined from parts with `&`, `|`
//! and `-`; and, built on those, rasterizing to terminal cells, braille dots or pixels, fitting
//! text and labels inside regions, and hit testing. The terminal UI and a GUI paint the same
//! results, so neither re-derives geometry. It must not depend on a UI crate (ratatui,
//! crossterm, portable-pty, vt100).

pub mod area;
pub mod geom;
pub mod part;
pub mod shape;

pub use area::{Area, AreaError};
pub use geom::{Handle, Placement, Point, Rect, Transform};
pub use part::{Kind, Part, Path, Segment};
pub use shape::{Placed, Region, Shape, ShapeError};
