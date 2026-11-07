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

*(design; items for tasks, lists, headings, blocks and typed notes and blocks exist:
`crates/engine/src/items.rs`)*

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

*(design; definitions load, are checked and are listed: `crates/engine/src/types.rs`,
`cold-lava types`; they match notes, table rows, list items, headings and paragraphs, and inline
types match in their text: `crates/engine/src/matcher.rs`; one field of a typed item is set
with the read-back check: `crates/engine/src/edit.rs`, `cold-lava item set`; the task is a
built-in definition, read in shadow mode beside the native parser: `crates/engine/types/task.toml`,
`crates/engine/src/shadow.rs`, `cold-lava types --shadow`. "Loading, as built", "Matching, as
built", "Writing, as built" and "The task as a definition, as built" below say where the code
settled what this design left open.)*

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

### Loading, as built

- **Every shape names its `on`**, even in a subtype: `[types.idea] extends = "task"` also says
  `on = "list-item"`. Inheriting shapes from a base waits for typed blocks.
- **A field can have several sources in one shape**, read in the order written; the first with
  a value wins (`title = { property = "title", note = "name" }`). TOML writes them as one
  table; the JSON of a definition writes them as a list (`[{"property": "title"}, {"note":
  "name"}]`), since not every JSON reader keeps an object's order. Both read.
