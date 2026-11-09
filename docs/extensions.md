# Extensions: design

Status: **draft**. This is the plan the code is being built towards; sections are marked with
what exists already. For the whole picture (the engine's layers, and how it is separate from
the terminal UI) start with [engine.md](engine.md).

cold-lava aims to be as extensible as Obsidian, and open-source. The product at the centre is
the notes app itself: open a workspace, find a note, read and edit it, move between notes through
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
 core              live workspace · index (links, backlinks, tags, aliases) ·
                   search · safe writes · events
```

Each layer only talks to the one below it, so any one of them can be replaced on its own.

## Core

- **Live workspace.** One `Core` owns the workspace. It is the only thing watching the folder; when a
  note changes it re-reads only that note and updates the index. Nothing else scans the disk.
- **Index.** Link resolution with Obsidian's rules, backlinks with line and context, tags,
  aliases, full text. (Exists: `index.rs`, `search.rs`.)
- **Safe writes.** Every write goes through one path that writes atomically and can refuse when
  the file changed since it was read. Renames, task toggles, whiteboards and extensions all use
  it, so nothing overwrites an edit made in Obsidian at the same time. (Exists: `write.rs`.)
- **Events.** `note.created`, `note.changed`, `note.removed`, `note.renamed`, emitted once per
  change, whatever caused it (cold-lava, Obsidian, Neovim, git). Anything can subscribe.

## Reading is core, editing is an extension

Reading a workspace and moving through it by links is the product, so it works with nothing else
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
  embeds, callouts, highlights, `%%comments%%`, tags, block ids and math). A workspace with an
  `.obsidian` folder reads as `obsidian`, any other folder as `github`.
- **Your own.** `config.toml` defines more, as a built-in flavour with features added or
  removed, and picks the one to use.
- **Later, extensions.** A flavour decides what is parsed. What it leaves unrendered, such as a
  fenced code block in `mermaid` or a callout type of a plugin's own, an extension can draw
  through `view/render`.

## Notes as data

*(design; items for tasks, lists, headings, blocks and typed notes and blocks exist:
`crates/engine/src/items.rs`; `items/query`, `items/subscribe`, `items/edit` and `types/list`
exist in-process: `crates/engine/src/gateway.rs`, "The workspace gateway, as built" below)*

You write notes, not data. But Markdown already has types, and Obsidian's community has settled
how to read them: a checkbox is a task, with dates written the Tasks plugin's way
(`📅 2026-11-12`) or Dataview's (`[due:: 2026-11-12]`); frontmatter is typed properties; a table
is rows of records; `key:: value` is a field; a heading starts a section. The core reads every
note this way, keeps it current as files change, and lets extensions ask for it and change it.
An extension that lists every open todo in the workspace, or a reading list built from a table, is
then a query and a view, not a parser of its own.

- **Items.** One pass per changed note, in the core, gated by the workspace's flavour (no
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

### The workspace gateway, as built

- **Where it lives.** The messages and their meaning are the engine's
  (`crates/engine/src/gateway.rs`), since the workspace gateway knows nothing of screens. The
  core answers them: `Core::query_items`, `Core::subscribe_items`, `Core::unsubscribe_items`,
  `Core::take_item_news` and `Core::edit_item`. An extension reaches them through `Host`
  (`query_items`, `subscribe_items`, `unsubscribe_items`, `edit_item`, `types`), which checks
  its manifest's permissions first. `gateway::Request` and `gateway::News` are the messages as a
  transport will carry them, pinned by tests.
- **`items/query`** takes `kind` (a native kind or a type; a subtype answers to its base),
  `filter` and `page`. The filter's conditions all hold: `folder` (the note is in it or under
  it), `tag` (the note has it, `#` or not; a nested tag counts), `status` (the item's
  `status_type` is one of these), `date` (`field` is `due`, `scheduled`, `start`, `done` or
  `agenda`, with `from` and `to`, both included, either open), `text` (every folded word is in
  one of the item's text values) and `property` (the item has a value for that field). An
  unknown filter key is refused when the message is read. Items come by note, then line, the
  order `cold-lava items` prints. A page is `after` (the last id of the previous page) and
  `limit`; the answer is `{items, next}`, with `next` the `after` of the next page or `null`.
  When the `after` item no longer matches, the query is refused as `changed`. An unknown kind
  is `refused`.
- **`items/subscribe`** takes `kind` and `filter` and answers `{subscription}`. The core keeps,
  per subscription, the id and `rev` of every matching item by note. Each time it reads notes
  again (one `poll`, one `note_changed_now`, one edit) it sends each subscription it touched one
  `items/changed {subscription, added, updated, removed}`: added and updated items whole,
  removed ones by id. An item that stops matching the filter (a task ticked out of `status:
  ["todo"]`) is removed. After a full rescan every subscription gets `items/reset
  {subscription}`, and the subscriber queries again. `items/unsubscribe {subscription}` stops
  one, only the caller's own. The menu hands news to the extension whose name made the
  subscription (`Extension::items`), or to its own screen; news for a subscription nobody holds
  any more stops it.
- **`items/edit`** takes `id`, `rev` and an `op` with its fields:
  `{"op": "set-field", "field", "value"}` through `edit::set_field`, or `{"op": "toggle"}`
  through `tasks::toggle` (only on an item that answers to `task`). An unknown op is refused
  when the message is read, naming the ops there are. The core finds the item by id (ids hash
  the note, so they are unique in the workspace), makes the edit through that op's safe path,
  reads the note again at once, and answers `{item, changed, next, warning}`: the item as it
  reads now (a new id when the edit changed its key), whether anything was written, a
  recurring task's next occurrence, and a warning that didn't stop the edit.
- **Refusals** are `{reason, message}`, the JSON-RPC error's `data` once a transport carries
  them: `changed` (the item is gone or not the version `rev` names, or the file changed while
  writing), `refused` (an unknown kind or field, a read-only source, a value that doesn't fit,
  a toggle on no task), `read-back` (the note wouldn't read back as asked; nothing was
  written), `io`, `forbidden` (the manifest doesn't ask for the permission), `no-workspace` and
  `exists` (`workspace/create` at a path where a file is).
- **Edits are made during the call, not queued.** An extension's reads and edits both need an
  answer (the new `rev`, or why it was refused), and a later edit in the same call must see the
  first one, so the core edits and reads the note again before `edit_item` returns. An item of
  a file another owner reads is the exception: its owner writes it, so the answer comes later
  ("Owners and types, as built"). The
  frontend's state is never touched during the call: the edit's events wait in the core and
  the next `poll` returns them, which the menu reads before it draws. The rule that effects
  are queued still holds for everything that changes the menu.
- **Attribution.** Every edit through the gateway makes the core emit `ItemEdited {note, item,
  by, what}` after the note's `NoteChanged`, with `by` the extension's name and `what` "set
  due", "removed due" or "ticked". The status line says `<extension>: set due on <note>`. A
  screen of the menu says more right after, and so does an extension: after a call in which it
  edited, the menu reads the edits back (`Core::has_unseen`, then `poll`) before it applies what
  the extension asked, so the extension's own message comes last. Calls made while reading back
  (the news it hands out) leave their edits for the next frame, so an extension that edits on
  every change can't loop within one.
- **A task says its status.** An item that answers to `task` always has a `status` and a
  `status_type` in answers, news and the answer to an edit, even when its type reads neither (a
  subtype that names its checkbox otherwise, or reads none): the status from its line, its type
  from the workspace's Tasks settings, as `tasks::Task` reads them (`gateway::answer`). The
  `status` filter sees them too, so such a subtype is an open task. A client never needs the
  settings or the note's lines.
- **The menu's screens are clients too.** The reader's `x` goes through `Registry::screen`, a
  `Host` named `reader` that may read and edit, so it uses exactly the path an extension does.
  The agenda and the table are extensions (below).
- **Over the wire.** An extension in its own process sends these as JSON-RPC requests, and
  `rpc::serve` reads each as a `gateway::Request` and answers it through the same `Host`, so
  the permission gate is the same; news goes out as notifications ("Transport, as built").
- **Files.** `workspace/read`, `workspace/write`, `workspace/create` and `workspace/files`
  read and write whole files ("File types, as built" below).
- **Not yet:** the ops `set-status`, `set-date`, `set-property`,
  `set-cell`, `replace` and `insert-after`. `set-field` already sets a status, a date, a
  property or a cell of a typed item; the others wait until an extension needs one.

## Types

*(design; definitions load, are checked and are listed: `crates/engine/src/types.rs`,
`cold-lava types`; they match notes, table rows, list items, headings and paragraphs, and inline
types match in their text: `crates/engine/src/matcher.rs`; one field of a typed item is set
with the read-back check: `crates/engine/src/edit.rs`, `cold-lava item set`; the task is a
built-in definition, the only way tasks are read: `crates/engine/types/task.toml`, with
`agenda` and `task toggle` on its items (`crates/engine/src/tasks.rs`); the reader draws
values with their looks: `crates/cold-lava/src/looks.rs`; every type has a table:
`crates/cold-lava/src/table.rs`, `type_table.rs`, `cold-lava table`. "Loading, as built",
"Matching, as built", "Writing, as built", "The task as a definition, as built", "Looks, as
built" and "The table view, as built" below say where the code settled what this design left
open.)*

Markdown gives the primitives: blocks (paragraph, heading, list item, quote, code fence, table
row, a note's frontmatter), inlines (text, link, tag), and value types (text, number, date,
datetime, duration, bool, link, tag, enum). A **type** gives them meaning, declared in TOML with
no code, by you, by a workspace, or by an extension. The built-in task is written in the same
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
  `${daily.format}`) let a definition follow the workspace's own settings.
- **Deliberately not there:** formulas, joins, conditions on other items, matching across
  blocks, look-around in patterns (Rust's regex has none, so no pattern can hang), and new
  grammar Obsidian would show raw. Behaviour is native code named in `actions`: the task's
  definition reads and styles tasks, while ticking (done dates, recurrence) stays
  `tasks.toggle`, written in Rust.
- **Conflicts.** One type per note, one per block, any number inline. A typed block replaces its
  structural kind, as a task replaces a list item today. Within a level the winner is: higher
  `priority`, then a subtype (`extends`) over its base, then the more specific shape, then
  origin (workspace, user, extension, built-in). A subtype still answers to its base, so
  `[types.idea] extends = "task"` with `where.tag = ["idea"]` stays in the agenda. Inside a
  code fence only types that name it with `where.inside` match, so tasks are never read from
  code. `cold-lava types --check` lists blocks two types wanted, and which won.
- **Identity.** The same scheme as tasks today: an author's `^block-id`, else the shape's `key`
  fields under the parent's key; `rev` hashes the item's own lines (a row's line, a note's
  frontmatter, a fence). An inline item's parent is the block it sits in. Existing ids don't
  change.
- **Where definitions live,** in load order: built in (`types/*.toml` in the binary), extension
  manifests (named `<extension>.<name>`), `[types.*]` in `config.toml`, and
  `<workspace>/.cold-lava/types.toml`, which travels with the workspace. Built-in names can be extended
  but not redefined. Mistakes are collected like `config.toml`'s: one line each, the broken type
  is dropped and the rest load. `cold-lava types WORKSPACE` lists every type, its fields and where
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
  match before any pattern runs. Targets on 10,000 notes with 30 types: the whole workspace matched
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
  of the daily note the block is in (the workspace's Daily notes folder and format). `capture`,
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
  value name, not nested). A field whose values are `${tasks.statuses}` takes the status types
  by name in `by-value` (`todo`, `in_progress`, `done`, `cancelled`, `non_task`), whatever
  symbols the workspace gives them.
- **`required`** in a shape names fields a block must have a value for (not `null`, not `[]`)
  to be of that shape: the task requires its words. A block that matches a shape but lacks one
  goes to the next shape that matches, as if the first hadn't.
- **An enum's values** are a table, or the setting `"${tasks.statuses}"`: the workspace's status
  symbols, each to the name of its type (`todo`, `in_progress`, `done`, `cancelled`,
  `non_task`), the plugin's core symbols first (` `, `x`, `/`, `-`), so a status set by its type
  is written with the symbol the plugin would. A symbol the settings don't name is `non_task`
  too; `agenda --json` calls it `other`, as it always has.
- **`priority`** of a type is its own, else its nearest base's, else 0: a subtype starts at its
  base's. The built-in task has `priority = 1`, so a list-item type that doesn't extend `task`
  never takes a checkbox from it unless it sets a higher priority.
- **Placeholders** in a `match` are `{date}` (ISO), `{number}` and `{duration}`; `\p{…}` and
  `\x{…}` escapes are kept whole. Settings (`${tasks.statuses}`, `${tasks.global-filter}`,
  `${tasks.format}`, `${daily.format}`) are checked by name at load, and filled in for each
  workspace when its notes are read (`Types::fill`, from `items::Settings::new`): in a `match` or
  `pattern` as the setting's text, escaped (`${tasks.statuses}` as any one of the symbols), in
  `rest`'s `without` as plain text, and as an enum's values. `cold-lava types` lists
  definitions as written, settings unfilled. A date `format`
  takes the moment.js tokens daily notes read, and any other token is refused rather than kept
  as literal text. Patterns compile to at most 256 KiB.
- **Origins.** One built-in definition exists, `task` (`crates/engine/types/task.toml`, compiled
  into the engine, origin `built in`, last in a conflict), loaded first for every workspace
  (`types::for_workspace`). `list`, `heading` and `block` are native kinds (`types::NATIVE`), and
  `task` is a built-in type (`types::BUILT_IN`): all four can be extended and not redefined, and
  only the built-in origin may define one of those names. An extension's manifest carries
  `types` (named `<extension>.<name>`); `config.toml`'s `[types.NAME]` is handed to the engine
  by the frontend, since the engine never reads config.toml; the workspace's
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
  Tasks are typed blocks like any other, of the built-in `task` type; an `idea` that `extends =
  "task"` with `where.tag = ["idea"]` wins over `task` as its subtype and still answers to
  `task`, so it is in the agenda and `task toggle` ticks it.
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
  (a form the table doesn't have doesn't parse; the statuses' is `non_task`), durations are
  minutes, dates are `YYYY-MM-DD`; a `datetime` with
  a `format` doesn't parse yet. A value that doesn't parse is `null` and counted, and `cold-lava
  types` reports the count.
- **Every source of a field is the field's.** The first source with a value gives it, as before;
  a value any other source of the field finds is taken out of `rest` too, and so is every
  occurrence of a marker or Dataview field: `📅 2026-11-12 📅 2026-11-13` is due the 12th with
  neither date in the words, the way the Tasks plugin reads it. A later source's text that
  doesn't parse as the field's type stays in the words (`🗓 later`).
- **Never in code.** No source reads in a code span or math: a marker, Dataview field, pattern,
  flag or tag there is words (`` `📅 2026-01-01` `` is no due date), as an inline type never
  matches there.
- **An unbracketed Dataview field** (`due:: x`) is read only at the start of a line, as Dataview
  reads it: a note's line, or a list item's text (`- due:: x`); in the middle of a task's line
  it is words. `[due:: x]` and `(due:: x)` are read anywhere.
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
- **Conflicts** go as designed, with "origin" as workspace, then `config.toml`, then an extension,
  and the type's name as the last tie-break. The more specific shape is the one with more
  conditions: each header column and property counts, as do `tag`, `folder`, `section`,
  `language`, `inside` and a `match`.
- **Settings.** `items::Settings` carries the loaded types and the compiled matcher, cached per
  workspace. The frontend hands over extensions' and `config.toml`'s types once at startup
  (`types::set_configured`), and `types::for_workspace` adds the workspace's own file.
- **Cost of inline types:** no second parse. When a type is `inline`, the pass collects every
  paragraph, list item, heading and table row (otherwise only those some type reads) and the
  code ranges, and each block's text runs each inline pattern that passed the note's gate.
  Measured on a generated 3,000-note workspace with two inline types finding 7 items per note
  (21,000 in all): `cold-lava items` took 430 ms instead of 293 ms (release, median of 9).
  The same two types with patterns that never match cost nothing measurable, so the time goes
  to the items made, not to collecting blocks or running patterns.
- **Cost,** measured on a generated 3,000-note workspace (tags, a table, tasks and list items in
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
- **The switch-over** (done). `types::for_workspace` loads the built-in layer first, so every workspace
  reads its tasks with the definition; `tasks::parse_line`, `Data::Task` and the shadow
  comparison are gone. `tasks::Task` is now a view of an item that answers to `task` (a subtype
  too: a field it doesn't read is empty, and the status comes from the line when it has no
  `status`), and `agenda` and `task toggle` read through it. Toggling stays native code
  (`tasks.toggle`): the status goes to its next symbol (the plugin's `nextStatusSymbol`, else
  done for an open task and todo for a closed one); to a done status it adds the done date
  (`✅` or `[completion:: …]`, as the line writes its fields) and, for a recurring task, the next
  occurrence with its dates moved; to a cancelled status it adds the cancelled date (`❌` or
  `[cancelled:: …]`, unless the plugin's `setCancelledDate` is off) and takes the done date
  out; otherwise it takes the done and cancelled dates out. `tasks::today()` is the local date,
  or `COLD_LAVA_TODAY` when set (scripted runs and their tests).
- **Results.** The shadow comparison's corpus (`crates/engine/tests/tasks/corpus.md`: every
  format, priority, recurrence, variation selectors, custom statuses, the global filter and the
  Dataview setting, nesting, quotes, callouts, continuation lines, block ids, fences, comments)
  is now a regression test against the native parser's reading, ids and revs included
  (`tests/task_corpus.rs`), and the example workspace's tasks are pinned as they were
  (`tests/golden/items-task.json`). `agenda --json`, `agenda` and `task toggle` give the same
  bytes as before on the example workspace.
- **The known differences,** settled (`known_differences_follow_the_tasks_plugin`):
  - Tags are read as the workspace reads them (`markdown::TAG`): `#2026` is no tag, `(#paren)` is.
  - A marker in a code span is words: no source reads in code.
  - The same field twice (`📅 a 📅 b`): the first, both out of the words, as the Tasks plugin,
    which reads fields from the end of the line, so the earlier one is read last and wins.
    Dataview's and an emoji both (`[due:: a] 📅 b`): Dataview's, both out of the words. The
    plugin reads only the format its settings name, and would leave the other in the
    description; cold-lava reads both formats, so a workspace that switched keeps its dates, and
    keeps the native reading here.
  - An unbracketed `due:: x` in a task's line is words: Dataview reads an unbracketed field only
    at the start of a line, and the Tasks plugin only in brackets.
  - `[priority:: urgent]` is `null` and out of the words: an enum's form outside its table
    doesn't parse. The plugin doesn't read it either.
  - A recurrence followed by words that are no field (`🔁 every day (or so)`): the definition
    reads `every day`, the plugin nothing, since it reads fields from the end of the line. Rust's
    regex has no look-ahead to require that a field or the end follows, and the pattern can't
    take the words after it without taking them out of the text; left as is.
- **Item JSON.** The same keys in the same order; tags lose their `#` (`"tags": ["home"]`).
  `status_type` keeps its names (`non_task`).
- **Writing a task.** `item set` works on tasks: `status` through the checkbox (a space is a
  status: `" "` unticks), `status_type` by name (`done` writes the workspace's done symbol), dates,
  priority and the rest through their markers or Dataview fields. A field read from the same
  text as the one set (the type of a status, from its checkbox) may change with it in the
  read-back check. `item set` changes only the field; ticking with dates and recurrence is
  `task toggle`. A date the task doesn't have is added through a writable source of a kind the
  line already writes its other fields with (a marker beside `⏳ …`, a Dataview field beside
  `[scheduled:: …]`), else a Dataview field when the workspace's Tasks plugin writes Dataview or the
  line has a `::`, as `task toggle` decides, else a marker (`📅 …`). This is the rule for any
  type's missing field, so it holds for subtypes of the task and for a type of one's own with
  both kinds of source.
- **Cost,** on a generated 3,000-note workspace with 30,000 tasks in both formats (release, median
  of runs interleaved with the old binary, on a loaded laptop): `cold-lava items WORKSPACE task`
  613 → 872 ms (+42%), `--json` 683 → 856 ms (+25%), `agenda --json` 766 → 1,029 ms (+34%),
  peak memory 45 → 119 MB. A typed task is a map of fourteen fields with their spans and
  sources, where the native task was a struct. Matching was +70% at first; field names shared by the type (`Arc<str>`), a
  copied empty item per type, no regex captures where no field reads a group, a `::` and
  `is_match` check before a Dataview field's pattern, ISO dates without the moment.js parser
  and `where.inside` built only when a shape asks brought it here. The rest is the per-item
  maps and allocations, which the performance step owns with the `RegexSet`.

### Writing, as built

- **One call.** `edit::set_field(root, note, SetField { id, rev, field, value }, dry_run)` reads
  the note, `edit::plan` makes the edit in memory, and `write::replace` writes it, refused if the
  file changed since it was read. The CLI is `cold-lava item set FILE ID FIELD VALUE` (or
  `--remove`), with `--rev`, `--dry-run` and `--json`, beside `task toggle FILE:LINE`: both
  name one item in one note, and `items` stays a read. `items/edit set-field` calls the same
  function. `Set` carries the new text, the item as it reads back, and `changed` (false when the
  note already said this; nothing is written then).
- **Finding the item.** By id in the note as it is now; gone, or a `rev` that differs, is
  `Changed` (exit code 3, like a stale `task toggle`). Only typed items, which tasks now are.
- **Which source.** Each typed item carries, off the wire, its shape, which source gave each
  field its value, its block's text and a row's cells (`matcher::Place`). The value is written
  through the source that gave it; when that one is read-only, through a writable source before
  it (so `title = { property = "title", note = "name" }` adds the property), and for a field
  with no value, through a writable source: one of a kind the item reads another field from,
  else a marker or a Dataview field as the workspace's tasks are written (see "The task as a
  definition, as built"), else the first.
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
    block's text (before a `^id`); removing takes the glyph, its value and one space. An emoji
    variation selector after the glyph (`⏳️`) goes with it.
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

### Looks, as built

- **Who does what.** The engine places marks; the frontend decides what they look like.
  `markdown::parse_marked(text, flavour, marks)` takes byte ranges of the file and puts
  `Inline::Mark { id, open }` around the inlines each covers; `Doc::broken` says which it
  couldn't place. `crates/cold-lava/src/looks.rs` turns a note's items into those ranges and
  their looks, and `reader.rs` draws them. The colour names become terminal colours in one
  table there (`looks::colour`), which the whiteboard's canvas presets use too.
- **Which text.** Each value with a look is one mark over its span (`Typed::spans`, one per
  value of a list field); `whole` is one mark over the item's own text, `Place::span`: the
  block's text, or an inline type's match without its outer spaces. `by-value` picks the
  value's look over the field's; a look given only by value is none for the other values. A
  subtype without a look for a field uses its base's.
- **From file offsets to the rendered text.** Spans are file offsets; `Source::text_range` maps
  them through removed `%%comments%%` (an end inside one breaks the mark). While reading inlines,
  each pulldown-cmark event keeps its source range: text whose content is exactly its source
  bytes can be cut at a mark's ends; a container (link, emphasis) a mark covers only in part
  leaves it to the inlines inside, which get their own marks (so a mark can open and close more
  than once, a chip around a link's half and the text beside it); a container a mark covers
  whole is marked around, not again inside. A mark is **broken**, and its look skipped, when it
  covers only part of anything else (an entity like `&amp;`, code, math, a footnote reference),
  or a cut would change what the extras read: inside a tag, in a `==`, or a new tag where the
  second part starts. Marks are placed before the extras run, and the extras look through them
  (a callout's `[!type]`, a trailing `^id`, an embed alone in its paragraph).
- **Drawing.** The reader keeps the marks open around each inline (outermost first) and patches
  the style with each look: `colour` the foreground, `dim` the dim grey (or the dim modifier
  with a colour), `strike`, `bold`; `badge` a chip, the colour on a dark tint of it with a space
  of padding at each end, unbroken by wrapping; `icon` and a space before the value; `hide`
  drops the written text and, with a badge or icon, draws the value in its place; `link`
  registers a wiki link to the value, which `Tab`, `Enter`, `f` and `K` treat like any other.
  `NO_COLOR` (non-empty) leaves colours out and draws chips in reverse video. Heading names
  (outline, jumps) are read without looks.
- **Frontmatter.** It isn't rendered, so a note type's values read from it can't be drawn in
  place. The note's typed fields with a look whose mark appears nowhere in the body, and isn't
  broken, go on a properties line above the note: the icon and value, or the field's name and
  value without an icon (`▲ 2549`). Reading starts below it, so the title's breadcrumb is the
  first heading's as before. A note type's `whole` is skipped: it would be the whole body.
- **Cost.** Once per note load and per change on disk, not per frame: the items come from the
  core's cache (`Workspace::items`) when the core's text is the text on disk, else are read once,
  and the marks are placed in the same pulldown-cmark pass that builds the `Doc`. The reader
  lays the marks out with the page, again only when the width or the note changes.
- **Not yet:** a light terminal's table, and looks in the GUI. The table view and the agenda
  draw the same looks (below).

### The table view, as built

- **The view is declarative and knows nothing of types.** `crates/cold-lava/src/table.rs`:
  a `Table` is a title, `Column`s (a name, a value type: `text`, `number`, `date`, `datetime`,
  `duration`, `bool`, `link`, `tag`, or `enum` with its names in order; an optional `look`, the
  type's `Look` as is; an optional `read_only` reason) and `Row`s (an `id` and one JSON value per
  column, `null` for none, a list for several). Back come `Event`s: `activated {row}` on Enter
  and `edit {row, column, value}` with the text typed. Everything derives serde and a test pins
  the JSON, so a table can cross the view gateway unchanged. It lives in the binary for now and
  moves to `crates/views` when a second frontend draws one.
- **The frontend owns** selection by row and column, sorting (`s` / `S` on the selected column:
  numbers and durations as numbers, dates and datetimes as their ISO text, enums by their
  declared order, bools, the rest by `search_key`; empty cells last either way; stable, so the
  sender's order breaks ties), filtering (`/`, or typing when the table asks for it: every
  folded word must be in some cell's text),
  column widths (the widest cell or the name, capped at 32 columns, cut with `…`), horizontal
  scrolling that keeps the selected column on screen, the edit prompt (prefilled with the value
  as shown; a list's cell and a `read_only` column say so instead of prompting), and keeping the
  selected row by id when a new table arrives. Looks are drawn by `looks::apply` as in the
  reader: a chip only as wide as its text, an icon before it; `whole` looks (a done task) style
  the whole row; `hide` is ignored, as a table always shows the value.
- **The sender** is `type_table.rs`, the built-in table extension (`Tables`, named `table`,
  reading and editing items), on the path one in a process takes: columns are
  the type's schema in order, then `note` (read-only); rows are every item that answers to the
  type (subtypes too), with the note, line and `rev` of each kept beside the table. A column is
  read-only when no shape of the type reads the field through a writable source; the reason
  says where it is read from. It reads through the workspace gateway: the type's definition
  from `types/list`, its items from `items/query` a page of 500 at a time, kept by an
  `items/subscribe` (`type_table::Feed`). `i` (`table.open`) stays a core command, with its
  place in the help and which-key, and the menu hands it to the extension as `command/run`. The
  extension sends the picker with `view/open`: a table titled "Table of…" with a row per type
  (`type`, and `items`, how many items answer to it, counted with queries when it opens) and
  `"filter": "typing"`, so typing a few letters filters it at once, as the menu's popup did
  before the table was an extension. Enter on a type is `view/event activated`;
  the extension closes the picker and opens that type's table in its place. Each table it has
  open keeps its own subscription until `view/closed`, so Back can return to any of them.
  `cold-lava table` calls `gateway::query` directly and prints `type_table::build`.
- **Editing** is `view/event edit` to the extension, which sends `items/edit set-field` with
  the row's `rev` and the value as typed (so `1h30m`, an enum's written form or `""` to remove
  all work). The status line says what happened (`ui/notify`): set, removed, already that
  value, or the refusal (`refused` and `read-back` with their reason, `changed` when the item
  changed since the table was read). The core reads the edited note at once, and the extension
  hears it through its subscription and sends the table again with `select` naming the row as
  it reads now, so an edit that changed the row's id (its key field) keeps the row selected.
- **Live.** The table applies its subscription's `items/changed` to its items (an updated item
  keeps its place, added ones are put in by note and line) and `items/reset` queries again,
  sending the table with `view/update`; the view keeps sort, filter and selection. Enter is
  `view/event activated`, and the extension asks for `workspace/open` at the item's line. Back
  (`Ctrl-o`) returns to the table as it was, since a view is a place while it is open.
- **CLI.** `cold-lava table WORKSPACE TYPE` prints the rows as the view lays them out, every value
  whole; `--sort FIELD` (or `-FIELD`) sorts as `s` (`S`) does, `--json` prints the `Table`.
- **Not yet:** editing several values at once, and a column picker.

### The list view and the agenda, as built

- **The view is declarative and knows nothing of tasks.** `crates/cold-lava/src/list.rs`: a
  `List` is a title, `Group`s (an `id`, a title, `collapsed` to start folded) of `Row`s (an
  `id`; `text`, a list of `Piece`s, each a string with an optional `Look`; an optional `check`,
  `unchecked`, `partial`, `checked` or `crossed`; `right`, pieces drawn at the right edge; a
  `depth` to nest under the row before; a `look` over the whole row), the sender's `keys` (one
  character, a command name and a title for the hint line) and what an empty list says. Back
  come `Event`s: `activated {row}` on Enter, `toggled {row}` on `x` over a row with a checkbox,
  and `command {command, row}` for the sender's keys. Everything derives serde and a test pins
  the JSON. It lives in the binary for now, beside the table, and moves to `crates/views` with
  it.
- **The frontend owns** the selection (a group's header or a row, kept by id when a new list
  arrives; a row folded away selects its group's header, and `select` follows a row that shows
  up later), folding (`h`/`←` folds the selection's group, `l`/`→` opens it, `Tab` and Enter
  on a header switch it; a fold by hand wins over `collapsed` across updates), filtering (`/`,
  or typing when the list asks for it: every folded word must be in the row's text or right
  side; folded groups show their matches, empty groups hide), and drawing: the checkbox, the pieces with `looks::apply` as the table
  does (a chip only as wide as its text, an icon before it), the text cut with `…` to leave room
  for the right side, a whole-row look (a done task) under the pieces'. The view's own keys win
  over the sender's.
- **The agenda** is `agenda.rs`, a built-in extension (`Extension` in process, named `agenda`,
  reading and editing items) on the path one in a process takes: it follows every item that
  answers to `task` (subtypes too) with `items/query` and `items/subscribe`
  (`type_table::Feed`), reads each one's `status_type` and dates from the item, and sends the
  list with `view/open`. Tasks are filed by the date `cold-lava agenda` files them under (`Task::agenda_date`: due, else scheduled, else start, else its daily
  note's). Groups, in order: Overdue (before today), Today, each of the next seven days that has
  tasks, Later, and Undated last, folded. A task with a non-task status is left out. Open tasks
  (todo, in progress) always show; a task done or cancelled today shows too, where it was, so a
  tick doesn't make it vanish; `d` (`show-done`, the agenda's own key) shows every closed task,
  and the first group is then "Earlier". Rows within a group go by date, then note, then line.
  Each row: the checkbox from the status type, the words with links as their text, `🔁` when
  it recurs, the priority's glyph with the type's look (the built-in task's chip), the whole-row
  look of its status (done and cancelled fade and strike through), and on the right the date
  (in Overdue, red while open, and in Later) and the note's name. Today is `tasks::today()`,
  which `COLD_LAVA_TODAY` pins for scripted runs. The agenda checks the day once a frame
  (`Extension::poll`) and, when it has changed since the list was sent, sends the list again
  filed by the new day.
- **Ticking** is `view/event toggled` to the agenda, which sends `items/edit toggle` with the
  row's id and `rev`; `x` in the reader is the same edit as the screen `reader`. Both reach `tasks::toggle`, as `task
  toggle --id` does: done dates, recurrence, custom statuses' next symbols, the cancelled
  date. A task changed since the agenda was read is refused with the reason on the status line.
  After the call the menu reads the edit back, the agenda hears it through its subscription and
  sends the list again with `select` naming the ticked task as it reads now, so it stays
  selected under its new line (a recurring task's next occurrence goes above it).
- **Live and back.** The agenda sends the list again on every `items/changed` its subscription
  hears, and the view keeps the selection, the folds and the filter. Enter is `view/event
  activated`, and the agenda asks for `workspace/open` at the task's line, where `x` ticks that
  same task; Back (`Ctrl-o`) returns to the agenda's view as it was, since a view is a place
  while it is open. `d` is `view/event command show-done`, and the agenda sends the list again
  with or without the closed tasks. `A` (`agenda.open`) stays a core command, with its place in
  the help and which-key, and the menu hands it to the agenda as `command/run`. The drive script
  (`agenda.drive`) is the one the screen had, unchanged.
- **Not yet:** nesting subtasks under their parent (`depth` is there; the agenda sends every
  task at depth 0), and a range other than the next seven days.

## File types

*(design; owners in manifests, `file/open`, `file/changed` and the file messages exist: "File
types, as built" below; so do items and links from owners and edits through them: "Owners and
types, as built". Previews and the whiteboard as an owner are design.)*

Markdown is the engine's own format. Any other kind of file (`.canvas`, `.kanban`,
`.excalidraw`, `.csv`) can have an **owner**: an extension that says so in its manifest.

```toml
[[file-types]]
extension = "kanban"       # owns *.kanban files
title = "Kanban board"
new = "kanban.new"         # the command that makes one
```

- **Opening.** Selecting a `.kanban` file in the tree, clicking it on a canvas or following
  `[[Projects.kanban]]` sends its owner `file/open {path}`. The owner reads it through the workspace
  gateway (`workspace/read` gives `{text, rev}`), parses its own format, and describes a view
  through the view gateway (a scene, a table, Markdown). A file with no owner shows its name, as
  now; `workspace/files {extension}` lists files of a kind, for pickers.
- **Saving and creating.** `workspace/write {path, text, expect: rev}` is refused if the file changed
  since it was read, so the owner re-reads, merges and tries again. `workspace/create` places a new
  file where Obsidian would put it and never overwrites. Outside changes arrive as
  `file/changed`, from the core's one watcher.
- **Joining the workspace.** An owner contributes its file's **items and links** ("Owners and
  types" below). A task on a kanban card is a `task`, queried and listed with Markdown tasks,
  and edited through its owner. A card's `[[Lava]]` is reported as a link that the core
  resolves by Obsidian's rules, so `Lava`'s backlinks list the board, the graph draws the
  edge, and renaming `Lava` updates the board through its owner. Other file types are then part
  of the workspace, not opaque files only one extension understands.
- **Previews.** `![[Projects.kanban]]` in a note, or a file card on a whiteboard, asks the owner
  for a small view of the file.
- **The whiteboard is the first owner:** `.canvas`, drawn as a scene, merged three ways on save,
  contributing its cards and their links. Images get a view kind of their own (`image`, drawn
  with Kitty pixels where the terminal has them), since every frontend can show a picture.

### Owners and types

*(design; Markdown is behind the owner interface, and owners in other processes give items and
links, and edit and relink their files: "Owners and types, as built" below. Owners on top of
Markdown are design)*

**Separated by file type, united by type data.** Types are what the user works with; a file type
only decides how data is written down. A task is a `task` whether it is a line in a note, a card
on a whiteboard or a row in a CSV, and whatever reads tasks (the agenda, the table, an extension
that copies them to a calendar) gets all of them without knowing any format. cold-lava enables
the user's data; it doesn't lock it into the file it was typed in.

An owner translates between its format and the workspace's types. It has three jobs; the engine
keeps everything that has to be shared and trusted:

| Job | The owner | The engine |
| --- | --- | --- |
| read | the file's text → items and links | the one watcher, a cache by `rev`, the index, the link resolver, types and their matching |
| write | an item edit → the file's new text | permissions, refused if the file changed, the read-back check, attribution, the safe write |
| show | a view of the file (`file/open`) | views, history, keys |

**Markdown is the first owner**: the engine's own, in-process, and it stays the main one, since
people write notes. The next slice puts today's Markdown reading and writing (`items.rs`, the
link parser, `edit.rs`, `rename.rs`) behind the owner interface with no change in behaviour,
the way the agenda proved the view gateway, before any other owner contributes items.

- **Two ways to give items.** A format with Markdown inside (a whiteboard's text cards, a kanban
  card) hands each piece's text to the engine's Markdown reader with its place (`{text,
  place}`), so the built-in `task` type and the user's types match there exactly as in a note,
  and the owner knows nothing about tasks. A format without Markdown (`.csv`, `.ics`, `.bib`)
  gives typed items directly, fields by name, for a type the user defined or one the owner
  ships (`[[types]]` in its manifest, origin `extension`), the way table rows in notes are
  matched by column name today.
- **Where an item lives.** An item says its file and a place in it the owner keeps stable: a
  canvas node id, a CSV row's key column, lines in Markdown. `workspace/open` takes the same
  place, so opening an item opens its file's owner at it.
- **Edits go back through the owner.** `items/edit` on an item in an owned file asks the owner
  for the file's new text (`file/edit {path, rev, id, change}` → `{text}`; for a Markdown
  piece the engine edits the piece's text and the owner puts it back in place). The engine
  then writes it with the usual checks, reads the file back through the owner and checks the
  item now says what was asked, so an owner can't quietly write something else. The extension
  that asked is named, as for any edit. Extensions that work on data never learn about formats.
- **Links.** An owner reports its links (`{target, place, text}`); the core resolves them by
  Obsidian's rules, so backlinks, the graph and tidy see them, and a rename asks each owner
  whose file links to the renamed note for that file's new text.
- **Cost.** Markdown stays in-process. An owner in its own process reads a file only when its
  `rev` changes; the core caches what it gave, by `rev`, under `~/.cache/cold-lava`, so a
  workspace opens without waking every owner. A broken or slow owner leaves its files' items
  out, and says so, instead of stalling the scan.
- **Owners on top of Markdown.** Who shows a file needn't be who reads it. A claim to *show*
  can match notes the way a type's `where` does (frontmatter, tag, folder): Obsidian's Kanban
  plugin keeps a board as a `.md` file with `kanban-plugin` in its frontmatter, so a kanban
  owner shows those as a board while Markdown still reads their tasks and links, and Obsidian
  still opens them. A `#flashcards` note can open in a review mode; a type can have a view of
  its own. The most specific claim shows; reading and writing stay with the format's owner.
  A new format of one's own is the user's choice, never a default, since Obsidian can't open it.

An example end to end: `- [ ] Call the lab 📅 2026-11-12` typed on a whiteboard card. The canvas
owner hands the card's text to the Markdown reader and it is a `task`. An extension that syncs
tasks to a calendar finds it with `items/query {type: "task"}` beside every task in the notes;
moving the event sends `items/edit set-field due`, the canvas owner rewrites that card's text,
the engine saves it safely and reads it back, and the agenda, the table, backlinks and the
graph all see the new date. None of them knew what a canvas is.

### File types, as built

- **Claiming a kind.** A manifest's `[[file-types]]` (`ext::FileType`: `extension`, `title`,
  optional `new`) names the kinds of file the extension opens. `extension` is read lowercase
  without its dot, and must be letters, digits, `-` and `_` (`tar.gz` is refused); `new` must
  be one of its own `[[commands]]`, which joins the palette and keys like any other. A mistake
  leaves the extension out, as other manifest mistakes do. Built-in extensions declare kinds
  in the `Manifest` they return, so both kinds of extension take one path.
- **One owner per kind.** `Registry` gives each kind to the first extension that claims it, in
  load order (built-in ones first, then the user's by folder name). A second claim, and any
  claim on `md` (notes are the engine's) or `canvas` (the whiteboard is the core's until it
  becomes an owner), is left out and reported like a command conflict: on the status line at
  start and in `cold-lava keys`. `cold-lava ext list` prints each claim as `opens: .csv  CSV
  table, new: csv-table.new`, with `(left out: … opens them)` on the one that lost.
- **Where owned files show.** In the note tree (`▤`, by their whole file name; the right pane
  says `CSV table · ⏎ to open it in csv-table`), in the quick switcher after the notes (with
  their folder and kind), as links (`[[Eruptions.csv]]` resolves to the file the way an
  attachment link does), and as bookmarks. Enter on any of these, or following the link, sends
  the owner `file/open {path}`. The owner shows the file in a view; the view's `view/open`
  puts where you were in Back's history, so `Ctrl-o` returns to the note the link was in. A
  kind nobody owns behaves as before: not in the tree, and a link to it creates a note. A link
  to an owned file resolves in the index and the graph, and the file's own links are
  backlinks ("Owners and types, as built").
- **File messages** (`crates/engine/src/files.rs`, answered by the core: `Core::read_file`,
  `write_file`, `create_file`, `files`). Paths are workspace-relative, `/`-separated:

  | Request | Answer |
  | --- | --- |
  | `workspace/read {path}` | `{text, rev}`; a file that isn't UTF-8 is `refused` |
  | `workspace/write {path, text, expect}` | `{rev}`, the new version; `changed` unless the file is still the version `expect`, or when it is gone |
  | `workspace/create {path, text}` or `{name, extension, text}` | `{path, rev}`; at `path`, refused as `exists` when something is there, or as `name.extension` where Obsidian puts a new file (`newFileLocation` in `.obsidian/app.json`, as for a new note), `name 1.extension` and so on when taken |
  | `workspace/files {extension}` | `{files}`, every file with that extension (any case, with or without the dot) outside hidden folders, sorted |

  A `rev` is 16 hex digits of the FNV-1a hash of the file's bytes. A write checks it, then
  writes through `write::replace`, which checks the bytes again just before the atomic rename,
  so an edit made in Obsidian meanwhile is never overwritten. A create goes through
  `write::create`. A note written or created this way is read again at once, like an item
  edit, so the next query sees it. A path is refused unless it is relative, has no `..`, `.` or
  hidden part (`.obsidian`, `.trash`, `.git`), and leads inside the workspace once symlinks are
  followed (for a new file, its deepest folder that exists).
- **`file/changed {path, rev}`** (host → extension, a notification) comes from the core's one
  watcher: every file that isn't a note, outside hidden folders, is now reported as
  `Event::FileChanged` once things settle, and the menu tells the owner of its kind, with its
  `rev` as it is now (`null` when it is gone). Its own writes come back too, with the `rev` its
  write answered, so an owner compares and ignores those. A file created or removed outside
  also updates the tree.
- **Permissions.** An owner may read, write, create and list files of its own kinds with no
  permission at all: opening them is what it is for. Anything else needs `files.read`
  (`workspace/read`, `workspace/files`) or `files.write` (`workspace/write`,
  `workspace/create`), which together cover notes too, as text. The check is `Host::files_core`,
  beside the items' permissions, so it holds for built-in extensions and over the wire alike. The
  menu's screens (`reader`, `table`) get neither. Refusals are `forbidden`, naming the
  permission.
- **The example** is `examples/extensions/csv-table` (Python, no dependencies): `file/open` reads
  the file and sends a table view (a text column per header cell, a row per line); `view/event
  edit` sets the cell and sends `workspace/write` with the `rev` it showed, and on `changed`
  reads the file again and sets the cell in what is there now (up to three times);
  `file/changed` with another `rev` sends the table again; `csv-table.new` creates
  `Untitled.csv` and opens it. On `file/open` of a file it already shows, it reads the file
  again, sends the table with `view/update` and brings it to the front with `view/show`, so
  the selection, sort and filter stay. `csv-table.drive` checks it through the menu (open from
  the tree, an edit saved, a change made outside with the drive step `write`, a link, the
  switcher bringing the filtered table back, a new table), and `ext/process.rs` checks the
  retry.
- **Not yet:** previews, the whiteboard as an owner, and a view kind for images.

### Owners and types, as built

- **The owner interface** is `crates/engine/src/owner/`. Markdown is the first owner, in-process
  (`owner/markdown.rs`): `read` (a note's links, tags and aliases, when it is scanned), `items`
  (on first use), `edit` (one item → the note's new text: `edit::plan` for `set-field`,
  `tasks::toggle_text` for `toggle`) and `relink` (rename's link rewriting; the `Rewriter` only
  says what each link becomes). The engine writes what `edit` made with `write::replace`, so a
  note is written in one place. `scripts/bench-items.py` measured no change.
- **`file/read`** (host → owner, a request) `{path, rev, text}` answers `{items, links}`
  (`owner/given.rs`, pinned by tests). An item is a Markdown piece `{place, text}`, read by the
  Markdown owner in the workspace's flavour, so `task` and the user's types match in it as in a
  note, or a typed item `{place, type, fields}` of a loaded type, each value read as its
  field's type (`Matcher::given`). A link is `{target, place, text}`; the links in pieces are
  read with them. Unknown keys are refused, and a type that isn't loaded or a field it doesn't
  have is reported and that item left out.
- **Where an owned file lives.** `Workspace::owned`, beside the notes: each file with its owner,
  the `rev` its items and links were read from, its items and its links, and the owner's place
  for each (`Owned::places` by item id, `Owned::links`), kept off the items so notes' items
  don't grow. An item's id hashes its file, its kind and the owner's place (and, in a piece,
  its key there, as in a note), so it is stable across processes and restarts. `items/query` and `items/subscribe` answer from
  the notes, then the owned files; a filter's `folder` sees the file's path, and `tag` matches
  none. Subscriptions key their state by file path with its extension, so an owned file's news
  is one more `items/changed`.
- **Links.** The resolver finds owned files by path or by name with their extension
  (`Resolver::resolve_owned`), so `[[Eruptions.csv]]` counts as an outgoing link and the graph
  draws the file as a node. An owned file's links resolve like a note's: they are backlinks
  (`index::From::Owned`, with the place), edges in the graph, and tidy counts them (a note they
  link to isn't an orphan, a link to nothing is unresolved). Enter on such a backlink sends the
  owner `file/open {path, place}`.
- **Opening at a place.** `workspace/open` takes an optional `item` (an id): on a file another
  owner reads, its owner gets `file/open` with that item's place. The agenda and the table pass
  the row's item. `view/open` takes an optional `select`, so the owner opens its view at the row
  before the first frame.
- **Asking, and the cache** (`owner/cache.rs`, `Core::set_owners`, `take_reads`,
  `file_given`). When the menu opens a workspace it tells the core each owned kind with its
  owner's name and version. The core lists those files, reads each one's `rev`, and takes what
  the owner gave for that `rev` from `~/.cache/cold-lava/owners/<hash of the workspace's
  path>-<owner>-<version>.json` (`{owner, version, files: {path: {rev, given}}}`, written after
  the frame an answer came in); a file it has nothing for is still in the index, with no items,
  and its owner is asked. The cache keeps the owner's answer, not items, so a change to the
  workspace's types applies at once. The watcher's `FileChanged` for an owned kind reads the
  `rev` again and asks again when it changed; a file removed leaves the index and the cache.
- **Answers arrive later.** `Process` sends `file/read` as a request and hands the answer to
  `Host::file_given`, which only the file's owner may call; the core installs it when the file
  is still that `rev` (else asks again), resolves links again and queues `items/changed` and
  `Event::OwnedChanged`, which the menu reads before it draws (backlinks, counts, news). An
  error, an answer that doesn't parse, no answer in 10 s or an owner that stopped is
  `Event::OwnerTrouble` on the status line, and the file keeps what it had. Nothing waits for an
  owner: the scan and the menu go on.
- **The example** `examples/extensions/csv-table` answers `file/read` with each row as a one-row
  Markdown table under the file's header (place: its first cell), and a cell that starts with
  `- ` as Markdown of its own (place: `row#column`). `Volcanoes/Eruptions.csv` (`Volcano, Year,
  VEI, Notes`, with `[[links]]` and a task) is then `eruption` rows, matched by a second
  table-row shape of that type in the example workspace's `types.toml`, five backlinks on
  Krakatoa and a task in the agenda. `csv-items.drive` checks it through the menu, and
  `ext/process.rs` through the real transport.
- **Edits through the owner** (`owner/edits.rs`, `Core::edit_item`, `file_edited`).
  `items/edit` on an item of an owned file makes the change to what the owner gave: on a
  Markdown piece, the Markdown owner's `edit` on the piece's text (kept on `Owned::given`, so
  `toggle` and `set-field` behave exactly as in a note); on a typed item, `{field, value}`
  (`toggle` is refused there: a typed item has no checkbox to tick). It is refused at once when
  the item isn't the version `rev`, the file on disk isn't the version the owner read, or
  another edit of the file is waiting for its owner. Then the owner is asked:

  | Request (host → owner) | Answer |
  | --- | --- |
  | `file/edit {path, rev, text, changes}` | `{text}`, the file's new text |

  A change is `{place, text}` (a piece's new text), `{place, field, value}` (a typed item's
  field) or `{place, link, to}` (a link the owner reported, for a rename), at one of the owner's
  places (`given::Change`).
- **Read back before writing.** The owner's new text is handed back to it as `file/read` with
  its own `rev`, and the answer must read as what the owner gave before with the changes made
  (`given::apply`): the same items, compared by kind and fields, and the same link targets,
  ignoring ids and places, since a place can be part of what changed (a CSV row's first cell).
  Only then is the text written, with `write::replace` against the text the edit began from, so
  a change made meanwhile refuses it as `changed`. Notes are read back before they are written
  too; here it costs the owner one more read, and a mismatch writes nothing. What the owner
  gave for the new text is kept for its `rev`, so the watcher's `FileChanged` asks nothing
  again. Then `OwnedChanged` and `ItemEdited`, with `note` the file's path and `by` whoever
  asked, as for a note. A refusal (`refused`), an answer that isn't text or is the old text, a
  read-back that differs (`read-back`, "nothing was written"), no answer in 10 s or an owner
  that stopped all fail the edit with the owner's name and why, and the file is as it was.
- **Answered later.** `Core::edit_item` answers `Answer::Now` for a note and `Answer::Later
  (ticket)` for an owned file; `Core::take_edited` has the answers by who asked, which the menu
  hands out after the frame's item news. Over the wire nothing changes: `rpc::serve` keeps the
  request's id and `Process` sends the same JSON-RPC response once the answer is in, so a caller
  can tell a note from an owned file only by how long it took. A built-in extension gets it as
  `Extension::edited(ticket, answer)`: the agenda and the table keep what they asked by ticket
  and say how it went then. The menu never waits: requests go out with the frame's `file/read`
  requests (`Core::take_file_edits`), `Process` times them out after 10 s like reads, and the
  core holds at most one edit per file.
- **Renames.** `rename::plan` also lists the owned files whose links change, with their owner
  (`FileEdit::owned`): each Markdown piece relinked by the Markdown owner's `relink` as a note
  is, and each link the owner reported on its own, through the same `Rewriter`. A file whose
  owner hasn't read it as it is now is left out with a warning. The owners are asked before
  anything moves: once the rename is confirmed, `Core::relink` asks each owner `file/edit` with
  the plan's changes and has it read the new text back, as for an item edit, but keeps the text
  instead of writing it. `Core::take_relinked` hands back every owned file's edit with its new
  text (`rename::answered` puts them into the plan), and only then does `rename::apply` check
  that no file changed, make the moves and write every file, the owned ones as the notes are.
  `apply` refuses a plan whose owners haven't answered. An owner that refuses, writes something
  else or doesn't answer in 10 s cancels the whole rename with its reason ("rename cancelled,
  nothing was changed: csv-table didn't edit Volcanoes/Eruptions.csv: …"), and every file is as
  it was. The menu doesn't wait: the rename dialog stays open saying "waiting for csv-table to
  update its files", and Esc stops waiting (`Core::cancel_relink`) and drops the late answer.
  What each owner read in its new text is kept for the file's new `rev` (`Core::relinked`), so
  nothing is asked again after the move.
- **The example** puts each piece back where it came from: a row's one-row table into the row's
  cells (keeping its Markdown cells, which the row showed empty), a Markdown cell into its cell,
  and refuses a row without its first cell, which names it. `csv-edit.drive` ticks the CSV's
  task in the agenda, sets a row's VEI in the table and renames Krakatoa, and `tests/drive.rs`
  reads the CSV after; `csv-edit-refused.drive` runs a copy that refuses every edit and doesn't
  answer one, and the CSV is unchanged. `csv-rename-refused.drive` runs a copy that refuses one
  rename and doesn't answer another, then cancels a third with Esc, and every file of the
  workspace is byte for byte as it was. `ext/process.rs` checks the late JSON-RPC answer and
  the timeout through the transport.
- **Not yet:** a merge (`merge.rs`) doesn't rewrite links in owned files, the CLI's
  `rename` and `merge` don't either and its `agenda`, `items` and `table` don't read owned files
  (they start no extension; the cache could serve them), owner-shipped types need a shape like
  any type, and an owned file has no backlinks pane of its own.

### Link resolver

*(design; step 1, Obsidian's rules in one engine module, is built: "Link resolver, as built"
below)*

**Owners find links; the resolver says what they mean.** An owner reports what is written and
where (`[[Lava#Textures|molten rock]]` on line 12, a CSV cell's `[[Krakatoa]]`). The resolver
decides which file or item that link leads to. The index builds everything else from those
answers: backlinks, the graph's edges, tidy's orphans and unresolved links, and the set of links
a rename rewrites. Resolving is a role, like owning a file type, and Obsidian's rules are its
built-in default, as Markdown is the default owner.

**One resolver per workspace, never one per extension.** Backlinks, the graph and renames must
agree on where a link leads, so they ask the same resolver. The default follows Obsidian's
rules, and that is what keeps "the same link opens the same file in Obsidian":

- `#heading`, `#^block` and `|alias` are split off first.
- A note is found by its workspace path, then by its name anywhere; of several notes with
  that name, the shallowest wins.
- An attachment or another owner's file is found by its path, then beside the linking note,
  then by its name with its extension.
- `./` and `../` start from the linking file's folder.
- A link that leads nowhere creates its note where Obsidian would put a new one
  (`newFileLocation` and `newFileFolderPath` in `.obsidian/app.json`), or in the folder the
  link names.

**It works in both directions, and the two agree.**

| Direction | In | Out | Used by |
| --- | --- | --- | --- |
| resolve | the link's text and the file it is in | a file (later, an item) | the index, the graph, tidy, following a link |
| write | a target and the file the link goes in, in a style | the link's text | renames and merges, inserting a link, the note behind an unresolved link |

A link the resolver writes resolves back to the target it was written for. A rename depends on
that: it writes each link again in the style it had (`[[Name]]` short, paths stay paths,
relative links stay relative), and the link must still lead to the renamed file.

**A pure function, run in batches.** The answer depends only on the link's text, the linking
file and the workspace's file list (later, its items too). So the engine can cache answers and
run the resolver again over every link when files come and go. It runs for every link on every
change, so a resolver runs in-process (Rust, later WASM) or takes a whole batch at once. A
round trip per link is never acceptable.

**Plan.**

1. The interface in the engine, with Obsidian's rules as its only implementation. *(built)*
2. Fallbacks from extensions: for a link the default leaves unresolved, an extension may name
   the target, including an item and not only a file. For example, `[[@smith2020]]` leads to an
   entry a `.bib` owner gave, and `[[202611081432]]` leads to a Zettelkasten note by its id.
   The default answers first, so a link Obsidian resolves keeps its meaning.
3. Replacing the rules, as a setting of the workspace: Logseq-, Dendron- or Foam-style rules,
   or a Hugo site's. This breaks parity with Obsidian, so it is the user's deliberate choice and
   never a default.

### Link resolver, as built

- **One module.** `crates/engine/src/resolver.rs` holds both directions of Obsidian's rules.
  It is functions and data shapes, not a trait: a trait waits for a second implementation
  (step 3).
- **Resolve.** `split_link` splits off the heading, block and alias. One set of rules
  (`Table::find`) answers every lookup, from the linking file's folder: `./` and `../` start
  there; any other link is a workspace path, then (an attachment, or a path with a folder) a
  path from that folder, then a name, the shallowest winning. `Resolver` is the index's table
  of notes and owned files (`resolve(dir, target)`, `resolve_owned`): backlinks, the graph,
  following a link to a note, the reader's unresolved-link look and the CLI's `rename NOTE`
  use it. `Files` is the same table over every file on disk: renames, merges, tidy and
  `workspace/files` use it, and so do `resolver::attachment` and `resolver::file`. So a
  backlink, a graph edge, following a link and a rename always name the same file.
- **Write.** `Files::write(target, dir, style)` writes a link to a file from a folder.
  `Style` is how a link is written: `Wiki {relative, folder, extension}` or `Markdown {root,
  like}`. A relative link into the linking file's own folder is a bare name (`[t](Top.md)`), as
  Obsidian writes it, unless that name leads elsewhere (Obsidian reads a bare name from the
  workspace's root, then by name); then it starts with `./` (`[t](./Top.md)`).
  `Rewriter::link` resolves a link before and after a move and writes it again in its own style
  when it leads elsewhere; Markdown's owner (`owner::markdown::relink`) finds the links and puts
  the new text in place. `resolver::new_note_path` and `new_file_path` say where
  a new note or file goes.
- **Agreement is tested.** `resolver.rs`'s tests write a link to every file from every note in
  every style, over the example workspace and over a fixture with same-name notes, and check
  that `Files` and `Resolver` both lead it back to the same file. `graph.rs` checks each `./`
  and `../` form in backlinks, the graph, following the link and a rename, and
  `relative-link.drive` follows a `../` link in the reader.
- **What Obsidian does, and what is open.** Obsidian documents none of this; the rules follow
  what its forum reports. A bare name or path is looked up from the vault's root first, so
  `[t](Top.md)` in `B/Note.md` opens a root `Top.md` over `B/Top.md`, and `./` makes a link
  relative ([forum
  69542](https://forum.obsidian.md/t/absolute-link-path-has-higher-precedence-than-relative-path/69542)).
  Obsidian itself writes a same-folder link as a bare name, a known bug ([forum
  41786](https://forum.obsidian.md/t/relative-path-link-points-to-a-wrong-file-with-same-file-name/41786));
  cold-lava writes `./` there instead. No source says how Obsidian reads `../`; cold-lava takes
  it from the linking file's folder, so the link leads where its path says. Items as targets and
  fallbacks are step 2.

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
/// A message-framed connection to one extension. None of these wait for the extension.
pub trait Transport: Send {
    /// Queue one whole message (a serialized JSON-RPC object).
    fn send(&mut self, message: Vec<u8>) -> io::Result<()>;
    /// The next message that came in: a message, nothing yet, or closed and why.
    fn try_recv(&mut self) -> Received;
    /// End the connection; a process still running after `grace` is killed.
    fn close(&mut self, grace: Duration);
}

/// Knows how to open one kind of transport, by the `transport` value in a manifest.
pub trait Connector: Sync {
    fn kind(&self) -> &'static str;
    fn connect(&self, launch: &Launch) -> io::Result<Box<dyn Transport>>;
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

### Transport, as built

- **Where it lives.** `crates/cold-lava/src/ext/`: `frame.rs` (the framing), `transport.rs`
  (`Transport`, `Connector`, `CONNECTORS` and the stdio connector), `rpc.rs` (JSON-RPC
  messages, and `serve`, which answers an extension's request through `Host`), `process.rs`
  (`Process`, the `Extension` that speaks for one in another process) and `installed.rs`
  (manifests on disk). A guide for extension authors is [extensions-guide.md](extensions-guide.md),
  with an example in Python in `examples/extensions/open-tasks`.
- **Framing: LSP's.** `Content-Length: N\r\n\r\n` then N bytes of UTF-8 JSON; other header
  lines are ignored, a bare `\n` is read too, and a message over 64 MiB is refused. Chosen over
  newline-delimited JSON because a message is read whole without scanning its bytes, a stray
  newline or a pretty-printed message can't split one, and every language has LSP code to copy.
  Bad framing on stdout ends the connection, as the stream can't be trusted after it.
- **Never waiting.** The trait differs from the sketch above: `try_recv` instead of a blocking
  `recv`, since the menu polls. The stdio transport runs a reader thread and a writer thread
  per extension, each behind a channel of 256 messages: an extension that floods cold-lava waits
  on its own pipe, and one that stops reading its input is stopped once the 256 fill up, instead
  of making the menu wait. The menu calls `Extension::poll` each frame (every 50 ms when an
  extension runs in a process), which reads at most 64 messages and answers them.
- **Starting and stopping.** A process starts when the menu opens a workspace
  (`Extension::workspace`) in the extension's folder, its stderr written to
  `~/.cache/cold-lava/extensions/NAME.log`, new each start. Opening another workspace stops it
  and starts a new one, since its subscriptions belong to the old workspace. At quit, and before
  that new start, it gets `shutdown`, then `exit`, then the end of its input, and is killed if it
  is still running 500 ms later. The CLI's commands (`agenda`, `items`, `keys`, …) never start
  one.
- **Time limits.** An extension that doesn't answer `initialize` in 10 s is stopped. A
  `command/run` it doesn't answer in 10 s is reported (`open-tasks didn't answer
  open-tasks.count in 10 s`), and it keeps running.
- **A crash** (its output ends, or it can't be written to) is said once on the status line with
  the last line of its log, its `ui/status` text is cleared, its subscriptions are stopped, and
  its commands are left out of the palette and their keys do nothing. `extensions.restart`
  ("restart extensions" in the palette) starts the stopped ones again.
- **Security.** An extension runs as you, as Obsidian's and Neovim's plugins do. Its
  `permissions` limit what the workspace gateway does for it; they don't sandbox the process,
  which can read and write files like any program you run. That is why only extensions in your
  config folder are loaded (see "Manifest").

## Protocol

JSON-RPC 2.0. Both sides can send requests and notifications.

- The host calls the extension: `initialize` (capabilities, the API version and the
  `commands` it registers), `command/run` (`id`, `args`: one of its commands),
  `view/render`, `shutdown`, and `file/open`, `file/changed` and `file/read` for a kind of file
  it owns.
  - The editor gets `editor/open` (a note, and a `jump` to a line, heading or block),
    `editor/save`, `editor/renamed` (`from`, `to`) and `editor/deleted` (`note`: it went to
    the trash; forget it without writing it back).
  - An extension hears from a pane it opened through `pane/message` (`kind` and `payload`, what
    its program wrote as `ESC ] 5379 ; kind ; payload BEL`) and `pane/exited` (with an `error`
    when the program never started).
- The extension calls the host: `workspace/notes`, `workspace/read`, `workspace/write` (through the safe
  write path, with the expected previous content), `links/backlinks`, `search/text`,
  `items/query`, `items/subscribe`, `items/unsubscribe`, `items/edit` and `types/list` (see
  "The workspace gateway, as built"), `view/open`, `view/update`, `view/close`, `view/show`
  and `workspace/open` (see "The view gateway, as built"), `ui/notify`, `events/subscribe`, and:
  - `command/run`: run any command in the menu's table by `id` with `args`, as a key would: a
    core command or another extension's. Core commands ignore `args`, except
    `note.follow-link` (`from`, `link`), which only extensions can run, and `search.open`
    (`query`, optional). An unknown id is reported in the status line.
  - `editor/opened`: the editor shows this note now. It is a report, not a request, so the
    host never answers it with `editor/open`, which would echo.
  - `workspace/saved`: a note was just written, so the host reads it now instead of when the file
    watcher notices.
  - `pane/open` (a `pane` id the extension picks, and the program with its arguments),
    `pane/input` (text to type into the pane) and `pane/close`.
- The host notifies subscribers: `items/changed` and `items/reset` for item subscriptions,
  `view/event` and `view/closed` for an extension's views, `event/noteChanged` and friends.

The method set is versioned (`apiVersion` in `initialize`), so old extensions keep working.
Paths in the `editor/*` messages are absolute and inside the workspace; items, `workspace/open`
and the file messages name them relative to the workspace. A message's tag is its method name, for example
`{"method": "pane/input", "params": {"pane": 1, "text": ":w\r"}}`.

Built-in extensions already use, in-process (`crates/cold-lava/src/ext/mod.rs`): `initialize`
(with `commands`), `command/run`, `editor/open`, `editor/save`, `editor/renamed`,
`editor/deleted`, `pane/message`, `pane/exited`, `items/changed`, `items/reset`, `view/event`,
`view/closed`, `file/open` and `file/changed` from the host, and `ui/notify`, `ui/status`, `command/run`, `editor/opened`,
`workspace/saved`, `workspace/open`, `pane/open`, `pane/input`, `items/query`,
`items/subscribe`, `items/unsubscribe`, `items/edit`, `types/list`, `workspace/read`,
`workspace/write`, `workspace/create`, `workspace/files`, `view/open`, `view/update`,
`view/close` and `view/show` from the extension. The `items/*`, `types/list` and file requests are answered
during the call, and `view/open` names its view at once; the rest are queued.

### Over the wire, as built

- **Host → extension.** `initialize` is a request, with `apiVersion` (1), the extension's
  `name`, `workspace` (`root`, absolute, and `name`), its `permissions`, and
  `capabilities.methods`, the methods it may call; its answer is not read, but an error stops
  it. `command/run` (`id`, `args`) is a request, so a refusal (the error's message) and a
  missing answer show on the status line. `shutdown` is a request, then `exit` a notification,
  as LSP does. `file/read` (`path`, `rev`, `text`) is a request, answered with `{items,
  links}` ("Owners and types, as built"). `items/changed`, `items/reset`, `view/event`, `view/closed`, `pane/message`,
  `pane/exited`, `file/open`, `file/changed` and the `editor/*` messages are notifications, the same `{"method", "params"}` the in-process types pin.
  Commands come from the manifest, not from `initialize`, so the menu's command table and
  `config.toml`'s keys are checked before any extension starts.
- **Extension → host.** Requests, answered with a result or an error: the workspace gateway's
  (`items/query` with `{items, next}`, `items/subscribe` with `{subscription}`,
  `items/unsubscribe` and `types/list`, `items/edit` with `{item, changed, next,
  warning}`, and the file messages of "File types, as built"), and `ui/notify` (`message`), `ui/status` (`text`, `null` to clear: the
  extension's own part of the first status line, kept until it sends another), `command/run`
  (`id`, `args`), `workspace/saved` and `editor/opened` (`note`), `pane/open` (`program`,
  `args`, answered with `{pane}`) and `pane/input` (`pane`, `text`), each answered `null` once
  queued. The view gateway's: `view/open` (`content`, optional `select`, answered with
  `{view}`), `view/update` (`view`, `content`, `select`), `view/close` and `view/show`
  (`view`), and
  `workspace/open` (`note`, `line`, optional `item`);
  a view over the limits is refused as `refused`. The same methods sent as notifications do the
  same and get no answer.
- **Errors.** A refusal of the workspace gateway is code `-32000` with the refusal as `data`:
  `{"code": -32000, "message": "...", "data": {"reason": "forbidden", "message": "..."}}`. Bad
  params are `-32602` (an unknown op or filter key, named in the message), an unknown method
  `-32601`, a message that isn't JSON `-32700` and one that isn't JSON-RPC 2.0 `-32600`, all
  with `data.reason` `refused`.

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
# ~/.config/cold-lava/extensions/word-count/extension.toml
name = "word-count"              # lowercase letters, digits and dashes; the folder's name
title = "Word count"             # optional, for people
version = "0.1.0"
description = "Counts the words in each note"
api = 1                          # the default
transport = "stdio"              # the default
run = ["python3", "main.py"]     # run in the extension's folder
permissions = ["items.read"]    # the default; add "items.edit" to change items

[[file-types]]                   # it opens *.wc files
extension = "wc"
title = "Word counts"
new = "word-count.new"           # one of its [[commands]]

[types.doc]                      # loaded as word-count.doc
on = "note"
fields.words = { property = "words", type = "number" }

[[commands]]
id = "word-count.show"           # must start with the extension's name
title = "Word count of this note"
keys = ["W"]                     # default keys; config.toml can change them
```

### Loading, as built

- **Where from.** One folder per extension in `~/.config/cold-lava/extensions/` (the config
  folder `paths::config_dir` gives), by folder name, after the built-in ones. The folder must
  be named as the extension. `run` is what the sketch called `command`: a word apart from
  `[[commands]]`.
- **Mistakes** are collected like config.toml's: an unknown field, a bad name, a name
  cold-lava keeps (`nvim`, `table`, `agenda`, `reader`), a folder named otherwise, a second
  extension of the same name, an `api` other than 1, a transport cold-lava doesn't have, an empty
  `run`, a permission it doesn't know, or a file type with a bad extension or a `new` that isn't
  one of its commands leave that extension out with one line each. The menu
  says so at start, and `cold-lava ext list [WORKSPACE]` prints every extension (where it runs
  from, its permissions, commands, types and log) and then the mistakes, exiting 1 when there
  are any.
- **Not from a workspace.** A workspace's `.cold-lava/extensions/` would travel with it, so a
  cloned repository could carry a program that runs as you the moment the folder is opened.
  Loading those after a one-time trust prompt (remembered per workspace) is future work; it
  also needs the menu's command table and `config.toml`'s keys to change with the open
  workspace, which today are checked once at start. Until then they are never run, and `ext
  list WORKSPACE` names them as not loaded.

### Permissions

`permissions` names the parts of the workspace gateway an extension may use. Without the
field, an extension may read. *(Exists: `ext::Permission`, checked by `Host`.)*

| Permission | Allows |
| --- | --- |
| `items.read` | `items/query`, `items/subscribe`, `items/unsubscribe`, `types/list` |
| `items.edit` | `items/edit` |
| `files.read` | `workspace/read` and `workspace/files` for kinds of file it doesn't own |
| `files.write` | `workspace/write` and `workspace/create` for kinds of file it doesn't own |

A request without its permission is refused as `forbidden`, with the permission it needs in
the message. An owner needs no permission for files of its own kinds ("File types, as built").
Creating is a write, so there is no `files.create`, and a manifest that names it doesn't load.
An extension changes notes only through the gateway (`items/edit`, or `workspace/write` with
`files.write`), never by writing files itself. The built-in Neovim declares `permissions = []`: it edits
notes as text in its pane, which the core watches like any other editor. The menu's own
screens (the table, the reader's `x`) read and edit as `table` and `reader`; the built-in
agenda asks for `items.read` and `items.edit` like any extension.

Views need no permission. Like `ui/notify` and `pane/open`, a view is the frontend's to draw,
size and close: the user closes it with Esc or `q`, it takes no keys but the ones its list
declares and a table's edit, and it is bounded ("The view gateway, as built"). A permission
would guard nothing an extension can't already do with `command/run` or a pane, and every
extension that shows anything would have to ask for it. What a view shows is read, and what
its events change is written, through the workspace gateway, under `items.read` and
`items.edit`.

## Views

Extensions can't draw into cold-lava's terminal directly. A view is one of:

- **Declarative:** a list, a tree, a table or Markdown, described as JSON and drawn by cold-lava
  (fits most panels: outlines, task lists, search results, a type's items). Rows carry item
  ids, so a command run on the selected row knows what it is about. The table and the list are
  built: "The table view, as built" and "The list view and the agenda, as built" above.
- **A terminal program in a pane:** the extension names a command and cold-lava runs it in a pane,
  the same way the main menu embeds Neovim. Full freedom, any TUI library, but no pixel graphics:
  the pane goes through a terminal emulator.
- **A scene** *(design)*: nodes, edges and labels that the core draws, for the graph, the
  whiteboard, and anything shaped like them (a mind map, a kanban board, a timeline).

### The view gateway, as built

- **Where it lives.** The messages are `crates/cold-lava/src/ext/view.rs` (`Content`, `Event`,
  `ViewNews`) and the `Effect`s in `ext/mod.rs`, pinned by tests; the frontend's open views are
  `crates/cold-lava/src/views.rs`, drawn with the list and table views. An extension reaches
  them through `Host` (`open_view`, `update_view`, `close_view`, `open_note`), and one in a
  process through `rpc::serve`, so both take one path.
- **Extension → host.** `view/open {content, select}` answers `{view}`, an id the host picks,
  unique among every extension's, and shows it at the row `select` names (optional). `content`
  is a list or a table as their JSON is pinned, with a `kind`:

  ```json
  {"jsonrpc": "2.0", "id": 4, "method": "view/open", "params": {"content": {
    "kind": "list", "title": "Open tasks",
    "groups": [{"id": "Plan.md", "title": "Plan", "rows": [
      {"id": "4f…", "text": [{"text": "Pay rent"}], "check": "unchecked",
       "right": [{"text": "2026-11-07"}]}]}],
    "keys": [{"key": "t", "command": "today", "title": "due today"}]}}}
  ```

  `view/update {view, content, select}` replaces what it shows: the frontend keeps the
  selection, the folds, the sort and the filter by row and group id, as the screens always did
  on updates, and `select` (optional) names a row to select now or when it shows up. `view/close
  {view}` closes it. `view/show {view}` brings one of its open views to the front, in place of
  what showed, which Back returns to, as `view/open` does; a view in front already, or not open,
  stays as it is. `workspace/open {note, line}` opens a note (an item's `note`, relative to
  the workspace, or an absolute path inside it; anything else is refused on the status line) at
  a line, from wherever the user is, which Back returns to; with `item`, an item's id, a file
  another owner reads opens in its owner at the item's place. Updates and closes of a view the
  extension doesn't have do nothing. The design's `title` and `place` are not there: the
  content has its title, and there is one place, the main area, until a second is needed.
- **Host → extension.** Notifications: `view/event {view, event, …}` with the list's and the
  table's events flattened in, by row id: `{"view": 3, "event": "activated", "row": "4f…"}`
  (Enter), `"toggled"` (`x` on a row with a checkbox), `"command"` with `command` and `row` (a
  key the list declares), `"edit"` with `row`, `column` and `value` as typed (a table's cell);
  and `view/closed {view}` when the user closed it, or the host did to make room. A view's
  events go only to the extension that opened it.
- **The frontend owns** layout, drawing and keys. A view shows over the main area, one at a
  time, in place of what showed, which becomes the way back. The extension never sees a key: it
  hears the list's declared keys (one character each; the view's own keys win) and the table's
  edit, as events. Esc always closes (once any filter is cleared), `q` closes, and `Ctrl-o` and
  `Ctrl-i` always go back and forward.
- **Filtering as you type.** A list or a table filters with `/` first by default
  (`"filter": "slash"`, left out of the JSON). With `"filter": "typing"` it filters as soon as
  you type, for a view whose whole job is picking a row (the table's type picker). The rule:
  every character typed without Ctrl or Alt goes into the filter, and so do Backspace and
  `Ctrl-u` (clear). Everything else keeps its meaning: the arrows (a table's columns, a list's
  folds), PageUp/PageDown, Home/End, `Tab` (a list's fold), Enter (activate the selected row:
  there is no prompt to end first), Esc (clear the filter, then close), and `Ctrl-o`/`Ctrl-i` (Back and Forward; bound to a plain character
  in `config.toml`, that character is typed instead). So the view's letter keys (`j`/`k`,
  `h`/`l`, `g`/`G`, `0`/`$`, `s`/`S`, `e`, `x`, `q`, `/`) are out of reach, and a list that
  binds `keys` can't ask for it: `view/open` and `view/update` refuse it (`refused`). The
  status line shows the filter as `/text`, the hint line "type to filter".
- **History.** A view is a place (`Place::View`, labelled with its extension's name) while it
  is open, and keeps how it was while out of sight, so Enter on a row, then Back, finds it as it
  was, updated meanwhile. A closed view, and every view of an extension that stopped or of a
  workspace that was closed, leaves Back's history, so Back skips it.
- **Bounds.** A view has at most 50,000 rows and groups together (`view::MAX_ROWS`), 64
  columns and 32 keys, else `view/open` and `view/update` are refused (`refused`, with the
  count). Updates wait for the frame and only the last one each view got is drawn
  (`Views::settle`), so an extension sending one per change costs one rebuild per frame; the
  frame's effect budget and the 64 messages read per poll bound the rest. An extension has at
  most 8 views open (`views::PER_EXTENSION`): opening a ninth closes its oldest, with
  `view/closed`.
- **The built-in agenda** is an extension on this path ("The list view and the agenda, as
  built"), and the example in `examples/extensions/open-tasks` sends a list of its own
  (`open-tasks.list`), checked through the real transport (`ext/process.rs`) and through the
  menu (`open-tasks-list.drive`).
- **Not yet:** the table as an extension ("The table view, as built"), Markdown and tree views,
  a view beside the tree rather than over it, and `scene` (below).

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
  writes with `workspace/write` expecting it, merging and retrying when refused.

The graph moves first: it's read-only, already draws through a backend-neutral `Scene`, and is
where the performance risk is (a 5k-note workspace is about 1.3 MB of `view/open`, sent once). The
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

1. Core: safe writes, live workspace, events. *(done)*
2. Commands, command palette, configurable keys. The Markdown reader can land alongside, since
   it's core.
3. The `Extension` interface; move built-in features onto it, starting with the Neovim editor.
   *(in progress: the interface, extension commands and the Neovim editor exist)* Then, in
   small PRs each checked with `cold-lava drive`:
   1. Notes as data: tasks, lists, headings and block ids, with `agenda` and `task toggle`
      rebuilt on them and `cold-lava items WORKSPACE KIND`. *(done: `items::extract` reads a
      note in one pass, `Workspace::items` caches it per note, ids and revs are pinned by tests,
      and the item JSON is pinned for `items/query`)*
   2. Ids, revs and safe edits; in the reader, `x` ticks the task under the cursor. *(done:
      `task toggle --id` and `edit::set_field` refuse a stale `rev`; `x` ticks the task the
      selected link is in, else the highlighted line's, else the first on screen below it, with
      the `rev` the core read, through `tasks::toggle`)*
   3. The protocol for items, and reads through `Host`. *(done, in-process: `gateway.rs`,
      `items/query`, `items/subscribe` with `items/changed` and `items/reset`, `items/edit`
      `set-field` and `toggle`, `types/list`; permissions in the manifest; every edit
      attributed; the table reads and edits through it, the agenda and the reader tick through
      it)*
   4. The declarative list view, and the agenda as an extension. *(done: `list.rs`, and
      `agenda.rs` as a built-in extension over `items/query`, `items/subscribe`, `items/edit`,
      `view/open` and `workspace/open`; `A`)*
   5. Types, in small steps: the loader and `cold-lava types` (with
      `example-workspace/.cold-lava/types.toml`: `volcano` notes and `eruption` table rows)
      *(done: `types::load`, definitions' JSON pinned for `types/list`)*;
      primitives with spans; typed blocks *(done for notes, table rows, list items, headings and
      paragraphs: `matcher.rs`, `cold-lava items WORKSPACE TYPE`)*; inline types *(done: in
      paragraphs, list items and tasks, headings and table rows, with the block as parent;
      `vei` in the example workspace)*; `set-field` with the read-back check *(done:
      `edit::set_field`, `cold-lava item set`)*;
      the task as a definition, first in shadow mode beside today's parser, then switched
      over *(done: `types/task.toml` is how tasks are read; `agenda` and `task toggle` read
      typed tasks)*;
      looks in the reader *(done: `looks.rs`, `markdown::parse_marked`; the task's done and
      cancelled lines and priorities, the example workspace's elevation and VEI)*; the table view
      *(done: `table.rs`, `type_table.rs`, `i`, `cold-lava table`)*. Types are done.
   6. File types: owners in manifests, `file/open`, `workspace/files`, `workspace/create`, items and
      links contributed by owners; the whiteboard as the first owner. *(in progress: owners in
      manifests, `file/open`, `file/changed`, `workspace/read`, `workspace/write`,
      `workspace/create` and `workspace/files`, with the `csv-table` example, are done ("File
      types, as built"), and so are Markdown as the first owner behind the owner interface and
      items and links from owners in other processes, cached by `rev` ("Owners and types, as
      built"), and so are edits and renames through owners (`file/edit`), and the link
      resolver's first step, Obsidian's rules in one module for both directions ("Link
      resolver"); next is the whiteboard)*
   7. Scenes: `drive` learns drag and scroll, and scripts pin today's graph and whiteboard. The
      scene view is extracted from the graph, runs in the menu's loop, and the graph becomes an
      extension. Then the whiteboard.
4. Transports (stdio first), JSON-RPC, manifests; an example extension in Python and docs.
   Extension authors test their extension's UI with `cold-lava drive` scripts (snapshot tests).
   *(in progress: the stdio transport, JSON-RPC, manifests in `~/.config/cold-lava/extensions`,
   `cold-lava ext list`, `ui/status` and the `open-tasks` example in Python, checked by a drive
   script, are done ("Transport, as built"), and so is the view gateway, so an extension can
   send the list and table views (`view/open`, "The view gateway, as built"), and the menu's
   table is an extension on it; next are workspace-local extensions behind a trust prompt)*
5. Installing extensions (`cold-lava ext install <git-url>`), permissions, versioning.

## Compatibility with Obsidian plugins

Obsidian plugins are JavaScript that draws into Electron, so running them unchanged isn't a
goal. Data compatibility is: cold-lava keeps reading and writing the same files and settings
(Tasks, Daily notes, graph colour groups, Canvas). A TypeScript extension could reuse a plugin's
logic that doesn't depend on its UI.
