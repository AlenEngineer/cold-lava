# The engine

The ideas behind all of this are in [foundations.md](foundations.md). This page describes the
engine as it is and as it is planned.

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

**0. Workspace** *(built: `core.rs`, `workspace.rs`, `index.rs`, `resolver.rs`, `search.rs`, `write.rs`, `rename.rs`)*.
One live workspace: one file watcher, re-reading only the notes that change, one link resolver
(Obsidian's rules by default; extensions.md, "Link resolver") that says where a link leads and
how to write one, an index of links and backlinks built from its answers, full-text search, and
the only write path, which writes atomically and refuses when the file changed since it was read. Events say what changed,
whoever changed it.

**1. Flavour** *(built: `markdown.rs`)*. Markdown is not one language. A flavour is a named set
of syntax features (`task-lists`, `tables`, `wikilinks`, `callouts`, …): built-in
`commonmark`, `github` and `obsidian`, or your own in `config.toml`. Syntax a flavour lacks is
plain text, so the same file reads the same way everywhere in cold-lava.

**2. Blocks** *(built for the reader; `markdown::parse`)*. The primitives every note is made
of, as CommonMark defines them: blocks (paragraph, heading, list item, quote, code, table,
frontmatter) and inlines (text, link, emphasis, code span, tag). Values inside them have a small
set of types: text, number, date, datetime, link, tag, true/false, list. These are a note's
bytes and integers. Named without reference to Markdown they are the alphabet: nine roles
of a piece, the text inside it, five relations and open attributes (extensions.md, "The
alphabet, as built"; named and versioned, a type's `on` is written in the roles, and an
extension's owner can give its file as pieces, as the canvas owner gives a board's cards
and arrows: "Pieces, as built").

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
and a `rev` for the item's own lines. That id is an *address*: it says where the item is now,
and it changes when the item is reworded or its note moves. An *anchor*, a `^id` the engine
writes into the note the first time something keeps a reference to the item, makes it the same
item for good *(decided, to build: #190)*. Extensions query them, subscribe to changes, and edit one
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
`cold-lava-host`, over the stdio transport too, and so are `workspace/read`, `write`, `create` and
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
when you press Enter, tick a row, use a key it bound or edit a cell; `crates/views/src/view.rs`
and the terminal's `views.rs`, ["The view gateway, as built"](extensions.md#the-view-gateway-as-built). The
agenda and the table are built-in extensions on it, the same path an extension in Python
takes.)*

The vocabulary stays small, and grows only when a real extension needs a new word:

| View | For | Status |
| --- | --- | --- |
| **markdown** | text with the reader's styling | built (the reader) |
| **list / tree** | the agenda, a todo aggregator, outlines | built (`crates/views/src/list.rs`, drawn by `crates/cold-lava/src/list.rs`; over `view/open`; the agenda, `A`, `agenda.open`, is an extension) |
| **table** | a type's items as rows: books, people, projects | built (`crates/views/src/table.rs`, drawn by `crates/cold-lava/src/table.rs`; over `view/open`; the menu's table, `i`, `table.open`, is a built-in extension on it) |
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
chosen Markdown flavour). Frontends sit above the engine, and reach extensions through one
host:

```
 cold-lava (the terminal UI, the main frontend)      cold-lava-gui      cold-lava lsp
     │                                                (crates/gui)       (planned, #195)
     │   each frontend: its built-in extensions, applying Effects, keys, drawing
     ▼
 cold-lava-host    manifests · permissions · Host (the gateway's guard) · Extension
     │             · Effects as data · the registry · processes, transports, JSON-RPC
     ▼
 cold-lava-views   the view vocabulary as data: list · table · image · scene, and the
     │             view gateway's messages
     ▼
 cold-lava-engine  workspace · flavours · blocks · types · items · safe writes · events
```

The arrows are the only direction a crate may depend in: frontends, then the host, then the
views, then the engine. The engine never depends on the host, and neither the host nor the
views depends on ratatui, crossterm, portable-pty or vt100 (`crates/host/tests/headless.rs`
walks `Cargo.lock` to check it). Frontends also render `cold-lava-editor` and
`cold-lava-canvas`, both headless.