- **The one-shape shorthand** puts `on`, `where`, `match` and `key` at the top, and each
  field's value type beside its sources (`fields.minutes = { capture = "v", type =
  "duration" }`). With `[[from]]` shapes, the top `fields` holds only value types and the shapes
  only sources. A field with no `type` is text.
- **Sources, in detail:** `pattern` is a field's own pattern over the block's text, read from its
  first group; `flag = "⏫"` is a bool; `flags = { "⏫" = "high", "🔼" = "medium" }` maps
  glyphs to names; `info` is `"language"` or `"arguments"`; `inlines` is `"tags"` or
  `"links"`; `note` is `"name"`, `"path"` or `"folder"`; `section` is `"heading"` or `"path"`;
  `children` names a kind or type; `rest = true`, or `rest = { without = ["#task"] }` to leave
  every occurrence of those strings out too; `checkbox = true` is the character between a list
  item's brackets (`[x]`), kept as written so a space is a value, and `note = "date"` the date
  of the daily note the block is in (the vault's Daily notes folder and format). `capture`,
  `pattern`, `marker`, `inline`, `flag`, `flags`, `checkbox`, `column`, `property` and `info`
  are writable (a checkbox is replaced only, like a capture); the rest are read-only. Each
  source can only be read on some primitives (`column` on `table-row`, `property` on `note`,
  `info` on `code`, `marker` and `rest` on text blocks, …), and `Source::on` in types.rs is that
  table.
- **`where`** has `header`, `property` (string values), `tag` (a nested tag counts),
  `folder`, `section`, `language` (the fence's) and `inside` (a type, native kind or primitive).
- **Looks:** `colour` (or `color`): `red`, `orange`, `yellow`, `green`, `cyan`, `purple`,
  `accent`, `muted`; `badge`, `icon`, `dim`, `strike`, `bold`, `hide`, `link` (the value is
  followed as a link), `whole` (the look covers the whole item), and `by-value` (looks by enum
  value name, not nested).
- **`required`** in a shape names fields a block must have a value for (not `null`, not `[]`)
  to be of that shape: the task requires its words. A block that matches a shape but lacks one
  goes to the next shape that matches, as if the first hadn't.
- **An enum's values** are a table, or the setting `"${tasks.statuses}"`: the vault's status
  symbols, each to the name of its type (`todo`, `in_progress`, `done`, `cancelled`, `other`,
  the names `agenda --json` uses).
- **Placeholders** in a `match` are `{date}` (ISO), `{number}` and `{duration}`; `\p{…}` and
  `\x{…}` escapes are kept whole. Settings (`${tasks.statuses}`, `${tasks.global-filter}`,
  `${tasks.format}`, `${daily.format}`) are checked by name at load, and filled in for each
  vault when its notes are read (`Types::fill`, from `items::Settings::new`): in a `match` or
  `pattern` as the setting's text, escaped (`${tasks.statuses}` as any one of the symbols), in
  `rest`'s `without` as plain text, and as an enum's values. `cold-lava types` lists
  definitions as written, settings unfilled. A date `format`
  takes the moment.js tokens daily notes read, and any other token is refused rather than kept
  as literal text. Patterns compile to at most 256 KiB.
- **Origins.** One built-in definition exists, `task` (`crates/engine/types/task.toml`, compiled
  into the engine, origin `built in`, last in a conflict), and it is loaded only in shadow mode
  (`types::with_built_in`) until the switch-over. `task`, `list`, `heading` and `block` are
  native kinds otherwise, which can be extended and not redefined; only the built-in origin may
  define one of those names. An extension's manifest carries
  `types` (named `<extension>.<name>`); `config.toml`'s `[types.NAME]` is handed to the engine
  by the frontend, since the engine never reads config.toml; the vault's
  `.cold-lava/types.toml` must start with `version = 1`, and a missing or unknown version skips
  the file. A later origin replacing an earlier type is a note, not an error. `actions` must be
  native actions the engine has (`tasks.toggle`).
- **Not yet:** timings in `cold-lava types`, and `--check` for conflicts.

### Matching, as built

- **One pass.** `items::extract` reads a note once with pulldown-cmark. When types are loaded,
  the pass also collects the blocks some type reads (no table rows are collected when no type is
  on `table-row`, and so on), and tags and links only when a shape needs them. After the pass the
  note is gated once (folder, frontmatter properties, tags), and each collected block is checked
  only against the shapes on its primitive that passed. No `RegexSet` prefilter yet; that is for
  the performance step. `matcher.rs` holds the matching, `frontmatter.rs` the line-by-line
  frontmatter reader.
- **Primitives so far:** `note`, `table-row`, `list-item`, `heading`, `paragraph` (a
  paragraph outside list items) and `inline`. Shapes on `code` and `quote` load but match
  nothing.
  Tasks stay native and win: a type on `list-item` never takes a task, so an `idea` that
  `extends = "task"` can't match a task yet. With the built-in definition loaded (shadow
  mode), tasks are typed blocks like any other: `items::Settings::native_tasks` is false, the
  pass doesn't call `tasks::parse_line`, and an `idea` with `where.tag = ["idea"]` wins over
  `task` as its subtype and still answers to `task`.
- **What each primitive's text is:** a list item's first line after its marker (without a
  trailing `^id`), a heading's words, a paragraph's text, a whole table row's line. A note's
  `match`, `capture` and `inline` read the text after the frontmatter. `rest` is that text with
  every other source's match taken out (a marker with its value, a whole `[due:: …]`, a flag's
  glyph, every glyph of a `flags` map, a checkbox with its brackets, the whole `match` when a
  capture is read, a trailing `^id` except on a list item, whose text is already without one),
  spaces collapsed. A marker's glyph includes an emoji variation selector after it (`⏳️`).
- **`where.tag`** on a note shape is the note's tags (frontmatter and body); on a block shape it
  is the block's own tags, not the note's: `[types.idea] where.tag = ["idea"]` means a list item
  tagged `#idea`. Tags compare without case, and a nested tag counts. `where.inside` names the
  note's type (or a base of it), `quote`, `list-item`, or the kind or type of the list item it
  is nested in. `children` and `info` read nothing yet (`[]` and `null`).
