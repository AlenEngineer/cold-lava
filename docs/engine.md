# The engine

Status: **overview**. Parts are built, parts are designed in [extensions.md](extensions.md), and
parts are still to be designed; each layer says which.

cold-lava is two things: an **engine** that understands a workspace of Markdown notes, and
**frontends** that show it (today one, the terminal UI). Extensions reach them through two
gateways: the **workspace gateway** to the engine, and the **view gateway** to the frontend. This page is the whole picture; [extensions.md](extensions.md) has the protocol and
plans in detail.

```
 you ─ keys, mouse ─────────────────────────────────────────┐
                                                            ▼
┌───────────────────────────────────────────────────────────▼────────────────────┐
│ FRONTEND · the terminal UI today; a GUI or a language server could be others   │
│ draws views its own way: braille or pixels, a real canvas in a GUI             │
│ owns keys, mouse and frame rate                                                │
└─────────────▲─────────────────────────────────────────────┬────────────────────┘
 VIEW GATEWAY │ views: markdown · list · table              │ selection, commands
              │ · scene · pane                              │
┌─────────────┴─────────────────────────────────────────────▼────────────────────┐
│ EXTENSIONS · any language, own process                                         │
│ agenda · todo aggregator · kanban · graph · habit tracker …                    │
│ an extension may use one gateway, the other, or both                           │
└─────────────▲─────────────────────────────────────────────┬────────────────────┘
    WORKSPACE │ items, events                               │ items/query · items/edit
      GATEWAY │                                             │
┌─────────────┴─────────────────────────────────────────────▼────────────────────┐
│ ENGINE                                                                         │
│ 4  ITEMS     typed, with ids and revs: task{status, due}, book{author} …       │
│ 3  TYPES     definitions over blocks: on list-item, match "[ ] ",              │
│              field due = date (built in, yours, extensions')                   │
│ 2  BLOCKS    paragraph · heading · list item · table row · code · quote        │
│              inlines: text · link · tag · code span (the primitives)           │
│ 1  FLAVOUR   which syntax exists: commonmark · github · obsidian · yours       │
│ 0  WORKSPACE files · one watcher · index (links, backlinks) · search ·         │
│              safe writes (refused if the file changed since it was read)       │
└────────────────────────────────────────────────────────────────────────────────┘
                                       ▲ ▼
               your .md files, also edited by Obsidian, Neovim, git
```

Both gateways are JSON-RPC messages over a transport (stdio, a socket, or in-process for
built-in extensions), but they are separate protocols: the engine never sees the view gateway,
and the frontend never parses a note.

## The layers

**0. Workspace** *(built: `core.rs`, `workspace.rs`, `index.rs`, `search.rs`, `write.rs`, `rename.rs`)*.
One live workspace: one file watcher, re-reading only the notes that change, an index of links and
backlinks resolved by Obsidian's rules, full-text search, and the only write path, which writes
atomically and refuses when the file changed since it was read. Events say what changed,
whoever changed it.

**1. Flavour** *(built: `markdown.rs`)*. Markdown is not one language. A flavour is a named set
of syntax features (`task-lists`, `tables`, `wikilinks`, `callouts`, …): built-in
`commonmark`, `github` and `obsidian`, or your own in `config.toml`. Syntax a flavour lacks is
plain text, so the same file reads the same way everywhere in cold-lava.

**2. Blocks** *(built for the reader; `markdown::parse`)*. The primitives every note is made
of, as CommonMark defines them: blocks (paragraph, heading, list item, quote, code, table,
frontmatter) and inlines (text, link, emphasis, code span, tag). Values inside them have a small
set of types: text, number, date, datetime, link, tag, true/false, list. These are a note's
bytes and integers.

**3. Types** *(designed in extensions.md, "Types"; definitions load and are checked:
`types.rs`, `cold-lava types`; they match notes, table rows, list items, headings and
paragraphs: `matcher.rs`)*. Definitions of meaning over blocks: a task is a list item that
starts with `[ ]`, whose `📅 2026-11-12` is a date field `due`; a `book` is a table row whose
header has `Author`. Built-in types (task, property, heading) are definitions like anyone
else's, and you or an extension can add more in a few lines, with no code. Definitions are
declarative (patterns, fields, value types, how to show them), so matching stays fast and a bad
one can't break anything.

