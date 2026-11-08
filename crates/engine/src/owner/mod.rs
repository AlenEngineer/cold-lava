//! Owners: what translates a kind of file to the workspace's items and links, and back
//! (docs/extensions.md, "Owners and types"). An owner has three jobs, each from a file's text:
//!
//! - **read**: the text → its links and items;
//! - **write**: an edit of one item → the file's new text;
//! - **relink**: links to moved files → the file's new text.
//!
//! The engine keeps everything that has to be shared and trusted: the watcher, the index and its
//! one link resolver, types, permissions, the safe write and the read-back check. Markdown is the
//! engine's own format and the first owner (`markdown`). An extension can own another kind of
//! file: it reads it in its own process and answers later (`given`, `cache`), and writes it
//! when asked (`edits`).

pub mod cache;
pub mod edits;
pub mod given;
pub mod markdown;
