# Architecture

cold-lava has a code editor's architecture, for human notes: an engine that understands a
workspace of plain files, owners that translate each file format the way language servers
translate languages, and frontends that ask the engine and draw. This page is the picture and
the map; [docs/engine.md](docs/engine.md) has the layers as built. The ideas it rests on are [docs/foundations.md](docs/foundations.md), which wins where
any page disagrees.

## The library

Someone keeps notes in many books, written in different languages, some in dialects. Looking
something up means switching languages, and that wears them out. So they hire a translator for
each language. The translators agree on one common language for their reports, and the reader
uses that same language to ask, and to ask for changes. Only the translators ever speak a
book's own language.

That is cold-lava. The rest of the cast keeps the books safe.

| In the story | In cold-lava | Where |
| --- | --- | --- |
| The library | The engine | `crates/engine` |
| The books | Your files: notes, canvases, CSV, anything | the workspace |
| A book's language | A file format, and its flavour | `markdown.rs`, `json_canvas.rs` |
| The translators | Owners: each reads one format into items and links, and writes one change back | `owner/` |
| Staff translators | The built-in owners, Markdown and canvas | `owner/markdown.rs`, `owner/canvas.rs` |
| Outside translators | Owners that extensions bring, answering `file/read` and `file/edit` | `owner/given.rs` |
| The common language | The alphabet (roles, marks, attributes, links) and the types written in it | `alphabet.rs`, `types.rs` |
| The librarian | The core: asks the translators, checks their work, swaps pages in | `core.rs` |
| The catalogue | The index: every item, link and backlink, kept from the translators' reports | `index.rs` |
| The door, and its forms | The workspace gateway's messages | `gateway.rs`, `files.rs` |
| The front desk | The extension host: checks every extension's badge (its permissions) before the librarian hears a word | `crates/host` |
| The readers | The terminal app, extensions that only ask, and a first GUI; a language server to come | `crates/cold-lava`, `crates/gui` |

**The librarian needs no book's language to check a translator's work.** When the librarian
asks for a change to a page (`file/edit`, for an item edit or a rename), the translator answers
with the new page. The librarian hands it back to the same translator and asks for a fresh
report on it. It compares that report with the one it expected: the old report with the asked
change made, both in the common language. If anything else moved, the page is refused
(`given::same`). Whether someone else wrote in the book meanwhile is a comparison of bytes, not a
reading: the bytes it read against the bytes there now (`write::replace`, and `read.rs`'s
`Seen`, which every write moves onto; over the gateway, a fingerprint of them, `rev`, made in
`read.rs`). Swapping the page in is copying bytes, atomically (`write.rs`), and
a page taken out goes to the trash, never deleted for good (`trash.rs`).

**An outside translator is a stranger.** Every report it gives is measured against limits as it
arrives (`bounded.rs`), and an item that doesn't read is set aside on its own, with a note saying
why (`Given::left_out`). A code editor's language server returns edits instead of saving files
for the same reason.

## Where the story is ahead of the code

