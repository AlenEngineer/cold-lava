# Extensions: design

Status: **draft**. This is the plan the code is being built towards; sections are marked with
what exists already. For the whole picture (the engine's layers, and how it is separate from
the terminal UI) start with [engine.md](engine.md).

cold-lava aims to be as extensible as Obsidian, and open-source. The product at the centre is
the notes app itself: open a vault, find a note, read and edit it, move between notes through
links. Everything else (the graph, whiteboards, the agenda, even the main menu) should be an
extension of that core.

## Layers

```
 extensions        graph · whiteboard · agenda · daily · menu · third-party…
                   ─────────────────────────────────────────────────────────
 extension host    commands · views · event subscriptions · permissions
                   ─────────────────────────────────────────────────────────
 protocol          JSON-RPC 2.0 messages (requests, responses, notifications)
                   ─────────────────────────────────────────────────────────
 transport         stdio · Unix socket · TCP · in-process … (pluggable)
                   ─────────────────────────────────────────────────────────
 core              live vault · index (links, backlinks, tags, aliases) ·
                   search · safe writes · events
```

Each layer only talks to the one below it, so any one of them can be replaced on its own.

## Core

- **Live vault.** One `Core` owns the vault. It is the only thing watching the folder; when a
  note changes it re-reads only that note and updates the index. Nothing else scans the disk.
- **Index.** Link resolution with Obsidian's rules, backlinks with line and context, tags,
  aliases, full text. (Exists: `index.rs`, `search.rs`.)
- **Safe writes.** Every write goes through one path that writes atomically and can refuse when
  the file changed since it was read. Renames, task toggles, whiteboards and extensions all use
  it, so nothing overwrites an edit made in Obsidian at the same time. (Exists: `write.rs`.)
- **Events.** `note.created`, `note.changed`, `note.removed`, `note.renamed`, emitted once per
  change, whatever caused it (cold-lava, Obsidian, Neovim, git). Anything can subscribe.

## Reading is core, editing is an extension

Reading a vault and moving through it by links is the product, so it works with nothing else
installed. Editing is left to the editor each person already uses: a text editor good enough to
live in is a project of its own, and it would be worse than the one they have.

- **The reader (core).** A Markdown view of the note: headings, lists, tasks and code blocks
  styled, links highlighted and followed from the keyboard, backlinks beside it. It replaces the
  plain-text preview the menu shows when no Vim is installed.
- **The editor (an extension slot).** An editor extension provides `note.edit`. There are three
  levels, and an editor can stop at any of them:

  | Level | What you get | Provided by |
  | --- | --- | --- |
  | Full screen | The editor takes over the terminal until it quits | Built in, for any `$EDITOR` |
  | In a pane | The editor runs beside the tree, as Neovim does today | Any TUI editor, through the "program in a pane" view |
  | Bridged | Follow links and open backlinks from inside the editor | One extension per editor |

- **Neovim is the first editor extension.** It keeps the Vim script and the meaning of the
  private OSC messages (`ESC ] 5379 ; kind ; payload BEL`) that make link following work inside
  it. The core keeps only the pane (`embed.rs`), which third-party views need as well. Without
  the Neovim-only checks, Helix, Kakoune or micro embed in the pane too, without the bridge.
  (Exists: `crates/cold-lava/src/ext/nvim.rs`.)
- **Saves need no cooperation.** The core's file watcher sees every save, so an editor only has
  to write the file. Telling the core about a save is a hint that makes it show up sooner.

### Markdown flavours

Markdown is not one language. GitHub, Obsidian and plain CommonMark each read the same file a
little differently, so the reader reads a note in a **flavour**: a named set of syntax features
(`tables`, `task-lists`, `wikilinks`, `callouts`, `highlights`, `tags` and so on). Syntax a
flavour doesn't have stays plain text. Under `github`, `==x==` shows its equals signs.