**The extension host** *(built: `crates/host`, #179)* is what runs extensions and guards the
workspace gateway, once, for every frontend. An extension asks for notes as data through
`Host`, which checks its manifest's permissions (`items.read`, `items.edit`, `files.read`,
`files.write`, `files.delete`) and puts its name on every edit. A security check written once can't drift
between frontends. What a frontend adds is small: it builds a `Registry` from its built-in
extensions and the installed ones, tells them the workspace, and once a frame hands them
what the core has for them and applies the `Effect`s they return, or ignores those it has
no screen for. `crates/host/examples/headless.rs` is that loop with no terminal, and
`crates/host/tests/permissions.rs` checks the permissions on the `open-tasks` example the
same way. The terminal UI keeps Neovim in a pane, its command table and keys, and drawing.
The GUI depends only on the engine today; putting it on the host is the next step.

A frontend asks the engine and draws; the engine never knows which frontend is asking. That's
what lets a language server, a graphical app or a script use exactly the same understanding of
the workspace as the terminal UI. Which frontend gets the effort is the next section.

## The frontend: one, opinionated

*(vision; the reader and panes exist, the rest follows file types and scenes)*

The engine stays separate because that's good design, not because we expect many frontends.
The aim is **one frontend, opinionated enough that nobody needs to replace it**: extensions
change what it shows, never how it works.

- **One smart canvas** *(design: [shapes.md](shapes.md))*. Everything the frontend shows is a
  drawing made of shapes (glyphs included), defined and tagged like types; a shape can be the
  drawing of a type's data, and users and extensions make their own (a Venn diagram whose regions
  show what two queries share). A headless canvas engine computes geometry, regions, fitting and
  bindings; the TUI only paints.
- **The UI is a host of views.** The frontend owns layout, drawing, keys, mouse and frame rate.
  What extensions show (the agenda, a type's table, the graph) is a view, and the built-in
  ones use the same view gateway as anyone's. The surfaces you work in (the page with the
  reader and the editor, the whiteboard) are built into the frontend *(decided 2026-11-11)*:
  typing, selection, drawing and undo need every frame, and the whiteboard showed that such a
  surface doesn't fit a protocol of lists, tables and scenes (#170). They still get all their
  data through the workspace gateway. File-type owners only say how to show
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
- **The notebook.** *(design; this replaces "text is still typed in your editor")* The
  experience is a notebook and a pencil: you write, start a drawing right there, and make a
  mental map of the concept you're writing about, on one page, without switching tools. So
  cold-lava has its own writing surface:
  - **The page is the note,** rendered as you type, the raw syntax showing only on the line you
    are on; plain Markdown on disk, so Obsidian and git see a normal file. Plain writing keys by
    default, a Vim mode as an option.
  - **The pencil moment.** Anywhere in the text, "draw here" creates a drawing's file, puts
    `![[…]]` at the cursor and opens it in place; Esc is back to writing below it. The drawing
    lives in its own file (docs/shapes.md), so the note stays clean text.
  - **The mental map.** "Map this note" makes a drawing bound to the note's structure (its
    headings, the concepts it links, its tags), not a copy: as you write, the map follows, and
    an arrow drawn between two concepts on the map can add the link in the text.
  - **In a terminal there is no pencil:** freehand with the mouse works in braille but is coarse,
    so the quick path is placing shapes, typing labels and connecting them by keyboard. Pencil
    and stylus belong to the GUI, on the same canvas engine and the same files.
  - **Other editors are extensions** over the gateway (Neovim today, in a pane), for people who
    want to keep theirs.