- **Inline types** run their `match` over the text of the blocks the pass already collects,
  with every match an item (`captures_iter`): a paragraph's text, a list item's own lines after
  its marker (its first line and the lines before a nested list; tasks too, since an inline
  item doesn't replace the block), a heading's words and a table row's line; a quote's or
  callout's paragraphs are paragraphs. Never code (the pass records code spans, code blocks and
  math, and a match that touches one is dropped) and never the frontmatter (it is no block). A match the
  same type already found in an outer block (a quote inside a list item) counts once. Its line
  range is the lines the match covers without its leading and trailing spaces, so its `rev`
  hashes those lines. Its parent is the item the block is: the list item or task, the heading,
  a typed row or paragraph, a paragraph with a `^id`; a plain paragraph or row is no item, so
  there the parent is `null`. Its key, without a shape `key`, is the matched text, so the id
  survives lines added above and ticking its task, and a new value is a new item. `where.tag`
  is the note's tags (a match has no text of its own to tag); `where.inside` also names the
  block's primitive (`paragraph`, `list-item`, `heading`, `table-row`) and its kind or type
  (`task`, `list`, a typed row's type), beside what the block itself sits inside. Two inline
  types can match the same text; they don't compete. `write` loads and is not used yet.
- **Values.** A field is a list when one of its sources gives a list (`inlines`, `children`,
  `section = "path"`); a list property fills it item by item. One value written where one is
  wanted, but several found (a list property on a text field), is `null`. Links are their
  targets (`[[Lava|hot rock]]` is `Lava`), tags lose their `#`, enums map written forms to names
  and anything else to `other`, durations are minutes, dates are `YYYY-MM-DD`; a `datetime` with
  a `format` doesn't parse yet. A value that doesn't parse is `null` and counted, and `cold-lava
  types` reports the count.
- **Spans.** Every value keeps the byte range it was read from in the file as written, through
  `%%comments%%` (`Flavour::source` maps offsets back); spans are not on the wire. Read-only
  sources (`note`, `section`, links, a note's tags) have none.
- **Ids.** A typed item's key is its shape's `key` fields (text as written, anything else as
  JSON, so a task keyed by its words keeps the id the native parser gave it); without one, a row's first cell, a
  list item's or paragraph's text, a heading's words, or nothing for a note. Its id is the note,
  its type and that key under its parent's, as for tasks, so a row keeps its id when other rows
  change and when its own non-key cells do. A note item's lines are its frontmatter (line 1 when
  it has none), so its `rev` changes with the frontmatter and not with the body. Items inside a
  typed note keep `parent: null`; the note item is not their parent.
- **Conflicts** go as designed, with "origin" as vault, then `config.toml`, then an extension,
  and the type's name as the last tie-break. The more specific shape is the one with more
  conditions: each header column and property counts, as do `tag`, `folder`, `section`,
  `language`, `inside` and a `match`.
- **Settings.** `items::Settings` carries the loaded types and the compiled matcher, cached per
  vault. The frontend hands over extensions' and `config.toml`'s types once at startup
  (`types::set_configured`), and `types::for_vault` adds the vault's own file.
- **Cost of inline types:** no second parse. When a type is `inline`, the pass collects every
  paragraph, list item, heading and table row (otherwise only those some type reads) and the
  code ranges, and each block's text runs each inline pattern that passed the note's gate.
  Measured on a generated 3,000-note vault with two inline types finding 7 items per note
  (21,000 in all): `cold-lava items` took 430 ms instead of 293 ms (release, median of 9).
  The same two types with patterns that never match cost nothing measurable, so the time goes
  to the items made, not to collecting blocks or running patterns.
- **Cost,** measured on a generated 3,000-note vault (tags, a table, tasks and list items in
  every note) with four types (note, table-row, list-item, heading) finding 31,000 items:
  matching adds about 0.1 ms per note to `cold-lava items` on a loaded laptop. The perf step
  owns the `RegexSet` and fewer per-item allocations.

### The task as a definition, as built

- **The definition** (`crates/engine/types/task.toml`): one shape on `list-item`, `requires =
  ["task-lists"]`, `match = '^\[[^\n]\]\s+.*${tasks.global-filter}'`, `key` and `required` the
  words. Its fields are the native task's, in its order: `status` and `status_type` from the
  checkbox (the type through `${tasks.statuses}`), `text` the rest without the global filter,
  each date from its Dataview field then its markers (`due` from `[due:: …]`, `📅`, `📆`, `🗓`;
  `scheduled` `⏳` `⌛`; `start` `🛫`; `created` `➕`; `done` `[completion:: …]` `✅`;
  `cancelled` `❌`), `recurrence` from `[repeat:: …]` or a pattern after `🔁`, `priority` from
  the five glyphs (lowest first, so the lowest present wins, as natively) then `[priority:: …]`,
  `tags` and `links` from the text, `daily_note` from `note = "date"`. `actions = ["tasks.toggle"]`.
- **Shadow mode.** The engine still reads tasks with `tasks::parse_line`. `shadow::compare` reads
  each note twice, with `Settings::load` and with `Settings::load_with(types::with_built_in)`,
  pairs the items by line and order, and compares a native task's fields (as the schema writes
  them: the status type by its `agenda --json` name, tags without `#`) and its id, rev, lines,
  section, parent and block id with the typed task's; any other item must be identical. A task
  taken by a subtype is counted, not compared. `cold-lava types VAULT --shadow [--json]` prints
  it and exits 1 on a difference.
