# Changelog

All notable changes to cold-lava are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may
change commands, keys and file formats; patch versions only fix things.

## [Unreleased]

### Added

- `cold-lava drive SCRIPT -- ARGS` runs cold-lava in a hidden terminal, types keys from a script,
  waits for text and checks the screen (`wait-for`, `expect`, `exits 0`, …). A failing step
  names its line and prints the screen. It drives the real binary, embedded Neovim included, with an empty home folder so
  it doesn't read your config or add to your list of vaults (`--real-home` to use yours).
  After a lone Esc it waits for the program to answer before typing on, so `keys esc q` isn't
  read as Alt-q on a slow machine.
- `COLD_LAVA_EDITOR=none` opens notes in the reader even when Neovim is installed.
- Link hints: `f` (`note.hints`) in the reader, or in the menu while the reader shows a note,
  labels every link on screen and every backlink in view with one or two letters from the home
  row. Typing a label follows that link the way `Enter` does (Back returns), `Esc` or a letter
  no label has cancels, and after the first letter of a pair only the labels it starts stay.
- Outline: `o` (`note.outline`) in the reader, or in the menu while the reader shows a note,
  lists the note's headings indented by level, with the one you're reading marked. Type to
  filter (fuzzy, accents ignored), `↑↓` or `Ctrl-n`/`Ctrl-p` to choose, `Enter` to jump there
  (Back returns to where you were in the note), `Esc` to close.
- Peek: `K` (`note.peek`) on the reader's selected link shows where it leads in a popup under
  the link, without going there: the note, drawn by the reader in the vault's flavour and
  scrolled to the link's `#heading` or `#^block`, sized to its text up to 40% of the screen.
  The reader's keys scroll it, `Tab` / `Shift-Tab` move to the next or previous link and the
  peek follows, `Enter` goes there (Back returns), `Esc` or `K` closes it. A missing note says
  `Enter` creates it, a URL says `Enter` opens it in the browser, and a link to an attachment
  names its type. On a backlink, `K` peeks at the linking note at the line with the link, and
  `Tab` sweeps through the backlinks.
- The reader's title shows where you are in the note: `Concepts/Lava › Temperature`, the
  headings above the top line, cut from the left with `…` when too long. A first heading that
  only repeats the note's name is left out.
- `]` / `[` (`reader.next-heading`, `reader.previous-heading`) jump to the next heading, and to
  the start of the section you're in or the heading before it.
- Back and forward, like a browser's, across the menu, the reader, search, the graph and
  whiteboards: `Ctrl-o`, `Alt-←` or `Backspace` (`history.back`) returns to where you were
  before following a link or opening a note, a search result, the graph or a board, with the
  reader's scroll position and selected link or the search's words; `Alt-→` (`history.forward`)
  goes forward again. The status line shows where they lead (`Ctrl-o ⟵ Lava`).
- `cold-lava items VAULT KIND [--json]` lists the vault's tasks, list items, headings or blocks
  with a `^id`, one per line (`note:line` and a short form), or as JSON lines with each item's
  id, line range, section, parent, block id and data. A task's id stays the same when it is
  ticked, re-dated, re-indented or moved down by lines added above it.
- Types, the first step: `[types.NAME]` in `config.toml` and `.cold-lava/types.toml` in a vault
  (starting with `version = 1`) define what notes, table rows, list items and other blocks mean:
  fields with value types, where each is read, and how it looks. `cold-lava types [VAULT]
  [--json]` lists every type with where it comes from, its shapes and fields, and whether each
  field can be written back, then one line per mistake (the broken type is left out, the rest
  load, exit code 1). The example vault defines `volcano` notes and `eruption` table rows.
- Types find their notes and blocks: `cold-lava items VAULT TYPE` lists the notes, table rows,
  list items, headings or paragraphs a type matches, with every field of the type read and
  parsed (`null` when missing or when it doesn't read as the field's type), and `cold-lava
  types VAULT` says how many items each type found and how many values didn't parse. One type
  wins per note and per block (priority, then subtype, then the more specific shape, then
  origin, then name); a type that `extends` another is also listed under it. Tasks stay tasks.
- Which key: `,` (`app.keys`, `graph.keys`, `board.keys`) opens a popup at the bottom of the
  menu, the reader, the graph or a whiteboard that lists the commands you can run there now,
  grouped (move, go, links, note, view, app, extensions), each with its key and a short name.
  Typing a listed key runs that command, `Esc` closes it. The groups and names live in the
  command tables, so the popup follows `[keys]` in config.toml. `?` still opens the full help.

### Changed

- The status line shows only a few keys and `, keys` / `? help`; the reader's no longer runs
  past the edge of the screen.
- `note.open-in-obsidian` moved from `o` to `O`: `o` opens the outline, which you use far more
  often while reading. To get the old key back, give the outline another key too (two commands
  on one key both keep their defaults): `"note.open-in-obsidian" = "o"` and
  `"note.outline" = "ctrl+l"`. The graph keeps `o` for Obsidian.
- `Ctrl-o` is now Back (`history.back`) instead of a second key for the quick switcher, which
  keeps `p`. To get the old key back: `"note.switch" = ["p", "ctrl+o"]` and
  `"history.back" = ["alt+left", "backspace"]` in `[keys]`.
- Building from source is now `cargo install --path crates/cold-lava`: the repository is a
  Cargo workspace, with the engine as its own crate (`cold-lava-engine`) beside the terminal UI.
- `agenda` and `task toggle` read tasks the way Obsidian and the Tasks plugin do, from the same
  parse as the rest of cold-lava:
  - Tasks in callouts and quotes (`> - [ ] …`) are listed, and `task toggle` ticks them.
  - Tasks in `~~~` code blocks, indented code blocks, `%%comments%%`, or text that only looks
    like a list (an indented line continuing a paragraph) are no longer listed.
  - A task's text leaves out a trailing `^block-id` and inline `%%comments%%`.
  - Custom statuses follow the Tasks plugin's status settings (`[?]` set to "todo" counts as
    open).
  - A malformed date (`📅 2026-13-45`) is ignored, so the task is filed under its next date
    instead of being left out.
  - Notes under `node_modules` are skipped, as everywhere else in cold-lava.
- `agenda` is about six times faster on large vaults: the daily-note date format is compiled once
  instead of once per note.

### Fixed

- Following a link to an attachment (`[[map.pdf]]`, `[map](Maps/map.pdf)`) no longer creates a
  note named `map.pdf.md`. The file opens in the system's app (`xdg-open`, `open` on macOS) when
  it's in the vault, found the way Obsidian finds it: by path, next to the note, then by name
  anywhere; a link to a canvas opens it in the whiteboard. A missing attachment says
  `map.pdf isn't in the vault` and nothing is created. Notes with dots in their names
  (`Mount St. Helens`) are still notes: only known attachment types (images, audio, video, PDF,
  canvas, base) count. The peek says whether `Enter` opens the attachment or it's missing.
- The whiteboard no longer overwrites a canvas file changed outside cold-lava between its merge
  and its save, or one it couldn't read (for example while another app was halfway through
  writing it). Your edits are kept and saved once the file can be read, and quitting first warns
  that they aren't saved yet.

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
- `nvim.restart` restarts the embedded Neovim from the menu: it saves every note, quits, and
  starts again on the note it showed. Run it from the palette (`:`), or give it a key in
  `config.toml`. It also starts Neovim again after it was closed.
- Extensions register commands like the core's. Their commands are in the palette and the help,
  `cold-lava keys` lists them, and `[keys]` in `config.toml` rebinds them by id with the same
  rules. An extension's command id must start with its name, such as `nvim.`.

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