- **Built in.** `commonmark` (only frontmatter is hidden), `github` (tables, task lists,
  strikethrough, footnotes, `> [!NOTE]` alerts) and `obsidian` (GitHub's, plus wikilinks,
  embeds, callouts, highlights, `%%comments%%`, tags, block ids and math). A vault with an
  `.obsidian` folder reads as `obsidian`, any other folder as `github`.
- **Your own.** `config.toml` defines more, as a built-in flavour with features added or
  removed, and picks the one to use.
- **Later, extensions.** A flavour decides what is parsed. What it leaves unrendered, such as a
  fenced code block in `mermaid` or a callout type of a plugin's own, an extension can draw
  through `view/render`.

## Notes as data

*(design; items for tasks, lists, headings and blocks exist: `crates/engine/src/items.rs`)*

You write notes, not data. But Markdown already has types, and Obsidian's community has settled
how to read them: a checkbox is a task, with dates written the Tasks plugin's way
(`📅 2026-11-12`) or Dataview's (`[due:: 2026-11-12]`); frontmatter is typed properties; a table
is rows of records; `key:: value` is a field; a heading starts a section. The core reads every
note this way, keeps it current as files change, and lets extensions ask for it and change it.
An extension that lists every open todo in the vault, or a reading list built from a table, is
then a query and a view, not a parser of its own.

- **Items.** One pass per changed note, in the core, gated by the vault's flavour (no
  `task-lists`, no tasks). Each item has a kind (`task`, `list`, `heading`, `property`, `row`,
  `field`, `link`, `tag`, `block`), a note, a line range, the heading path it sits under
  (`section`), and its own fields: a task's status symbol and type, its dates, recurrence,
  priority, tags and links; a property's key, Obsidian type (`.obsidian/types.json`, else
  inferred) and value. A table's rows are items of their own, so one row can change without the
  rest. Items are parsed on first use and cached per note, so the `search` and `rename` commands
  pay nothing for them.
- **Identity.** An item's `id` comes from its note and its anchor: the author's `^block-id`,
  a property's key, a heading's path, else a hash of its text without status and dates (and the
  nth identical one). It survives ticking, re-dating, re-indenting and lines added above, and it
  is the same after a restart and in every process. Editing a task's words reads as one removed
  and one added. Separately, `rev` hashes only the item's own lines: it says *which version*, and
  edits refuse a stale one. The core never adds `^ids` by itself.
- **Reading.** `items/query` takes a `kind` and a fixed filter (notes under a folder or with a
  tag, status, a date range on `due`, `scheduled`, `start`, `done` or `agenda`, text, a property
  key), returns items ordered by note and line, and pages. No query language in the core: an
  extension that wants Dataview's can compile it to this. Reads are answered at once; they don't
  change anything, so effects stay queued.
- **Hearing changes.** `items/subscribe` (kinds and the same filter), then `items/changed` with
  what was added, updated and removed, by id. After a full rescan, `items/reset` says to query
  again.
- **Changing.** `items/edit` names an item, its `rev` and ops: `toggle`, `set-status`,
  `set-date`, `set-property`, `set-cell`, `set-field`, `replace`, `insert-after`. The core finds
  the item in the note as it is now, checks the `rev`, splices only its lines, and writes through
  `write::replace`. If something else in the note changed, the edit still applies; if this item
  changed, it is refused as `changed`. Obsidian's rules stay in the core: Tasks' done dates and
  recurring tasks, emoji or Dataview format as written, and YAML edited line by line, never
  re-serialised (nested values are refused). If the note is open in the editor, it is saved
  first.
- **The agenda becomes an extension** on top: a list view grouped by day, kept live by a
  subscription, ticking through `items/edit`. `cold-lava agenda --json` and `task toggle` keep
  their output, as thin clients of the same query and edit.

## Types

*(design; nothing of it exists yet)*

Markdown gives the primitives: blocks (paragraph, heading, list item, quote, code fence, table
row, a note's frontmatter), inlines (text, link, tag), and value types (text, number, date,
datetime, duration, bool, link, tag, enum). A **type** gives them meaning, declared in TOML with
no code, by you, by a vault, or by an extension. The built-in task is written in the same
language anyone else uses.