- **Results.** The example vault (34 notes, 4 tasks), and a generated stress set of four vaults
  (178 notes each, 8,054 tasks in all: emoji and Dataview fields in any order, every priority,
  recurrence, variation selectors, global filter `#task` at the start, middle or end, custom
  statuses including `NON_TASK`, the Dataview task format setting, nesting four deep, numbered
  lists, quotes, callouts, nested quotes, continuation lines, block ids, code fences and
  comments) read the same, ids included, but for 6 tasks whose recurrence is followed by a
  `^x` that names nothing (the last case below). The same set with the known differences below
  written into every note finds only those.
- **Known differences,** each pinned by `shadow::tests::known_differences`; the native parser's
  reading is the one to drop at the switch-over unless said otherwise:
  - Tags: the native parser's own pattern takes `#2026` and not `(#paren)`; the definition reads
    tags as the vault does (`markdown::TAG`), like Obsidian.
  - A marker inside a code span (`` `📅 2026-01-01` ``): natively a date, and taken out of the
    code; the definition's marker reads the date with the closing backtick, which isn't a date.
    Neither is right; the switch-over should keep sources out of code spans.
  - The same field twice (`📅 a 📅 b`, or `[due:: a] 📅 b`): natively the last date (Dataview's
    when it parses), every one taken out of the words; the definition reads the first source
    with a value and leaves the others in the words.
  - An unbracketed `due:: x` in a task's line is a Dataview field to the definition, words to the
    native parser.
  - `[priority:: urgent]` is `null` natively and `other` (the enum's fallback) to the definition.
  - A recurrence followed by words that are no field (`🔁 every day (or so)`, or a `^x` that
    isn't a block id because a continuation line follows): natively no recurrence, as the
    plugin, which reads fields from the end of the line; the definition reads `every day`.
    Rust's regex has no look-ahead to require what follows.
- **The switch-over** loads `Layer::built_in()` in `types::for_vault`, drops `task` from
  `NATIVE` and `Data::Task`, and moves `agenda`, `task toggle` and the reader's `x` onto typed
  tasks. Still to settle there: the item JSON (pinned by `item_json_is_pinned`) changes from
  `status_type: "non_task"` to `other` and loses the `#` on tags; `set-status` through the
  checkbox (a space is a value `input` now reads as removal); toggling and recurrence stay
  `tasks.toggle`; and the known differences above.

### Writing, as built

- **One call.** `edit::set_field(root, note, SetField { id, rev, field, value }, dry_run)` reads
  the note, `edit::plan` makes the edit in memory, and `write::replace` writes it, refused if the
  file changed since it was read. The CLI is `cold-lava item set FILE ID FIELD VALUE` (or
  `--remove`), with `--rev`, `--dry-run` and `--json`, beside `task toggle FILE:LINE`: both
  name one item in one note, and `items` stays a read. `items/edit set-field` will call the same
  function. `Set` carries the new text, the item as it reads back, and `changed` (false when the
  note already said this; nothing is written then).
- **Finding the item.** By id in the note as it is now; gone, or a `rev` that differs, is
  `Changed` (exit code 3, like a stale `task toggle`). Only typed items: a task's fields wait
  for the task as a definition.
- **Which source.** Each typed item carries, off the wire, its shape, which source gave each
  field its value, its block's text and a row's cells (`matcher::Place`). The value is written
  through the source that gave it; when that one is read-only, through the first writable
  source before it (so `title = { property = "title", note = "name" }` adds the property), and
  for a field with no value, through its first writable source.
- **The value** comes in the form items carry it (an ISO date, minutes, an enum's name, a
  link's target) or as a string the field's type reads (`1h30m`, `[[Lava]]`, an enum's written
  form); `null` or `""` removes. It is written by its value type: a date in the field's
  `format` (ISO without one), a datetime with `T` or a space as the old value had, a number as
  JSON writes it, `yes`/`no` when the old value was written so, a link as `[[target]]` unless the
  old value was a bare name or a Markdown link, a tag with `#` in text and without in a
  property, an enum in the old value's form, else the form the note's other items of the type
  use, else the first, a duration as `1h30m` (minutes alone, `90m`, when the old value was).
  A datetime with a `format` is refused, as it doesn't read yet.
- **Sources.** Replacing changes only the value's span. Adding and removing:
  - `marker`: a value goes after a glyph that has none, else ` 📅 value` is appended to the
    block's text (before a `^id`); removing takes the glyph, its value and one space.
  - `inline`: a block gets ` [key:: value]`, or `(key:: value)` when the block's other Dataview
    fields use parentheses; a note gets a `key:: value` line at the top of its body. Removing
    takes the brackets too, and an unbracketed field's whole line when nothing else is on it.
  - `flag`: true appends the glyph, false takes it out with a space; null is refused. `flags`
    swaps the glyph for the one of the new name, appends it, or takes it out.
  - `column`: the cell between its pipes is rewritten as wide as it was when the value fits
    (its leading spaces kept), else just wide enough for the value; other rows are never
    touched. A value with `|` or a line break is refused, as is a column the row has no cell
    for. Removing blanks the cell, keeping its width.
  - `property`: a scalar or a one-item list's value is replaced inside its quotes, or quoted
    when YAML needs it; `key:` with nothing gets the value on its line; a missing key is a new
    line before the closing `---`, with new frontmatter when the note has none. Removing deletes
    the property's lines. Nested YAML and lists of several values are refused.
  - `capture` and `pattern`: replaced only. A pattern says where a value is read, not where one
    goes, so a missing value is refused, and so is removing one (it would leave the text around
    it). The type's `write` template, meant for this, is still not used.
  - `rest`, `inlines`, `note`, `section` and `children` are read-only and refused, and so is
    `info`, which reads nothing yet.
- **The read-back check.** The new text is read again with `items::extract`, and the edit is
  refused (`ReadBack`, with what would differ) unless: the note has as many items as before;
  the item (found by its id, else, when its key changed, as the item of its kind starting on
  its line) is of the same type, has exactly the new value, and every other field as it was;
  and every other item, paired in order, has the same kind and fields (spans aside), block id
  and parent. What follows from the edit itself is allowed: a new id for the item when the
  field is part of its key (its shape's `key`; without one a list item's, paragraph's or
  heading's text and an inline match's whole text are the key, so they get new ids; a row
  without `key` changes id only with its first cell, a note never), new ids and parent ids for
  what sits under it, a new `rev` for items sharing its lines, and the same one section name
  changed for the items under a typed heading whose words changed. The key-field decision:
  the edit goes ahead and returns the new id, since refusing would make keys unwritable, and
  the caller gets the item as it reads now.
- **Not yet:** the `write` template for adding a pattern's value or a whole inline item, and
  writing a list field (tags, links, children are read-only anyway, and a property holding
  several values is refused).

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
    `editor/save`, `editor/renamed` (`from`, `to`) and `editor/deleted` (`note`: it went to
    the trash; forget it without writing it back).
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
(with `commands`), `command/run`, `editor/open`, `editor/save`, `editor/renamed`,
`editor/deleted`, `pane/message`
and `pane/exited` from the host, and `ui/notify`, `command/run`, `editor/opened`, `vault/saved`,
`pane/open` and `pane/input` from the extension.

- **Saves are not acknowledged yet.** `editor/save` gets no answer, so before it reads the files
  (to rename a note, to show the reader, to quit) the core waits 300 ms. A later version answers
  `editor/save` once the files are written.
- **`editor/renamed` and `editor/deleted` are temporary.** Until extensions can subscribe to
  events, a rename or a delete tells the editor directly. Then the editor follows `note.renamed` like any other subscriber.

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

[types.doc]                      # loaded as word-count.doc
on = "note"
fields.words = { property = "words", type = "number" }

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
      `example-vault/.cold-lava/types.toml`: `volcano` notes and `eruption` table rows)
      *(done: `types::load`, definitions' JSON pinned for `types/list`)*;
      primitives with spans; typed blocks *(done for notes, table rows, list items, headings and
      paragraphs: `matcher.rs`, `cold-lava items VAULT TYPE`)*; inline types *(done: in
      paragraphs, list items and tasks, headings and table rows, with the block as parent;
      `vei` in the example vault)*; `set-field` with the read-back check *(done:
      `edit::set_field`, `cold-lava item set`)*;
      the task as a definition, first in shadow mode beside today's parser *(done:
      `types/task.toml`, `cold-lava types --shadow`)*, then switched over;
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
