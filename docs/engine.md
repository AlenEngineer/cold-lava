# The engine

Status: **overview**. Parts are built, parts are designed in [extensions.md](extensions.md), and
parts are still to be designed; each layer says which.

cold-lava is two things: an **engine** that understands a vault of Markdown notes, and
**frontends** that show it (today one, the terminal UI). Extensions sit on the engine, not on
the UI. This page is the whole picture; [extensions.md](extensions.md) has the protocol and
plans in detail.

```
 you ─ keys, mouse ──────────────────────────────────────────────────────┐
                                                                         ▼
 ┌─ FRONTEND: VIEWS (drawn by the frontend) ─────────────────────────────────┐
 │ reader · lists and tables · scenes (graph, whiteboard) · panes (Neovim)   │
 │ type styling: a date as a chip, a status as a mark                        │
 └────────────▲─────────────────────────────────────┬────────────────────────┘
              │ what to show                        │ commands (keys → ids)
 ┌─ EXTENSIONS (any language, own process) ─────────▼────────────────────────┐
 │ agenda · todo aggregator · kanban · graph · habit tracker …               │
 │ they query, subscribe, edit and describe views; they never draw or        │
 │ see keys                                                                  │
 └────────────▲─────────────────────────────────────┬────────────────────────┘
              │ items, events                       │ items/query · items/edit
 ┌─ PROTOCOL ── JSON-RPC over a transport (stdio, socket, in-process) ───────┐
 └────────────▲─────────────────────────────────────┬────────────────────────┘
 ┌─ ENGINE ───┴─────────────────────────────────────▼────────────────────────┐
 │ 4  ITEMS     typed, with ids and revs: task{status, due}, book{author} …  │
 │ 3  TYPES     definitions over blocks: on list-item, match "[ ] ",         │
 │              field due = date (built in, yours, extensions')              │
 │ 2  BLOCKS    paragraph · heading · list item · table row · code · quote   │
 │              inlines: text · link · tag · code span (the primitives)      │
 │ 1  FLAVOUR   which syntax exists: commonmark · github · obsidian · yours  │
 │ 0  VAULT     files · one watcher · index (links, backlinks) · search ·    │
 │              safe writes (refused if the file changed since it was read)  │
 └───────────────────────────────────────────────────────────────────────────┘
                                   ▲ ▼
                   your .md files, also edited by Obsidian, Neovim, git
```

## The layers

**0. Vault** *(built: `core.rs`, `vault.rs`, `index.rs`, `search.rs`, `write.rs`, `rename.rs`)*.
One live vault: one file watcher, re-reading only the notes that change, an index of links and
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

**3. Types** *(to design)*. Definitions of meaning over blocks: a task is a list item that
starts with `[ ]`, whose `📅 2026-11-12` is a date field `due`; a `book` is a table row whose
header has `Author`. Built-in types (task, property, heading) are definitions like anyone
else's, and you or an extension can add more in a few lines, with no code. Definitions are
declarative (patterns, fields, value types, how to show them), so matching stays fast and a bad
one can't break anything.

**4. Items** *(designed in extensions.md, "Notes as data"; tasks are being built)*. What the
types find: typed values with an `id` that survives ticking, re-dating and lines added above,
and a `rev` for the item's own lines. Extensions query them, subscribe to changes, and edit one
item; the engine changes only that item's lines, through the safe write path, and refuses if
that item changed meanwhile.

**Protocol** *(in-process half built: `ext/mod.rs`)*. JSON-RPC 2.0, modelled on the language
server protocol, over a transport chosen per extension. Built-in extensions use the same
messages in-process.

**Extensions** *(the Neovim editor is the first)*. Programs in any language that do things
with the engine: aggregate, show, edit. They register commands and describe views; they never
draw or see keystrokes, so a broken one can't trap you.

**Views** *(the reader and panes are built; lists, tables and scenes are designed)*. Drawn by
the frontend: the reader, declarative lists and tables, scenes of nodes and edges (the graph and
the whiteboard), and terminal programs in a pane. Types can say how their values look.

## One line, all the way up and back

You write, in any editor:

```markdown
- [ ] call the plumber 📅 2026-11-12 @alice
```

0. The watcher sees `Home.md` change; the engine re-reads that note only.
1. The vault reads as `obsidian`, so task lists exist.
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

The engine has no user interface. `core.rs`, `write.rs`, `vault.rs`, `index.rs`, `search.rs`,
`markdown.rs`, `daily.rs` and `rename.rs` don't use ratatui or crossterm; two helpers still sit
in UI files (`board::find_vault_root`, used by `tasks.rs`, and `home::config_dir`, used by
`config.rs`) and should move. Everything is one crate today.

The aim is the engine as its own library crate, with frontends beside it:

```
 cold-lava-engine   vault · flavours · blocks · types · items · safe writes · events
     ▲         ▲           ▲
     │         │           └─ an extension host: the protocol, transports, the registry
     │         └─ cold-lava lsp: links, backlinks, completion and renames in any editor
     └─ cold-lava (the terminal UI)        … a graphical frontend could sit here too
```

A frontend asks the engine and draws; the engine never knows which frontend is asking. That's
what lets a language server, a graphical app or a script use exactly the same understanding of
the vault as the terminal UI. Splitting the crate waits until items exist, so its interface is
cut once, around the real thing.

## What stays fixed

- **A note stays a valid Markdown file** that Obsidian, GitHub and any editor read. New syntax
  is text patterns or fenced blocks, never grammar other tools would choke on.
- **Obsidian compatibility is the floor:** its files, settings and conventions (Tasks,
  Dataview, Properties, daily notes, Canvas). The same vault opens in both at once.
- **One writer, safe:** nothing overwrites an edit made elsewhere.