- The librarian still reads one language itself: Markdown. The core walks notes into items and
  the matcher reads their text. A note's `set-field` is checked by Markdown's own read-back
  (`edit::read_back`), and a tick only by finding a task on its line; neither by `given::same`. Track D (#272) moves Markdown's reading out of the
  core into its owner.
- A translator can still write its own books without the librarian's check: an owner may save
  its own kind of file with `workspace/write`, which is checked only for changes made elsewhere
  (`rev`), and its new text is not read back. Whether to keep that door is the owner's decision
  (#310).
- Not every reader comes through the door yet: the terminal app still reads the index and writes
  some files itself (`crates/cold-lava/tests/one_door.rs` lists each), and track B (#270)
  closes those. The GUI scans the workspace itself, beside the core, and is outside that test's
  lists (#277 deletes it until the GUI is real).
- The librarian keeps some state machines that belong to clerks of their own: the owned-file edit
  and rename, and the nine places it both sends events to subscribers and keeps them for `poll`.
  Track C (#271) moves them out, so
  the librarian only calls on its clerks in order.
- Features live inside the library: tasks, daily notes, recipes, and the shapes and bindings
  that draw data (`shapes.rs`, `bind.rs`, `shows.rs`, `act.rs`) share the engine's crate with
  the core, which is why the engine depends on `canvas`. They are meant to be readers that come
  through the door like any other: the rule and its test first, then the split into crates
  (both #226, decided in #185), after which the core depends on nothing of the project's.
- Characters not hired yet. Decided: **the bookmark**, a name stuck on a passage (an item) the
  first time something refers to it, so the reference survives a reword or a move (#190).
  Proposed, with decisions still open: **the archivist**, who keeps every page the librarian
  replaces (#305); **the logbook**, where every change is written down with who asked and which
  translator wrote it (#306); **dictionaries**, a format described as data, so the librarian
  could check a write by its bytes (#307); and a **ladder of trust** for how far an outside
  translator may write (#308).

## The rules everything stands on

Each is one line here; [foundations.md](docs/foundations.md) has the idea, and the code named is
where it lives. Where the code doesn't meet a rule yet, the line says so.

- **Files are never harmed.** A write is refused when the file changed since it was read, is
  atomic, and a delete goes to the trash (`read.rs`'s `Seen`, `write.rs`, `trash.rs`). What the engine doesn't
  understand in a file, it keeps: a canvas's unknown fields (`json_canvas.rs`, and the board's
  `as_written` in `crates/cold-lava`), and every byte of a note an edit doesn't touch.
- **One door.** Every program reaches the engine through the gateway's messages, and every
  extension behind the host's permission check (`gateway.rs`, `files.rs`, `crates/host`).
  `crates/cold-lava/tests/one_door.rs` lists what still goes around it, and the lists only
  shrink; the GUI, which scans the workspace itself, is outside that test (#277).
- **The core knows data, not syntax.** Formats belong to owners (`owner/`); the core holds
  items, links, types, identity (`identity.rs`), the index and safe writes. Not yet for
  Markdown, which the core still reads itself
  (see above).
- **Outside answers are bounded as they arrive** (`bounded.rs`), and text from outside is escaped
  where it is shown, by one function, `cold_lava_views::shown::escape`, which `say.rs`,
  `cells.rs` and `notice.rs` call.
- **A caller or a leaf.** Every unit only calls its tools in order, or needs nothing the project
  controls; a tool owns its state (foundations.md, "How the code is built"). Not yet
  everywhere: the refactor tracks (#264, #265, #270 to #274) get the rest there.

## Code map

Six crates in the Cargo workspace and the GUI beside it. The dependency direction is frontends →
host → views → engine → canvas: the engine depends only on `canvas` (for its shapes and
bindings, a feature still in its crate: see above), which depends on nothing, and `editor`
stands alone.

- **`crates/engine`**, the library: no user interface. Start with `core.rs` (the librarian),
  `owner/` (the translators: `markdown.rs`, `canvas.rs`, and `given.rs` for an extension's),
  `alphabet.rs` and `types.rs` (the common language), `index.rs` (the catalogue), `gateway.rs`
  and `files.rs` (the door's messages), `identity.rs` (an item's id), `read.rs` (what was read,
  and a file's version), `write.rs` and `trash.rs` (how a page goes in or out).
- **`crates/host`**, the front desk: the extension host every frontend shares, with manifests,
  the permission check and the process transport. Start with `host.rs` (the badge check),
  `manifest.rs`, `registry.rs` and `process.rs`.
- **`crates/views`**, what a notice board (an extension that shows a view cold-lava draws for
  it) may show: lists, tables, images and scenes as data,
  and `shown.rs`, the one escape for text on its way to a screen.
- **`crates/cold-lava`**, the terminal app, the first reader: the menu (`home.rs`), the reader,
  the whiteboard (`board.rs`), the built-in extensions (agenda, table, graph) and the CLI
  (`main.rs`, `say.rs`). `tests/` holds the drive scripts and the one-door guard.
- **`crates/canvas`**, geometry with no dependencies: shapes, regions, rasterizing, hit testing.
- **`crates/editor`**, the headless text editor to come.
- **`crates/gui`**, a first Tauri window, outside the Cargo workspace.
- **`examples/extensions/`**: `open-tasks` (a reader and a notice board: open tasks as a list),
  `csv-table` (a translator, a notice board and a coiner of words: CSV files as tables, with a
  type of its own) and `kanban-board` (a reader and a notice board: Obsidian Kanban boards as
  lanes, which stay Markdown notes). The parts are those of the guide's
  [Which part you play](docs/extensions-guide.md#writing-an-extension);
  [docs/extensions-guide.md](docs/extensions-guide.md) builds on them.

[CLAUDE.md](CLAUDE.md) has the module-by-module notes, for whoever changes the code.
