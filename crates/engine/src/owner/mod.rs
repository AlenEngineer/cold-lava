//! Owners: what translates a kind of file to the workspace's items and links, and back
//! (docs/extensions.md, "Owners and types"). An owner has three jobs, each from a file's text:
//!
//! - **read**: the text → its links and items;
//! - **write**: an edit of one item → the file's new text;
//! - **relink**: links to moved files → the file's new text.
//!
//! The engine keeps everything that has to be shared and trusted: the watcher, the index and its
//! one link resolver, types, permissions, the safe write and the read-back check.
//!
//! Markdown is the engine's own format and the first owner (`markdown`). It is the one direct
//! path: the scan reads notes with it, and a note's edit or rename is made and written by the
//! engine, with no request and no answer.
//!
//! Every other file goes one way, whoever owns it: the core asks its owner (`file/read`,
//! `file/edit`), and the answer comes in through `Core::file_given` or `Core::file_edited`,
//! where it is read back before anything is written (`edits`). An extension answers later,
//! from its own process (`given`, `cache`). `.canvas` files have the engine's other owner
//! (`canvas`), which answers the same requests in the same terms, inside the call: the core
//! asks through `give` and `edit` below, and an answer from either kind of owner is handled by
//! the same code from there on.

pub mod answer;
pub mod cache;
pub mod canvas;
pub mod change;
pub mod edits;
pub mod given;
pub mod markdown;
mod order;

use given::{Change, Given};

/// The owners in the engine besides Markdown, by the kind of file each reads (lowercase, without
/// the dot). They answer in-process, at once, and no extension can claim their kinds.
pub const BUILT_IN: &[(&str, &str)] = &[(canvas::EXTENSION, canvas::OWNER)];

/// The built-in owner of the file at workspace-relative `rel`, by its extension.
pub fn built_in(rel: &str) -> Option<&'static str> {
    built_in_kind(&crate::files::extension_of(rel)?)
}

/// The built-in owner of files of kind `x` (lowercase, without the dot).
pub fn built_in_kind(x: &str) -> Option<&'static str> {
    BUILT_IN.iter().find(|(e, _)| *e == x).map(|(_, o)| *o)
}

/// Whether the file at `rel` is a board: a JSON Canvas, read by the engine's own owner.
pub fn is_board(rel: &str) -> bool {
    built_in(rel) == Some(canvas::OWNER)
}

/// Whether the owner of the file at `rel` answers once for each version of it: an extension,
/// whose answer comes from its own process and is kept by version (`cache`). Each such answer
/// that left something out was reported before #303, so it still is (`core::owned_news`).
/// The engine's own owners give an answer whenever the engine reads the file, on every scan
/// and after the whiteboard's own saves, so what such a file leaves out is news only when it
/// changes.
pub fn answers_once_per_version(rel: &str) -> bool {
    built_in(rel).is_none()
}

/// `file/read`, answered by the built-in owner of the file at `rel`: `None` when `owner` isn't
/// it, and the request goes out to the extension.
pub fn give(owner: &str, rel: &str, text: &str) -> Option<Result<Given, String>> {
    (built_in(rel) == Some(owner)).then(|| canvas::read(text))
}

/// `file/edit`, answered by the built-in owner of the file at `rel`: `None` when `owner` isn't
/// it, and the request goes out to the extension.
pub fn edit(
    owner: &str,
    rel: &str,
    text: &str,
    changes: &[Change],
) -> Option<Result<String, String>> {
    (built_in(rel) == Some(owner)).then(|| canvas::edit(text, changes))
}