A type is a **schema** (its fields, their value types, how they look, and native actions by
name) plus one or more **shapes**, the ways one shows up in a note:

```toml
[types.book]
fields = { title = "text", author = "link", rating = "number",
           status = { type = "enum", values = { "to read" = "todo", reading = "reading", read = "done" } } }
style.rating = { icon = "★" }

[[types.book.from]]                  # a row of any table whose header has Title and Author
on = "table-row"
where.header = ["Title", "Author"]
key = ["title"]
fields = { title = { column = "Title" }, author = { column = "Author" },
           rating = { column = "Rating" }, status = { column = "Status" } }

[[types.book.from]]                  # or a note whose frontmatter says type: book
on = "note"
where.property = { type = "book" }
fields = { title = { property = "title", note = "name" }, author = { property = "author" },
           rating = { property = "rating" }, status = { property = "status" } }

[types.estimate]                     # ~30m anywhere in text
on = "inline"
match = '(?:^|\s)~(?<v>{duration})\b'
fields.minutes = { capture = "v", type = "duration" }
write = " ~{minutes}"
style.minutes = { badge = true, icon = "⏱" }
```

- **Shapes** say which primitive (`on`), cheap conditions (`where`: table header, frontmatter
  property, tag, folder, section, fence language, inside another type), an optional pattern,
  and where each field is read. A note's types are found in the one pass that already reads it.
