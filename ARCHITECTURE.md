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

**The librarian needs no book's language to check a translator's work.** When the librarian asks
for a change to a page (`file/edit`, for an item edit or a rename), the translator answers with
the new page. The librarian hands it back to the same translator and asks for a fresh report on
it. It compares that report with the one it expected: the old report with the asked change made,
both in the common language. If anything else moved, the page is refused (`given::same`).
Whether someone else wrote in the book meanwhile is a comparison of bytes, not a reading: the
bytes it read against the bytes there now (`read.rs`'s `Seen`; over the gateway, a fingerprint
of them, `rev`, made in `read.rs` too). `Seen` is the one type for it: most writes and deletes
go through it, and the rest move onto it in track A step A5 (#265). Swapping the page in is
copying bytes, atomically (`write.rs`), and a page taken out goes to the trash, never deleted
for good (`trash.rs`).

**A translator changes what a book says, never its name or its place.** Renaming, moving and
removing a book are the librarian's: the librarian plans a rename and asks each translator only
how its own pages should read once a name they mention has changed (`Core::relink`). Owning a
kind of file grants no rename, move or delete, and a translator can't write whole a book another
translator reads (#323). Removing is a reader's request under its own badge: any extension the
person gave `files.delete` may send a file to the trash, whoever reads it, and the trash keeps
it. Renaming and moving have no message at the door yet; when they come (#193), they come as a
reader's requests under the same rule.

**The librarian has clerks.** If the librarian did everything alone, it would never keep up, so
each job that holds its own state belongs to a clerk: a tool the librarian calls in order and
never reaches inside. Some are already at their desks, some still work from the librarian's own
desk (`core.rs`), and some aren't hired yet:

| Clerk | Job | Where |
| --- | --- | --- |
| The front desk | checks an extension's badge before the librarian hears a word | `crates/host/src/host.rs` (split out of one file in C1, #312) |
| The report keeper | keeps each translator's last report between visits, and asks again when a book changes or a kept report no longer reads | `owner/cache.rs` |
| The edit desk | carries an edit to a translator, waits for its answer and reads it back | `owner/edits.rs` holds the edit; the waiting and checking still sit in `core.rs` (track C step C2) |
| The town crier | announces each change once, in order, to whoever listens | still in `core.rs` (`broadcast`, `tell`; track C step C3) |
| The shelver | shelves, moves and trashes books, and keeps their shelf marks (symlinks; see References), reading no language | partly at its desk: `write.rs` (`create`, `move_file`) and `trash.rs`; the door's guard on shelf marks comes with #353, and the watcher's linked notes are #357 |
| The archivist | keeps every page the librarian replaces | not hired yet (#305, after track C) |
| The logbook keeper | writes down every change, who asked and which translator wrote it | not hired yet (#306, after track C) |

**An outside translator is a stranger.** Every report it gives is measured against limits as it
arrives (`bounded.rs`), and an item that doesn't read is set aside on its own, with a note saying
why (`Given::left_out`). A code editor's language server returns edits instead of saving files
for the same reason.

## References

A book that mentions another book writes a **reference** in its own language: `[[Lava]]` or
`[text](Lava.md)` in a note, a file card on a canvas, a link column in a CSV file, a web
address. Only the book's translator reads a reference or rewrites one (the Markdown owner's
`links`, the canvas owner's file cards, the links in an outside translator's `file/read`; for a
rename, `Core::relink`).

A reference has two halves, and different people own them. **Which book** is the catalogue's.
The librarian finds the book by Obsidian's rules, whatever language either book is in
(`resolver.rs`). **Where in that book** (`#heading`, `#^anchor`, `#page=4`) is the target's
translator's, because only that translator reads the target. So a reference is one of three
kinds:

| Kind | Leads to | Checked by |
| --- | --- | --- |
| Internal | a place in the same book | the book's own translator |
| External | another book, in any language | the catalogue for the book, the target's translator for the place |
| Foreign | a web address, or a file no translator reads | nobody; it is kept as written |

A kind of file gets places that can be checked only by getting a translator: an extension that
owns `.pdf` would make `#page=4` checkable. Nothing stands in for a missing translator. Each
translator declares the place forms it understands (Markdown: headings and block anchors; a
canvas: card ids; CSV: row keys), so the librarian can tell a reference that points nowhere
from one it can't check.

**A symlink is a shelf mark, not a reference.** It is written in no book. It says where a book
stands, so it is the shelver's, never a translator's. A note linked from another folder is one
book with two shelf marks: one entry in the catalogue, with the link as another name for it. A
link that leads off the shelves (outside the workspace) is a book the library doesn't hold. The
person may open it, but the library never lends it to a visitor (an extension), nor anything in
the staff room (`.git`, `.obsidian`). So the door never follows a symlink, and the person's own
saves (the whiteboard, the menu, an item edit) write through one to the file it leads to (#353).

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
- No translator checks a reference's place half yet, and none declares its place forms. The
  resolver splits `#heading` and `#^block` off every reference by Markdown's rule, whatever
  language the target is in (`split_link`; the canvas owner reads a file card's `subpath`
  through it too, in `canvas::anchors`). The reader uses the place only to jump there, and
  `cold-lava tidy` reports a reference whose book is missing, never one whose heading is gone. A
  file card's place half is an attribute of its card, not part of its link (#359). References
  to items wait for #94.
- Not every shelf mark is the shelver's yet. The scan skips a symlinked note, but the watcher
  reads it as a note of its own, so its tasks show twice and the link's copy goes stale, and an
  item edit on it never meets the door's path checks (#357). Until #353 merges, the door follows
  a symlink that stays inside the workspace, and a save replaces a linked note with a copy
  (#349).
- **The bookmark** is placed but not yet followed. An extension can stick a name on an item
  once (`items/edit` with `{"op": "anchor"}` writes a `^name`, #335), and the name survives a
  reword. Finding the item by its name after its note moves (#190, PR 2) and bindings that
  bookmark what they name (PR 3) are still ahead.
- Characters decided but not hired yet: **the archivist**, who keeps every page the librarian replaces, outside the workspace
  (#305); **the logbook**, where every change is written down with who asked and which
  translator wrote it, so any change can be undone, even after a restart (#306);
  **dictionaries**, a format described as data in a small format of cold-lava's own, so the
  librarian can check a write by its bytes (#307); and the **ladder of trust** for how far a
  writer may write unchecked, now one of the foundations (#308; its unchecked rung waits for
  an owner that needs it). The archivist and the logbook wait for track C (#271).

## The rules everything stands on

Each is one line here; [foundations.md](docs/foundations.md) has the idea, and the code named is
where it lives. Where the code doesn't meet a rule yet, the line says so.

- **Files are never harmed.** A write is refused when the file changed since it was read, is
  atomic, and a delete goes to the trash (`write.rs`, `trash.rs`). What the engine doesn't
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
