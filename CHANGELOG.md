# Changelog

All notable changes to cold-lava are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may
change commands, keys and file formats; patch versions only fix things.

## [Unreleased]

## [0.2.0] - 2026-11-06

### Added

- `cold-lava rename VAULT NOTE NEW` renames or moves a note and updates every link to it, like
  Obsidian's "Automatically update internal links": `[[wikilinks]]` (with aliases, headings,
  blocks and embeds), Markdown links (relative or vault paths), links in frontmatter, and
  `.canvas` note cards and text cards. Each link keeps its style; the moved note's own relative
  links are fixed; code is left alone. `--dry-run` shows the changes first, and nothing is
  written if a file changed after the plan was made.
- In the main menu, `R` renames the selected note and `m` moves it to another folder, with a
  preview of every link that will change before you confirm. The open note is saved first, and
  Neovim shows it under its new name afterwards.
- Command palette (`:`) in the main menu: run any command by name, with its key shown.
- Keys are configurable in `~/.config/cold-lava/config.toml` (`[keys]`, by command id);
  `cold-lava keys` lists every command and its keys and reports mistakes in the file.
- Markdown flavours (`commonmark`, `github`, `obsidian`, or your own in `config.toml`) for the
  reader.
- The reader, in the main menu's right pane: the note rendered in the vault's flavour, with
  headings, lists, tasks, quotes, callouts (folded ones closed), code, tables, highlights, tags,
  embed placeholders and links (unresolved ones dimmer). `Tab` selects a link and `Enter`
  follows it like from Neovim, URLs open in the browser, and a click follows a link. It replaces
  the plain preview, and follows changes on disk without losing its place. With Neovim, `v`
  (`note.read`) switches the pane between the reader and Neovim. Its keys are under `reader.*`
  in `config.toml`, and the key names `up`, `down`, `left`, `right`, `pageup`, `pagedown`,
  `home` and `end` are new.

### Changed

- The embedded Neovim is now the first editor extension, on cold-lava's new extension interface
  (see `docs/extensions.md`). The pane's title names the editor (Neovim or Vim) instead of the
  program, and the help lists the editor's keys under its name.
- `Ctrl-q`, which leaves Neovim for the menu, is now the command `pane.leave`. Set its key under
  `[keys]` in `config.toml` if you use `Ctrl-q` in Neovim; it can't be `[]`. The pane's title,
  the status line and the help show the key you set.
- Without an embedded Neovim or Vim, `Enter` on a note in the main menu goes into the reader
  instead of opening `$EDITOR` full screen; `e` still does that.
- Every write to a vault now goes through one path: atomic (a hidden temporary file, then a
  rename) and refused if the file changed since it was read. Task toggles, renames, new notes,
  daily notes and whiteboards all use it.
- The main menu now re-reads only the notes that changed on disk instead of the whole vault;
  a full rescan only happens when a folder moves or a whiteboard changes.
- The graph view and the main menu share one live vault with a single file watcher, so changes
  made while the graph is open show up in the menu too.
- Every action in the main menu, the graph view and the whiteboard is a named command (e.g.
  `graph.open`, `graph.local`, `board.new-card`); each screen's help (`?`) is generated from
  them.

### Fixed

- When the embedded editor can't start, the message no longer repeats itself.
- `Ctrl-q` in the main menu no longer quits; it goes back to the editor.
- Control keys (such as `Ctrl-u`) are no longer typed as letters in text prompts; `Ctrl-u`
  clears the field.

## [0.1.0] - 2026-11-05

The first release.

### Added

- **Main menu**: your vaults (found from Obsidian's own vault list, recently opened ones, or
  added by hand), each vault's notes and whiteboards as a folder tree with backlink counts, and
  the selected note in an embedded Neovim (or Vim) that follows the selection. A rendered
  preview is shown when neither is installed.
- **Links and backlinks**: `Enter` or `gf` on a `[[link]]` in Neovim opens the note, at its
  `#heading` or `#^block`, or creates it in Obsidian's default new-note location. A backlinks
  pane lists every note that links to the open one, with the line it's on.
- **Quick switcher** (`p`) with fuzzy, accent-insensitive matching on names, aliases and paths,
  and **full-text search** (`s`) with highlighted matches; also from Neovim with
  `:ColdLavaSwitcher` and `:ColdLavaSearch`, and from scripts with `cold-lava search`.
- **Graph view**: a live force-directed graph of the vault, drawn with braille characters or, in
  Ghostty, Kitty and WezTerm, as real pixels through the Kitty graphics protocol. Local graph,
  search, tags and unresolved links as nodes, and colour groups read from Obsidian's graph
  settings or made by folder or tag.
- **Whiteboards** that read and write Obsidian Canvas (`.canvas`) files: text, note, link and
  group cards, arrows, colours, undo, and three-way merging so edits made in Obsidian at the same
  time are never overwritten.
- **Agenda, daily notes and tasks for other programs**: `cold-lava agenda --json` (Obsidian Tasks
  format, Dataview fields, global filter), `cold-lava daily` (Daily notes settings and
  templates) and `cold-lava task toggle` (done dates, recurring tasks, a guard against edits in
  between).
- Live updates when notes change on disk, accent-insensitive matching everywhere, and an example
  vault that shows every feature.

[Unreleased]: https://github.com/AlenEngineer/cold-lava/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/AlenEngineer/cold-lava/releases/tag/v0.1.0