- **Sources know how to write.** `marker = "📅"` (a glyph and a value), `inline = "due"`
  (Dataview's `due::`, `[due:: …]`, `(due:: …)`), `flags` (which glyph is present), `column`,
  `property`, `capture` (a pattern's group), the fence's `info`. Each can put a value back: only
  the value's span changes, glyphs and brackets stay as written, a missing field is added the
  way the line already writes them, a table is never re-padded, frontmatter is edited line by
  line and never re-serialised. Some are read-only (`rest`, the leftover text; tags and links in
  text; the note's name; child items).
- **Every write is checked.** After an edit the engine re-reads the note in memory, and the item
  must read back exactly the value set with nothing else changed, or the edit is refused before
  anything is written. That makes your own patterns safe to write through.
- **Values** parse by type: dates in moment.js formats (as daily notes use), durations like
  `1h30m`, links resolved by Obsidian's rules when queried or drawn, enums mapping written forms
  to names. A few **settings providers** (`${tasks.statuses}`, `${tasks.global-filter}`,
  `${daily.format}`) let a definition follow the vault's own settings.
- **Deliberately not there:** formulas, joins, conditions on other items, matching across
  blocks, look-around in patterns (Rust's regex has none, so no pattern can hang), and new
  grammar Obsidian would show raw. Behaviour is native code named in `actions`: the task's
  definition reads and styles tasks, while ticking (done dates, recurrence) stays
  `tasks.toggle`, written in Rust.
- **Conflicts.** One type per note, one per block, any number inline. A typed block replaces its
  structural kind, as a task replaces a list item today. Within a level the winner is: higher
  `priority`, then a subtype (`extends`) over its base, then the more specific shape, then
  origin (vault, user, extension, built-in). A subtype still answers to its base, so
  `[types.idea] extends = "task"` with `where.tag = ["idea"]` stays in the agenda. Inside a
  code fence only types that name it with `where.inside` match, so tasks are never read from
  code. `cold-lava types --check` lists blocks two types wanted, and which won.
- **Identity.** The same scheme as tasks today: an author's `^block-id`, else the shape's `key`
  fields under the parent's key; `rev` hashes the item's own lines (a row's line, a note's
  frontmatter, a fence). An inline item's parent is the block it sits in. Existing ids don't
  change.
- **Where definitions live,** in load order: built in (`types/*.toml` in the binary), extension
  manifests (named `<extension>.<name>`), `[types.*]` in `config.toml`, and
  `<vault>/.cold-lava/types.toml`, which travels with the vault. Built-in names can be extended
  but not redefined. Mistakes are collected like `config.toml`'s: one line each, the broken type
  is dropped and the rest load. `cold-lava types VAULT` lists every type, its fields and where
  they're read, how many items it found and how long matching took. A file starts with
  `version = 1`.
- **Looks.** A small, closed set: a colour by name (Obsidian's six canvas colours, plus accent and
  muted), badge, icon, dim, strike, bold, hide, a link to follow, and the same by enum value.
  Colours are names, so a light terminal is one table in the frontend, never a property of a
  type. The reader draws them, and every view uses the same looks.
- **The table view** comes free with every type: `table.open` picks a type, and a built-in
  extension queries its items and sends a table (columns typed by the schema, rows by item id).
  Sorting and filtering happen in the frontend; editing a cell becomes `items/edit set-field`,
  refused and refreshed if the item changed elsewhere.
- **Performance.** A note is gated once (folder, tags, frontmatter keys), blocks are checked only
  against types for their primitive, and one `RegexSet` per primitive tells which types could
  match before any pattern runs. Targets on 10,000 notes with 30 types: the whole vault matched
  in under 1.5 s on first use, one changed note in under 2 ms.

## File types

*(design)*

Markdown is the engine's own format. Any other kind of file (`.canvas`, `.kanban`,
`.excalidraw`, `.csv`) can have an **owner**: an extension that says so in its manifest.

```toml
[[file-types]]
extension = "kanban"       # owns *.kanban files
title = "Kanban board"
new = "kanban.new"         # the command that makes one
```

- **Opening.** Selecting a `.kanban` file in the tree, clicking it on a canvas or following
  `[[Projects.kanban]]` sends its owner `file/open {path}`. The owner reads it through the vault
  gateway (`vault/read` gives `{text, rev}`), parses its own format, and describes a view
  through the view gateway (a scene, a table, Markdown). A file with no owner shows its name, as
  now; `vault/files {extension}` lists files of a kind, for pickers.
- **Saving and creating.** `vault/write {path, text, expect: rev}` is refused if the file changed
  since it was read, so the owner re-reads, merges and tries again. `vault/create` places a new
  file where Obsidian would put it and never overwrites. Outside changes arrive as
  `file/changed`, from the core's one watcher.
- **Joining the vault.** An owner can contribute its file's **items and links**. A kanban card
  becomes an item of the owner's type (`kanban.card`), queried and listed with Markdown tasks,
  and edited through its owner. A card's `[[Lava]]` is reported as a link that the core
  resolves by Obsidian's rules, so `Lava`'s backlinks list the board, the graph draws the
  edge, and renaming `Lava` updates the board through its owner. Other file types are then part
  of the vault, not opaque files only one extension understands.
- **Previews.** `![[Projects.kanban]]` in a note, or a file card on a whiteboard, asks the owner
  for a small view of the file.
- **The whiteboard is the first owner:** `.canvas`, drawn as a scene, merged three ways on save,
  contributing its cards and their links. Images get a view kind of their own (`image`, drawn
  with Kitty pixels where the terminal has them), since every frontend can show a picture.

## Commands

Every action has a name, such as `graph.open`, `note.rename`, `daily.today`. Commands are what
keys, the command palette, `cold-lava run <command>` and extensions all call. Keys are
configurable in `~/.config/cold-lava/config.toml`.

Extensions register their own commands in their manifest (`commands`). The menu has one command
table at runtime: the core's commands, then each extension's. Keys, the palette, the help,
`cold-lava keys` and the `[keys]` section of `config.toml` all read that table, so an extension's
command is configured, listed and reported exactly like a core one. *(Exists:
`commands::menu`, which `Registry` builds when it loads extensions.)*

- **Ids are namespaced.** An extension's command id is `<extension name>.<something>`, such as
  `nvim.restart`. The core leaves out a command whose id is outside that namespace or is already
  a command, and reports why in the status line and in `cold-lava keys`.
- **Default keys are a suggestion.** A default key that another command already has, or that
  doesn't parse, is dropped and reported. The command stays, without that key.
- **Bound in the menu only.** While a pane has focus every key goes to its program, except
  `pane.leave`. Extensions' commands run from the menu and the palette.
- **Running one.** A key or the palette sends the extension `command/run` with the command's
  `id` (and `null` args). An extension's own `command/run` goes through the same table, so it can
  run core commands and other extensions' commands as a key would.

Editing works the same way. `note.follow-link` is a core command: the editor runs it
(`command/run`) when you press Enter on a link. `note.edit` is the command the active editor
extension will fill in.

## Transport: an abstraction from the start

Extensions run as separate processes. How bytes travel between cold-lava and an extension is
hidden behind one interface, so new kinds of connection can be added without touching the
protocol, the host or any extension:

```rust
/// A bidirectional, message-framed connection to one extension.
pub trait Transport: Send {
    /// Send one complete message (a serialized JSON-RPC object).
    fn send(&mut self, message: &[u8]) -> io::Result<()>;
    /// The next complete message, or None when the other side closed the connection.
    fn recv(&mut self) -> io::Result<Option<Vec<u8>>>;
    fn close(&mut self) -> io::Result<()>;
}

/// Knows how to open a transport to an extension described by its manifest.
pub trait Connector {
    /// The `transport` value in a manifest this connector handles, e.g. "stdio".
    fn kind(&self) -> &'static str;
    fn connect(&self, manifest: &Manifest) -> io::Result<Box<dyn Transport>>;
}
```

- **Framing belongs to the transport.** Over stdio and sockets, messages are framed like LSP
  (`Content-Length: N\r\n\r\n` then N bytes). A WebSocket transport would use its own frames. The
  protocol layer only ever sees whole messages.
- **First implementation: stdio.** cold-lava starts the extension's command and talks over its
  stdin/stdout; stderr goes to a log.
- **Later, without other changes:** Unix sockets (connect to a long-running extension or
  daemon), TCP, named pipes on Windows, or an in-process transport for extensions built into
  cold-lava, which lets built-in and external extensions use exactly the same code path.
- The manifest picks the transport; connectors are registered by kind, so adding one is a new
  `Connector` and nothing else.

## Protocol

JSON-RPC 2.0. Both sides can send requests and notifications.

- The host calls the extension: `initialize` (capabilities, the API version and the
  `commands` it registers), `command/run` (`id`, `args`: one of its commands),
  `view/render`, `shutdown`.
  - The editor gets `editor/open` (a note, and a `jump` to a line, heading or block),
    `editor/save` and `editor/renamed` (`from`, `to`).
  - An extension hears from a pane it opened through `pane/message` (`kind` and `payload`, what
    its program wrote as `ESC ] 5379 ; kind ; payload BEL`) and `pane/exited` (with an `error`
    when the program never started).
- The extension calls the host: `vault/notes`, `vault/read`, `vault/write` (through the safe
  write path, with the expected previous content), `links/backlinks`, `search/text`,
  `items/query`, `items/subscribe` and `items/edit` (see "Notes as data"), `ui/notify`, `ui/openNote`, `events/subscribe`, and:
  - `command/run`: run any command in the menu's table by `id` with `args`, as a key would: a
    core command or another extension's. Core commands ignore `args`, except
    `note.follow-link` (`from`, `link`), which only extensions can run, and `search.open`
    (`query`, optional). An unknown id is reported in the status line.
  - `editor/opened`: the editor shows this note now. It is a report, not a request, so the
    host never answers it with `editor/open`, which would echo.
  - `vault/saved`: a note was just written, so the host reads it now instead of when the file
    watcher notices.
  - `pane/open` (a `pane` id the extension picks, and the program with its arguments),
    `pane/input` (text to type into the pane) and `pane/close`.
- The host notifies subscribers: `event/noteChanged` and friends.

The method set is versioned (`apiVersion` in `initialize`), so old extensions keep working.
Paths are absolute and inside the vault. A message's tag is its method name, for example
`{"method": "pane/input", "params": {"pane": 1, "text": ":w\r"}}`.

Built-in extensions already use, in-process (`crates/cold-lava/src/ext/mod.rs`): `initialize`
(with `commands`), `command/run`, `editor/open`, `editor/save`, `editor/renamed`, `pane/message`
and `pane/exited` from the host, and `ui/notify`, `command/run`, `editor/opened`, `vault/saved`,
`pane/open` and `pane/input` from the extension.

- **Saves are not acknowledged yet.** `editor/save` gets no answer, so before it reads the files
  (to rename a note, to show the reader, to quit) the core waits 300 ms. A later version answers
  `editor/save` once the files are written.
- **`editor/renamed` is temporary.** Until extensions can subscribe to events, a rename tells the
  editor directly. Then the editor follows `note.renamed` like any other subscriber.

### Panes and keys

The core owns every pane. It starts the program an extension names in `pane/open`, draws it,
resizes it and sends it the keys you type. The extension only types text of its own
(`pane/input`) and hears what the program sends back. Extensions never see keystrokes.

While a pane has focus, the core keeps one key: `pane.leave` (`Ctrl-q`) takes you back to the
menu. It can be moved in `config.toml` but not unbound, so every pane has a way out. The core
shows one pane at a time and answers a second `pane/open` with `pane/exited`.

## Manifest

```toml
# .cold-lava/extensions/word-count/extension.toml (per vault)
# or ~/.config/cold-lava/extensions/word-count/extension.toml (for every vault)
name = "word-count"
version = "0.1.0"
api = "1"
transport = "stdio"
command = ["python3", "main.py"]
permissions = ["read"]          # "read", "write", "network"…

[[commands]]
id = "word-count.show"           # must start with the extension's name
title = "Word count of this note"
keys = ["W"]                     # default keys; config.toml can change them
```

## Views

Extensions can't draw into cold-lava's terminal directly. A view is one of:

- **Declarative:** a list, a tree or Markdown, described as JSON and drawn by cold-lava (fits most
  panels: outlines, task lists, search results). Rows carry item ids, so a command run on the
  selected row knows what it is about.
- **A terminal program in a pane:** the extension names a command and cold-lava runs it in a pane,
  the same way the main menu embeds Neovim. Full freedom, any TUI library, but no pixel graphics:
  the pane goes through a terminal emulator.
- **A scene** *(design)*: nodes, edges and labels that the core draws, for the graph, the
  whiteboard, and anything shaped like them (a mind map, a kanban board, a timeline).

### Scenes

The graph and the whiteboard are both nodes and edges with positions, zoom, pan, selection and
dragging. The extension says what exists and what it means; the core does everything that runs
at frame rate, so nothing crosses the connection per frame and a slow extension never makes
zooming wait.

- **The core:** the camera, hit-testing, selection and moving it by arrow or along links,
  dragging and resizing, edges attached to card sides, label placement, dimming around the
  selection, the local-graph neighbourhood, search over labels, help and the status line, and
  drawing in braille or Kitty pixels. Layouts: `fixed` (positions are the extension's data) and
  `force` (the core simulates; Barnes–Hut belongs here).
- **The extension:** which nodes and edges exist, their shapes (`dot`, `card`, `group`), labels,
  colours, a card's Markdown or the note it shows, the legend; the document, saving and merging
  it; what its commands mean.
- **Messages.** `view/open` with the whole scene, then `scene/update` deltas by id (upsert,
  remove, style), each numbered. Back come `scene/selected`, `scene/activated` (Enter or
  double-click), `scene/clicked` on empty space, `scene/moved` once on drop (fixed layouts only),
  and the results of `scene/pick` (choose a node, for connecting) and `ui/input`.
- **Keys.** The core binds navigation (arrows, pan, zoom, Tab, Enter, Esc, `?`, `q`) as
  `scene.*` commands. Everything else is an extension command scoped to its view type
  (`CommandSpec.view`): it runs with the selection in its `args`, and its keys only clash within
  that view.
- **Text.** Card text is typed into a core input anchored on the card (`ui/input`), or in
  `$EDITOR`; the extension hears the result once. Keys never reach it.
- **Positions.** In a force layout the core owns them and keeps them by id across updates. In a
  fixed layout the core moves dragged nodes itself, reports the drop once, and the extension's
  next update is authoritative.
- **Colour groups** move with the graph extension, which reads `graph.json` through the core.
  The **three-way merge** moves with the whiteboard extension, which reads with a revision and
  writes with `vault/write` expecting it, merging and retrying when refused.

The graph moves first: it's read-only, already draws through a backend-neutral `Scene`, and is
where the performance risk is (a 5k-note vault is about 1.3 MB of `view/open`, sent once). The
whiteboard follows once the graph has carried the protocol; if input and picking make the scene
protocol grow into a UI toolkit, keeping the whiteboard in the core is a fair outcome. Scenes
start full screen; one beside the tree waits for a way to keep Kitty images under overlays.

## Built-in features become extensions

The graph, whiteboards, agenda, daily notes and rename move onto the same `Extension` interface
(in-process, through the in-process transport). If our own features fit the interface, it is
good enough for everyone else.

A built-in extension implements the `Extension` trait (`crates/cold-lava/src/ext/mod.rs`)
instead of reading messages from a transport. The trait is the protocol: each of its methods is
one host→extension method, and the `Host` it is given has one method per extension→host
method. Every type that crosses derives serde, and tests pin its JSON, so an extension can move
to its own process without changing shape. What an extension asks for is queued and applied
after its call returns, so it never holds the menu's state.

## Order of work

1. Core: safe writes, live vault, events. *(done)*
2. Commands, command palette, configurable keys. The Markdown reader can land alongside, since
   it's core.
3. The `Extension` interface; move built-in features onto it, starting with the Neovim editor.
   *(in progress: the interface, extension commands and the Neovim editor exist)* Then, in
   small PRs each checked with `cold-lava drive`:
   1. Notes as data: tasks, lists, headings and block ids, with `agenda` and `task toggle`
      rebuilt on them and `cold-lava items VAULT KIND`. *(done: `items::extract` reads a
      note in one pass, `Vault::items` caches it per note, ids and revs are pinned by tests,
      and the item JSON is pinned for `items/query`)*
   2. Ids, revs and safe edits; in the reader, `x` ticks the task under the cursor.
   3. The protocol for items, and reads through `Host`.
   4. The declarative list view, and the agenda as an extension.
   5. Types, in small steps: the loader and `cold-lava types` (with
      `example-vault/.cold-lava/types.toml`: `volcano` notes and `eruption` table rows);
      primitives with spans; typed blocks; inline types; `set-field` with the read-back check;
      the task as a definition, first in shadow mode beside today's parser, then switched over;
      looks in the reader; the table view.
   6. File types: owners in manifests, `file/open`, `vault/files`, `vault/create`, items and
      links contributed by owners; the whiteboard as the first owner.
   7. Scenes: `drive` learns drag and scroll, and scripts pin today's graph and whiteboard. The
      scene view is extracted from the graph, runs in the menu's loop, and the graph becomes an
      extension. Then the whiteboard.
4. Transports (stdio first), JSON-RPC, manifests; an example extension in Python and docs.
   Extension authors test their extension's UI with `cold-lava drive` scripts (snapshot tests).
5. Installing extensions (`cold-lava ext install <git-url>`), permissions, versioning.

## Compatibility with Obsidian plugins

Obsidian plugins are JavaScript that draws into Electron, so running them unchanged isn't a
goal. Data compatibility is: cold-lava keeps reading and writing the same files and settings
(Tasks, Daily notes, graph colour groups, Canvas). A TypeScript extension could reuse a plugin's
logic that doesn't depend on its UI.