- **A lossless Markdown tree for the editor** *(built: `cst.rs`; #140 step 1)*. A note is a
  concrete syntax tree of Obsidian-flavoured Markdown on `rowan`, rust-analyzer's tree library:
  an immutable "green" tree holding every byte, whitespace and blank lines included, with
  shared subtrees, and a "red" layer made on demand for parents and offsets. Printing the tree
  gives the note back byte for byte (a test does this for every note of the example workspace,
  the docs and the fixtures, with CRLF, a BOM, tabs and no final newline among the edge cases).
  - *Blocks* are found line by line as CommonMark does: a line first continues the open
    containers it can (block quotes and callouts, lists and their items), then closes the rest,
    then opens new blocks (frontmatter, ATX and setext headings, thematic breaks, fenced and
    indented code, HTML blocks as opaque text, tables, `%%` comment blocks,
    paragraphs with lazy lines). A container's marker on a later line (`>`, an item's
    indentation) is a token of the innermost block that holds that line. A lazy line, which the
    paragraph's containers don't go on to, ends the paragraph at any block that starts there,
    a list item of another kind (`1. a` then `2) b`) included. A table whose header starts
    with `|` ends the paragraph above it, as on GitHub; another table's header is a
    paragraph's only line. A link definition (`[label]: destination "title"` on one line,
    where a paragraph would start) is a block of its own, and the line after it starts
    afresh, as pulldown-cmark reads it. With footnotes, `[^label]:` is none: it starts a
    footnote's definition (`FootnoteDefinition`), a container as a list item is. It starts
    where a block can start, ends the paragraph above it, and holds what follows it on its
    line and the lines under it that are blank or indented four or more: a second paragraph,
    a list, a task, a fenced block. A line that is neither ends it, unless it goes on the
    footnote's paragraph lazily; a footnote that starts right inside another ends that one.
    This is how pulldown-cmark read footnotes, and so how the items read them before #178:
    the links, tags and tasks in a footnote's indented lines count (they were indented code
    in the tree until #212), and a `^id` on a definition's line names the footnote's text
    without its `[^label]:`. An edit inside a definition, or on the line right after one,
    reparses the definition: it is one top-level block, as a list is. Math is no block: a
    line that starts with `$$` is a paragraph's line like any other, starts nothing and
    hides nothing after it (math is read in a block's text, below). A line
    right under a callout's title that doesn't go on the quote is the callout's first
    paragraph, lazily, as the title is a paragraph's line to CommonMark. With wikilinks, a
    destination that starts one is none either: `[Source]: [[Book]]` is a line of text with a
    wikilink, which backlinks and a rename find.
  - *Inlines* are read per line, left to right, the first to start winning: code spans,
    wikilinks and embeds (target, `#heading` or `^block`, `|alias`), Markdown links and images,
    Dataview fields (`[key:: value]`, `(key:: value)`, a `key:: value` line), `%%comments%%`,
    `$math$`, tags and block ids. A code span its line doesn't close goes on into the next
    lines of its paragraph when one of them closes it, as CommonMark has it. Math (`$…$`,
    `$$…$$`) is what pulldown-cmark read as math, which is what the reader always showed: it
    is in one block's text, on one line or over several lines of a paragraph (keeping their
    indentation, not their containers' markers), and never goes on past its block. A `$`
    opens with something that isn't blank after it; the block's next `$` in the same braces
    (`{}`) closes it, with something that isn't blank before it on its line, or nothing
    does. `$$` is closed by `$$`. A backslash before a `$` takes it out. So a price pairs
    with no later `$`, and two `$$` in two paragraphs are text. Math that opens in a link's
    text and closes after the link is the math, and the link none. A link's text may
    go on to a later line of its block too: a `[` that nothing closed is still open there, and
    the `](destination)` that closes it is a `LinkEnd`, whose destination counts as a link's.
    A link's text may hold an image (`[![alt](pic.png)](Note.md)`), though no other link. A
    Markdown link ends at the `)` after its destination and its title, as in CommonMark: the
    destination is bare, with its parentheses balanced, or in `<>`, where it may hold spaces;
    the title is in `"…"`, `'…'` or `(…)`. The `LinkDest` token holds both, and
    `cst::link_destination` says where the destination is in it, which is all a rename
    rewrites. Where that isn't what is written, the link ends at its first `)`, as it always
    did, and counts as a link only if nothing but a title follows its destination. In a
    table cell the pipe before a wikilink's alias may be escaped, as Obsidian writes it there:
    the backslash belongs to the `Pipe` token, not to the target. Outside a table the
    backslash stays in the target. A
    task's checkbox is a token of its list item. The Tasks plugin's emoji fields stay text,
    which the `task` type reads (its markers are data, not syntax). Every one of these is
    gated by the flavour's `Syntax`.
  - *What text holds* (#212) is read after that, in what is left as `Text`, a block at a
    time: escapes (`\*`), what is written as a character reference (`&amp;`), autolinks
    (`<https://…>`) and inline HTML as CommonMark defines them, a footnote's reference
    (`[^label]`; its definition is a block, above), a hard break (two
    blanks or a `\` before the line break, where the block has another line), and emphasis,
    strong, strikethroughs and highlights. `*` and `_` follow CommonMark's delimiter runs
    (left- and right-flanking, `_` not inside a word, the rule of three), with the nearest
    opener taken and what opened in between left as text; `~` and `~~` pair with a run of
    their own length; `==` pairs left to right among what is directly inside the same span,
    as the reader always paired it. A span opens and closes in the same inline node, so it
    holds a link whole or sits inside its text, and it may close on a later line of its block,
    with the block's `>` and indentation on the lines between as tokens inside it. A
    wikilink's alias and the label a link by reference shows (`[[Note|*shown*]]`,
    `[**label**]`) are read the same way, being text on screen. This reading only cuts those
    tokens and wraps them in nodes: the links, tags, fields and
    blocks found before it are the same with it and without it, which a test checks on every
    note of the corpus. So the index and the items read without it (`Gates::plain`,
    `Gates::INDEX`) and pay nothing for it, and `Tree::parse` reads with it: their sink is
    the one they had, and the sink that holds a block back is compiled once, behind a trait
    object. The new kinds come last in `Kind`, so the others keep their numbers. On a GitHub
    runner the scan's functions take the same instructions as before (callgrind: the block
    parser, `Line::run`, `Links::step`), and a keystroke in a 5,000-line note takes what it
    took; a full parse of that note, which now reads what text holds, takes about a tenth
    longer. Strikethroughs,
    highlights and footnotes are the flavour's `Syntax`; the rest is CommonMark's. Within a
    line: an autolink, a tag or a footnote that the line reader's tokens cut through isn't
    one. The line reader knows the escapes that matter to it: a backslash before `[`, `]`,
    `` ` ``, `$`, `%`, `!`, `|`, `*`, `_`, `#`, `=`, `~` or another backslash starts nothing
    and ends nothing, so `\[[Note]]` is text for the reader and no link for the index. A
    link's text holds no link (wikilink, Markdown link or link by reference, outside code;
    an image may be in it): the one inside is the link, the brackets around it are text,
    and a link ends the brackets open before it.
  - *Reference links:* `[text][label]`, `[label][]` and `[label]` (and the same after `!`)
    are links only where the note defines the label, anywhere in it, as in CommonMark; else
    the brackets are text. Labels match whatever their case and the amount of whitespace, and
    a label's first definition wins. The tree holds the label (`LinkLabel`), not the
    destination: `Tree::definitions` says where it points. A reader going forwards gets the
    definitions from `cst::blocks`, apart from the blocks, which `read` takes and drops when
    the walk ends: a reader that kept the blocks while it built its refs made the allocator
    do more work for the same calls (callgrind). A definition that takes more than one line
    isn't read, and neither is a reference link that goes on past its line.
  - *Tags after markers:* a tag follows the start of the line, a space, `(` or `,`, and the
    markers that open emphasis (`*`, `_`), a strikethrough (`~~`) or a highlight (`==`) may
    stand between that and its `#`, so `**#tag**`, `_#tag_` and `==#tag==` hold a tag, as in
    Obsidian. After `_`, the `_` that closes the emphasis isn't part of the name. The
    strikethrough and the highlight are the flavour's. `a#b`, `a**#b**`, a `#` in a URL,
    `[[#Heading]]` and a link's destination hold none.
  - *Incremental reparsing:* `Tree::edit` parses again from the top-level block before the edit
    (a block's end depends on the first line after it that isn't blank) until the parse is
    back on a line where nothing is open and an old block starts, then splices the new blocks
    into the old green tree, sharing the rest. An edit on line 1 parses the note whole, since
    line 1 decides the frontmatter. A link definition anywhere changes how `[label]` reads in
    every block, so when the blocks parsed again don't define exactly what the blocks they
    replace defined (labels and destinations, in order), the note is parsed whole; otherwise
    the definitions are the old tree's, and the new blocks are read with them. A test makes
    thousands of random edits, definitions and reference links among them, and checks each
    against a full parse. A long list is one top-level block, so an edit inside it reparses
    the whole list. What text holds is read within a block (a paragraph, a heading, a table
    cell), so emphasis widens nothing an edit reparses.
  - *One parse, two sinks:* the parse writes to rowan's builder for a tree, or through
    `cst::read`, which hands a reader the same steps a walk through the tree takes, without
    making it. The workspace's scan reads every note once and drops it, so it uses `read`. On a
    GitHub runner, 3,300 notes took 76 ms to make into trees and 54 ms to read so, where the
    line reader before it took 58 ms. A test checks both give the same steps on every note of
    the corpus.
  - *What reads it:* the Markdown owner, for every link and tag. `owner::markdown::links`
    gives each file a note names and where it is written (a wikilink's or an embed's target, a
    Markdown link's or an image's destination, a link by reference, a definition's
    destination), and `tags_in` each tag. `read` (backlinks, the graph), tidy's
    `link_targets`, a rename's `relink` and an item's own `tags` and `links` all take them
    from there, so they agree on what a link is; a test compares them on every note of the
    corpus. The index reads in the same syntax whatever the flavour, as it always has
    (`Gates::INDEX`: wikilinks, embeds and tags count; links in comments and math do too);
    items read in the workspace's flavour. Until #203 a rename, tidy and the matcher read
    lines with patterns of their own. The tree follows the Markdown the reader shows: links in
    indented code, in a ```` ``code`` ```` span or one that goes on past its line, inside an
    HTML block, in a fence inside a quote or a fence that a shorter fence line didn't close
    don't count, and neither does a Markdown link in the frontmatter (Obsidian's properties
    hold wikilinks); a tag needs to be text, not inside a link's destination or a wikilink; a
    link by reference to a note is a link, on the line it is written on, and a tag may follow
    emphasis, strikethrough and highlight markers.
  - *The reader's `Doc`* (#212) is built from the tree too: `markdown::parse` walks it
    (`cst::read`, in the workspace's flavour with what text holds, over the note without its
    `%%comments%%`, as the items read it) and makes the same `Doc` the reader always drew. So
    what the reader shows as a link, a tag or a task is what the index and the items count.
    What isn't a node is put together on the way: a paragraph that holds only `$$` math
    is shown as a block of math, a `LinkEnd` takes the inlines since its `[` as the link's text, a `^id` names
    its block only at the block's end, a checkbox that is no `[ ]` or `[x]` is text, and a
    character reference is looked up (`markdown/entities.rs`). pulldown-cmark's parse stays
    in `markdown/pulldown.rs`, for tests only: one compares the two `Doc`s on every note of
    the corpus, the example workspace, some ninety tricky notes and a 5,000-line one, in
    each flavour. They are equal but for two lists kept there with the reason for each row.
    `DIFFERENT`: the tree reads what the index counts (a `#` right after a link, or one
    that starts a link's text, is no tag), or pulldown-cmark
    wasn't Obsidian (a frontmatter whose first line is blank, YAML in the middle of a note, a
    byte order mark, `[[A\|B]]` in a table, an escaped `\#`). `LESS`: the reader shows less
    than Obsidian does, because the tree reads it so and the index with it (a link
    definition over two lines is text). Four more differences were the tree's to mend, and
    are mended in it, for the items too and, but for math, the index: a backslash escapes
    what the line reader finds (`\[[Note]]` is text), a link's text holds no link (the inner
    one is the link), math is read as pulldown-cmark read it (above), and a lazy line under
    a callout's title is the callout's.
  - *Renames:* `relink` rewrites the bytes of a target and nothing else: a link by reference
    is rewritten once, in its definition (`[ref]: Old.md "Title"` keeps its label, its title
    and its `<>`), whether a link uses it or not. Two tests hold "nothing else": every note of
    the corpus is relinked, and every note of the example workspace renamed in a plan, then
    the old and the new text are read again and must give the same tree, token for token,
    but for the targets rewritten.
  - *Items:* `items::extract` walks the tree with `cst::read`, in the workspace's flavour, over
    the note without its `%%comments%%` (the code a `%%` is in comes from the tree too), where
    it read pulldown-cmark's events before. Ids, revs and spans hang on the blocks, so the walk
    reads them as pulldown-cmark had them: a paragraph, a table row or a fence starts at its
    first character; a callout's title is the first line of a paragraph that the next line
    goes on (a GitHub alert's `[!NOTE]` is none); link
    definitions are no paragraph (the tree has them as blocks); a table row's cells are
    split at every `|` not escaped and cut or filled to the table's columns. A test keeps the
    pulldown-cmark pass and checks both give the same items on every note of the corpus, with
    types that read every kind of block, in three flavours, and on tricky notes (lazy lines,
    loose lists, indented code, HTML, block ids, callouts, tables, math, comments, reference
    links, tags after emphasis markers). They differ
    in one way, where pulldown-cmark wasn't Obsidian: a frontmatter whose first line is blank,
    as the Kanban plugin writes it, was a rule and Markdown to it, and is frontmatter now.
    Other cases where pulldown-cmark wasn't Obsidian are kept in the test (`DIFFERENT`): a
    `---` pair in mid-note, a byte order mark, `[[#Heading]]` as a tag, and a tag after a
    highlight's `==`, which it didn't read. On a GitHub runner, 3,300 notes' items took
    160 ms with no type loaded, the walk and the ids alone, where pulldown-cmark's pass took
    153 ms; with the bench workspace's types, 495 ms and 493 ms, the type matching being most
    of it. A Markdown link names a note where `cst::link_destination` reads a destination in
    what the link points to, as for the index: `[a](B.md and more)` names none (#212; the
    items found the destination with code of their own before, which took `B.md`).
  - *One parser:* nothing the engine ships uses pulldown-cmark (#212). It is a
    dev-dependency, the oracle of the two tests above: the reader's `Doc` and the items as it
    read them. Snapshots of both were the other way to keep those tests. They would be large
    (the repository's Markdown is 468 KiB, read in three flavours), and a snapshot can't say
    how pulldown-cmark reads a note added to the corpus tomorrow.
- **A framework, extracted, not designed up front.** The frontend is built concrete and
  opinionated first. Once its parts have proven themselves (the view host, the document
  layout, embed slots, the scene renderer), they move into a crate others can use, shaped by a
  real app instead of guesses.
- **Our own terminal graphics library, by extraction.** A TUI graphics library is two halves:
  scene in, cells out (geometry, regions, rasterizing to cells, braille and pixels, fitting,
  hit testing), and cells to the terminal (a cell buffer diffed frame to frame, escape codes,
  keys and mouse). The first half is ours, the canvas engine (docs/shapes.md), and it is what no
  widget library offers. The second half stays ratatui's buffer and crossterm, since it is solved
  and fiddly. As shapes take over drawing (the graph, placed shapes, the whiteboard, the document
  view, then tables and lists), ratatui's widgets drop out until only its buffer and backend
  remain. **Decision point: around placed shapes (#136).** Replace that thin layer with our own
  writer when ratatui's buffer blocks something we need: layers (text over braille over Kitty
  pixels, which the graph fakes with `z=-1`), redrawing only damaged regions for smooth large
  scenes, sub-cell placement, or synchronized output. The canvas engine plus that writer is then
  cold-lava's terminal graphics library, and can be published as a crate. Until then the canvas
  engine stays free of UI crates, so the switch stays small. At #136 the buffer was still
  enough: placed shapes paint the canvas engine's grid into it cell by cell, over the
  whiteboard's dots and arrows, so the writer waits for a real need.
- **Honest limits.** A terminal draws whiteboards, diagrams, boards and graphs well, with Kitty
  pixels where the terminal has them and braille elsewhere. It won't feel like a tablet.

## Direction and order

*(decided 2026-11-11; the work, in this order, is the first section of roadmap issue #100)*

cold-lava has a code editor's architecture, for human notes: owners are to file formats what
language servers are to languages, types are the type system, and extensions are the plugins.
The center is **shared types**: the contract between the engine, extensions and the person
writing.

What was decided:

- **A shared vocabulary of types** (#191, #186). Three name spaces: `core` (what the engine
  needs), `std` (a small set shipped as data files: an address, a person, a place) and one per
  extension. A manifest says which it uses, so two extensions that have never heard of each
  other agree on `std.address`. Value types are definitions too, not only text, number and
  date.
- **Identity** (#190). An item gets an anchor the first time a shape, a link or an extension
  keeps a reference to it; items nobody refers to are never written to.
- **One door** (#179, #180, #193, #194). Every program is a client of one versioned protocol:
  the terminal app, a GUI, `cold-lava lsp`, the command line and extensions. The extension host
  and its permission check are shared by all frontends *(built: `cold-lava-host`, #179)*, settings belong to each open workspace,
  and what the menu does to files goes through gateway messages *(built for delete, restore
  and new folders, the first group of #193; rename, merge, recipes, undo and bookmarks
  follow)*.
- **Built-in surfaces, extension data.** Extensions bring data, file formats, types and simple
  views. The page and the whiteboard are built in ("The frontend", above). The earlier goal
  that the menu and the canvas become extensions is dropped.
- **Drawings mean something only through bindings.** What a drawing means is an ordinary type
  edit through an explicit binding; position and colour stay in the drawing's file, and nothing
  is inferred from geometry. A bound shape also saves a readable snapshot of what it last
  showed into its canvas node, so Obsidian shows a card with content (#192; docs/shapes.md,
  "Who decides what").
- **The language server comes before the editor** (#195). It is the smallest frontend, it puts
  types, links and renames into the editor you already use, and cold-lava's own editor later
  builds on the same pieces.

The order, engine-facing work first and the UX last. Since 2026-11-12, a refactor goes before
the feature that lands on its code (`docs/foundations.md`, "How the code is built"); roadmap
issue #100 is the one list of open work and says which step each feature waits for.

1. Finish the half-done migrations, so there is one path for each thing: one Markdown reading
   (the tree), one path for owned files, the whiteboard's screens in the menu's loop.
2. One door into the engine.
3. `cold-lava lsp`, used daily on real notes.
4. Types and identity.
5. The terminal views (lists, tables, the graph).
6. Drawings.
7. The notebook: cold-lava's own editor and the page. Whether the page is built for the
   terminal or the GUI first is open (#196), to decide after real use.

## What stays fixed

The full list is [foundations.md](foundations.md). The three that everything else leans on:

- **A note stays a valid Markdown file** that Obsidian, GitHub and any editor read. New syntax
  is text patterns or fenced blocks, never grammar other tools would choke on.
- **File safety is the floor:** nothing overwrites an edit made elsewhere, files are written
  atomically and deleted only to the trash, and what the engine doesn't understand it keeps.
  The same workspace can be open in another program at once.
- **Another product's reading is a profile, never the core.** cold-lava has its own flavour of
  Markdown. Obsidian's files, settings and conventions (Tasks, Dataview, Properties, daily
  notes, Canvas) are a profile a workspace can choose.
