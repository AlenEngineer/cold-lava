//! cold-lava's small text editor, headless.
//!
//! It knows text and never draws: a buffer, cursors and selections, edits as transactions with
//! undo and redo, movement by grapheme, word, line and block, and the editor's commands. The
//! terminal UI and the GUI each render it their own way (a frontend computes wrapping for its
//! width and asks the editor where the cursor lands), so both edit exactly alike.
//!
//! It is the "light block editor" of `docs/engine.md`: quick edits of a paragraph, a card's text,
//! a table cell, a commit-message-sized note, never a replacement for Neovim. Saves go through
//! the engine's safe write path, so an edit never overwrites a change made elsewhere; styling
//! comes from the engine's parse of the Markdown, never from a second parser.
//!
//! Empty for now; it fills when the document view needs to edit in place.