**4. Items** *(tasks, list items, headings, block ids and typed notes and blocks are built:
`items.rs`, `cold-lava items`; queries, subscriptions and edits are built in-process:
`gateway.rs`, extensions.md, "The workspace gateway, as built")*. What the types find: typed
values with an `id` that survives ticking, re-dating and lines added above,
and a `rev` for the item's own lines. Extensions query them, subscribe to changes, and edit one
item; the engine changes only that item's lines, through the safe write path, and refuses if
that item changed meanwhile.

**Extensions** *(the Neovim editor is the first)*. Programs in any language that do things
with notes and show them: aggregate, edit, draw a board. They register commands; they never see
keystrokes or draw pixels, so a broken one can't trap you.

**Other file types** *(owners open, save and create files: extensions.md, "File types, as
built"; items and links from owners are designed in "Owners and types")*. Separated by file
type, united by type data: Markdown is the engine's own format and its first owner; a `.canvas`,
`.kanban` or `.csv` file can have an owner, an extension that opens it, saves it safely, and
translates it to the workspace's types and links, so a task on a whiteboard card is a `task` to
the agenda and to any extension, and a card's `[[Lava]]` shows in `Lava`'s backlinks and the graph.

## Two gateways

**The workspace gateway** *(the item messages are built: `gateway.rs` in the engine, `Host` in
`ext/mod.rs`, over the stdio transport too, and so are `workspace/read`, `write`, `create` and
`files`)* is the engine's
protocol: files, items, types, queries, edits and events. It knows nothing about screens. An
extension that only aggregates and ticks tasks, a script, or a language server needs nothing
else. An extension changes notes only through it, within the permissions its manifest asks
for, and every edit names the extension that made it.

**The view gateway** is the frontend's: what an extension wants shown, and what you did to it.
It knows nothing about workspaces; it speaks in rows, cells, nodes, edges, text and selection. Views
are **declarative**: an extension says *what* to show, never *how* to draw it. So an extension in
any language can be graphical (a kanban board is a few dozen lines of Python sending JSON), every
frontend draws the same view its own way, and drawing, keys and frame rate stay with the
frontend. Neovim's UI protocol works this way, and so, at the extreme, does HTML.

*(Built for lists and tables: an extension sends `view/open` with one and hears `view/event`
when you press Enter, tick a row, use a key it bound or edit a cell; `crates/cold-lava/src/ext/view.rs`
and `views.rs`, ["The view gateway, as built"](extensions.md#the-view-gateway-as-built). The
agenda and the table are built-in extensions on it, the same path an extension in Python
takes.)*

The vocabulary stays small, and grows only when a real extension needs a new word:

| View | For | Status |
| --- | --- | --- |
| **markdown** | text with the reader's styling | built (the reader) |
| **list / tree** | the agenda, a todo aggregator, outlines | built (`crates/cold-lava/src/list.rs`; over `view/open`; the agenda, `A`, `agenda.open`, is an extension) |
| **table** | a type's items as rows: books, people, projects | built (`crates/cold-lava/src/table.rs`; over `view/open`; the menu's table, `i`, `table.open`, is a built-in extension on it) |
| **scene** | nodes and edges: the graph, the whiteboard, mind maps | designed |
| **pane** | the escape hatch: any terminal program (a GUI's could be a web view) | built |
| **image** | pictures, with Kitty pixels where the terminal has them | to design |

Types can say how their values look (a date as a chip, a status as a mark), and every view draws
that the same way. A general UI toolkit for every language is a project of its own; this is
deliberately not one. If the view gateway proves general, the clean boundary lets it become its
own project later.

## One line, all the way up and back

You write, in any editor:

```markdown
- [ ] call the plumber 📅 2026-11-12 @alice
```

0. The watcher sees `Home.md` change; the engine re-reads that note only.
1. The workspace reads as `obsidian`, so task lists exist.
2. The parser finds a list item on line 14.
3. `task` matches: status todo, `due` 2026-11-12. A `person` type from an extension matches
   `@alice`.
4. An item: `{kind: task, id: "Home:task:~3fa1…", rev, due: 2026-11-12}`.
5. Subscribers hear `items/changed`: one task added.
6. Your todo extension re-queries "open, due this week" and updates its list.
7. The reader shows a checkbox, a date chip and `@alice` as a link; the list shows the task
   under Wednesday.

And back: you press `x` on it in the list. The key runs the extension's `todo.toggle`, which
sends `items/edit {id, rev, toggle}`. The engine finds the item in the note as it is now, checks
its `rev`, writes `- [x] … ✅ 2026-11-07` the way the Tasks plugin would, and every view (and
Obsidian) sees the same file.

## Engine and frontend

The engine has no user interface. It is its own library crate, `cold-lava-engine` in
`crates/engine` (the core, safe writes, scanning, the index, search, renames, Markdown flavours,
items, tasks and daily notes), and it doesn't depend on ratatui, crossterm or the embedded
terminal, so the compiler keeps UI code out of it. The terminal UI is the `cold-lava` binary in
`crates/cold-lava`, which reads `config.toml` and hands the engine what it needs from it (the
chosen Markdown flavour). Frontends sit beside the engine:

```
 cold-lava-engine   workspace · flavours · blocks · types · items · safe writes · events
     ▲         ▲           ▲
     │         │           └─ an extension host: the protocol, transports, the registry
     │         └─ cold-lava lsp: links, backlinks, completion and renames in any editor
     ├─ cold-lava (the terminal UI, the main frontend)
     └─ cold-lava-gui (a Tauri window, crates/gui)      both draw from cold-lava-views,
                                                        and render cold-lava-editor (headless)
```

A frontend asks the engine and draws; the engine never knows which frontend is asking. That's
what lets a language server, a graphical app or a script use exactly the same understanding of
the workspace as the terminal UI. Which frontend gets the effort is the next section.

## The frontend: one, opinionated

*(vision; the reader and panes exist, the rest follows file types and scenes)*

The engine stays separate because that's good design, not because we expect many frontends.
The aim is **one frontend, opinionated enough that nobody needs to replace it**: extensions
change what it shows, never how it works.

- **The UI is a host of views.** The frontend owns layout, drawing, keys, mouse and frame rate.
  Everything it shows (the reader, the agenda, a type's table, a whiteboard) is a view; the
  built-in ones use the same view gateway as anyone's. File-type owners only say how to show
  their files; what the frontend does with that (where, how big, inline or full screen) is its
  call.
- **The document view.** The reader grows into the place you think in: a note rendered with
  its embeds alive in place. Between two paragraphs, `![[eruptions.canvas]]` is the whiteboard
  itself, drawn as a scene you can select and move around in; a ` ```mermaid ` fence (what
  Obsidian renders natively) is the diagram; `![[Projects.kanban]]` is the board, drawn by its
  owner. Each is still its own file (or fence), referenced from the note, so the note stays
  plain Markdown that Obsidian shows the same way.
- **Making things where you are.** "Insert a whiteboard here" creates `….canvas` through
  `workspace/create`, puts `![[…]]` at the cursor, and opens it in place. Same for a diagram or any
  file type whose owner declares a `new` command. You write, drop in a board, keep writing.
- **Text is still typed in your editor.** A text editor good enough to live in is a project of
  its own, so Neovim (or yours) keeps typing; cold-lava inserts references and edits items for
  you. A light block editor for quick, one-paragraph edits may come later, never a full editor.
- **A framework, extracted, not designed up front.** The frontend is built concrete and
  opinionated first. Once its parts have proven themselves (the view host, the document
  layout, embed slots, the scene renderer), they move into a crate others can use, shaped by a
  real app instead of guesses.
- **Honest limits.** A terminal draws whiteboards, diagrams, boards and graphs well, with Kitty
  pixels where the terminal has them and braille elsewhere. It won't feel like a tablet.

## What stays fixed

- **A note stays a valid Markdown file** that Obsidian, GitHub and any editor read. New syntax
  is text patterns or fenced blocks, never grammar other tools would choke on.
- **Obsidian compatibility is the floor:** its files, settings and conventions (Tasks,
  Dataview, Properties, daily notes, Canvas). The same workspace opens in both at once.
- **One writer, safe:** nothing overwrites an edit made elsewhere.
