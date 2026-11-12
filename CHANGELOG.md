# Changelog

All notable changes to cold-lava are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may
change commands, keys and file formats; patch versions only fix things.

## [Unreleased]

### Added

- An extension that owns a kind of file can say what the file holds as **pieces**, with no
  Markdown anywhere (#243, step 3 of the neutral model). A piece in `file/read` is
  `{place, role, attributes, text, …}`: a role of the alphabet, the attributes it carries
  (`"task": true`, `"checkbox": "open"`, `"due": "2026-11-20"`), and its text as plain runs
  that say their own marks, links, labels and fields. cold-lava never reads a piece's text
  as Markdown, so `#42`, `$5`, `[[x]]` and `**x**` in it are characters: no tag, no math, no
  link, no mark. Types match a piece as they match a note's block, reading the attributes
  the owner gave, so the built-in `task` lists an owner's chores in the agenda and the table
  beside a note's. `in` says what a piece is inside and `cells` are a row's; a position is
  carried as attributes. The two older answers, a Markdown piece and a typed item, read as
  before with the ids they had. The alphabet's version, which extensions get in `initialize`,
  is 2: give pieces only when it is 2 or more. `cold_lava.py` has helpers to build them.
  `docs/extensions.md`, "Pieces, as built".
- An owner's pieces are edited in the model's terms (#243). Ticking a task an owner gave as
  a piece sends the owner `checkbox = done` at its place, never a line of Markdown; setting
  its due date sends the attribute `due`; rewording it sends its new text as runs, with the
  labels and links that are still among the words kept. `file/edit` has two new changes for
  this, `{place, attribute, value}` and `{place, text}` with a list of runs (`cell` for a
  row's cell), and renaming a note repoints the links a piece says (`{place, link, to}`).
  The agenda, the table and `items/edit` work on these items as on any other. Before
  anything is written the owner reads its new text back, and now every piece must say what
  was asked and stand where it stood, so an owner that quietly changes another piece,
  changes the other of two identical chores (where it keeps their places) or moves one
  is refused. New words are
  refused when some of the piece's words don't show in the field being set (a type's
  `marker`, `pattern` or `without` took them out), since they would be dropped unseen. With custom Tasks statuses a tick goes on
  to the next status that is another of the four words. Ticking a recurring task in a piece
  is refused for now, and a tick adds no done date there.
  `docs/extensions.md`, "Changes in the model's terms, as built".
- A whiteboard's cards and arrows are in the model (#243). The canvas owner gives each card
  as a piece at its place: a text card a `text`, a file card or a web page an `embed`, a
  node of a type it doesn't know an `opaque`, each with where it sits (`x`, `y`, `width`,
  `height`) and its `color`, `url` or `subpath` as attributes. Each arrow is a link between
  two pieces with its label, a new entry of `file/read`'s `links`: `{from, to, label}`.
  Both are carried and never interpreted, and an extension's owner can give the same link
  between its own pieces. Nothing a board gave before moves: what is written on a card is
  still read as Markdown, so its tasks, links and tags have the ids they had, and
  `items`, the agenda, the table, tidy, backlinks and the graph show what they showed. A
  card shows up as an item only where a type of yours tests for one of its attributes, for
  example `on = "text"` with `where.attribute = "x"`. Every other type lists what it
  listed, also one that reads an inline field or an attribute: a card's own piece has no
  words, and a piece with no words matches only a shape whose `where.attribute` it
  carries. That holds for every owner: a piece an extension gives with no words (no text,
  empty runs, only field runs, a row of empty cells) is typed only by `where.attribute`,
  where before any shape whose conditions it passed took it. An empty `item` is still a
  `list` and an empty `section` a `heading`.
  Setting a card's attribute through such a type is refused with the reason: the canvas
  owner writes a card's text and file only. A group is not given yet, nor an arrow to one. The alphabet's version is 3:
  give a link between two pieces only when it is 3 or more. `docs/extensions.md`, "The
  canvas owner, as built" and "Pieces, as built".
- A type's `on` takes the roles of the alphabet (#241, step 1 of the neutral model):
  `document`, `section`, `text`, `item`, `row`, `aside` and `verbatim`, and `inline` as
  before. The names it had (`note`, `heading`, `paragraph`, `list-item`, `table-row`, `quote`,
  `code`) still read and mean the same, so no types file changes. `where.inside` takes either,
  except `section` and `document`, which it can't test yet and refuses with the reason. `cold-lava types` prints the role beside the name (`on list-item · role item`), and
  a definition's JSON (`cold-lava types --json`, `types/list`) keeps `on` as it was and adds
  `role`. `embed` and `opaque` are named and refused in `on` until something is read as one.
  The alphabet (nine roles, text, five relations, open attributes) is written down in
  `docs/extensions.md`, "The alphabet, as built", with a version that extensions get in
  `initialize` as `alphabet`. No item changes: every id, rev, field and kind is what it was.
- Attributes, and types that read them (#242, step 2 of the neutral model). A piece carries
  attributes, a name and a value each, given by the owner of its file. The Markdown owner
  gives `checkbox` on a list item (the character between the brackets), `level` on a heading,
  and `callout` and `fold` on a callout (`> [!warning]- Hot` is `warning` and `closed`). A
  type's shape tests one with `where.attribute = "checkbox"` and reads one with the source
  `{ attribute = "checkbox" }`, so it says what a piece is and not how Markdown writes it.
  Every older source (`checkbox = true`, `marker`, `inline`, `pattern`, …) reads as before.
  `items/edit set-field` writes a field read from `checkbox` or `callout` where it is
  written; `level` and `fold` are read-only. `cold-lava types` lists the new source and
  condition like the others. Reading a note costs what it did: an attribute is read only
  when a type names it. Items over the gateway are unchanged: attributes reach the wire in
  a later step.
- The built-in `task` is defined over attributes (#242). It takes an item its owner counts
  as a task (the attribute `task`), reads its status from `checkbox`, and reads `due`,
  `scheduled`, `start`, `created`, `done`, `cancelled`, `recurrence` and `priority` by name;
  it names no emoji and no bracket. How Markdown writes each one is now a table of readings
  that belongs to the Markdown owner (`crates/engine/markdown/attributes.toml`): `checkbox`
  on every item that has one, `task` on those the Tasks plugin's global filter lets through,
  `📅` and `[due:: …]`, the priority glyphs, `🔁`. You can read it with the new
  `cold-lava attributes [WORKSPACE]` (`--json` too) and change it in `[attributes]` in
  config.toml or in `<workspace>/.cold-lava/attributes.toml`, where a later entry replaces an
  earlier one and an empty list takes one out, which leaves that attribute on no piece.
  Reading and `set-field` follow the table; ticking a task and its recurrence still write
  the built-in markers, and `cold-lava attributes` says so beside a changed entry for a
  date. A broken entry is listed by `cold-lava types` too. An attribute nobody declared is the Dataview
  field of its name (`[rating:: 5]`), by the table's `"*"` entry, which can be changed or
  removed like any other. No task changes: every id, rev, field and line is what it was,
  `agenda`, `items` and `table` print the same bytes, and ticking or setting a field writes
  the same text. `cold-lava types` lists the task's fields as `attribute due` and so on. A
  type that extends the task, or reads a checkbox or a marker itself, works as before.

### Changed

- The csv-table example gives its rows as pieces, and a cell is plain characters (#243). A
  row is a `row` with its cells by column, so `#42`, `$5`, `[[x]]`, `**x**` and `- [ ] y` in
  a cell make no tag, no math, no link, no bold and no task. What else a column holds, its
  header says: `Volcano (link)` makes each cell a link to the file it names (in backlinks
  and the graph, followed by a rename), and `Notes (markdown)` makes each cell Markdown of
  its own, where a task is a task. An edit reaches it in the model's terms and it writes
  the one cell that was asked: no other byte of the file changes, where before every edit
  wrote the whole file out again in its own quoting and line endings. If you already use
  the example, with version 0.2.0:
  - every row gets a new id, since a row is now a piece and no longer a line of a Markdown
    table. Rows whose first cells are alike are named by their number in the file
    (`a (row 3)`, where it was `a (2)`), so their ids change when a row is added above
    them; a row with a first cell of its own keeps its id. That is what lets any row's
    first cell be set, alike or not. A task in a notes column keeps its id when its row's first cell is unchanged;
  - a `[[link]]`, a `#tag` or a task written in a cell is no longer one. To keep the links
    of a column, name it `Name (link)` and write the bare names (`Krakatoa`, not
    `[[Krakatoa]]`). A heading or the words to show may follow the name, as they did inside
    the brackets (`Krakatoa#Eruptions`, `Krakatoa|the big one`): the link is to the file,
    and a rename keeps what follows. One cell is one link: a cell with two links, or a link
    among other words, has to become a `(markdown)` column to stay links. To keep Markdown in a column, name it `Name (markdown)`: before, a cell
    was Markdown only when it started with `- `;
  - a type's `where.header` and `column` read the column's name without `(link)` or
    `(markdown)`, and `inlines = "tags"` finds nothing in a row;
  - a link column's cell can't be reworded from a type's table (it is a link, and
    `items/edit` takes words): set it in the CSV's own table;
  - `csv-table.row` has no `tags`, and its `name` can't be set from the type's table: it
    is read by a pattern over the row's words, and a piece's words are not a pattern's to
    rewrite. `cold-lava types` still prints that field as `writable`, which is what it is
    in a note;
  - a blank line of the file is no row any more. Before, it was an item with every cell
    empty;
  - a row whose first cell is empty has a place of its own (`row 3`, by its number), so its
    other cells can be set. Before, an edit of such a row's cells was refused. A row is still
    not allowed to lose its first cell through an edit;
  - on a cold-lava that takes no pieces (an alphabet below 2), it gives only the Markdown
    cells.

  The example workspace's two CSV files are written the new way.
  `docs/extensions-guide.md` now says when an owner gives a piece, a Markdown piece or a
  typed item, and which changes it must answer, with csv-table and a made-up format's owner
  as the worked examples.
- A task an extension's owner gave as a typed item (`{place, type, fields}`) can be ticked
  (#243): `items/edit toggle` sets its `status` to the next status symbol. It was refused
  before. A field value of exactly one character is now read as written there, so a
  `status` of one space is an open task; before it was read as no status.
- A type on a quote or a callout (`on = "aside"`, or the older `on = "quote"`) now matches.
  Before, such a type loaded and matched nothing. Now a quote or a callout it takes is an
  item of that type: its text is its first words (a callout's title), and its lines are the
  whole quote. The items the type already had keep their ids.

- A message about a mistake in a type says the role with the older name beside it
  (`column can't be read on document (note); only on row (table-row)`, where it said `on note
  (only on table-row)`), and an unknown `on` lists the roles and the older names.

### Fixed

- An extension can no longer take cold-lava down with the steps it answers `shape/gesture`
  with (#268). Those steps skipped the checks a shape's own `do` gets when it is loaded, and
  a `set-field` with none of `value`, `add` and `remove` made the app panic when the
  whiteboard ran it. An answer's steps now pass the same rules as a loaded shape's, a
  `rebind` to a shape that isn't loaded among them. An answer that breaks one is refused
  whole before anything is written, none of its steps run, and the whiteboard says why with
  the extension's name: `tally: it answered shape/gesture with steps that can't run:
  set-field takes exactly one of value, add and remove`; `cold-lava shapes --do` fails with
  the same reason. Before, a `create` in front of a bad step made its note first. Every
  `rebind` in an answer is checked now, though still only the last one is applied. Before,
  only the last was looked at: when it named a shape that isn't loaded, it was refused after
  the steps before it had run, and when an earlier one did and the last named a loaded
  shape, the whole answer ran (a `create`, a `rebind` to `nope`, then a `rebind` to
  `sticky` made the note and the shape became a sticky). That answer is refused whole now.
  This also refuses answers that ran before by accident: an `edit` with none of
  `set-field`, `toggle` and `recipe` used to tick the item, one with a recipe and something
  else (an item, another kind of edit) used to run the recipe, `options` without a recipe
  were ignored, an `edit` with a `set-field` and a `toggle` together used to set the field
  and ignore the toggle, and a `set-field` with two of `value`, `add` and `remove` used to
  take the first. An answer that doesn't read at all now names `Reply` where its message
  named `Answered` (`its answer doesn't read: data did not match any variant of untagged
  enum Reply`). A shape's own steps load and run as before.

## [0.6.0] - 2026-11-11

One reading of a note, and one door into the engine. Everything that reads a note (the reader,
backlinks and the graph, tasks and the agenda, renames, tidy) now reads the same lossless
syntax tree, so they can no longer disagree about what is a link, a tag or a task, and
pulldown-cmark is out of the shipped engine. Files that owners read are changed one way,
whoever owns them. The whiteboard, its shape editor and its picker run inside the menu, so
extensions are answered whatever is in front. The extension host is a crate of its own that
every frontend shares, and deleting, restoring and making folders are gateway messages with a
permission of their own.

- **Markdown**: the reader draws from the tree; reference links, tags after emphasis, links in
  tables, with titles and in angle brackets, footnote definitions that hold blocks, and
  backslash escapes are read the same everywhere; a stray `$$` is text and no longer hides the
  tasks and headings after it.
- **Renames** reach every link form the tree reads, including a reference link's definition,
  and change no other byte.
- **The whiteboard**: one file watcher, extensions' effects shown, nothing an extension does
  can take away unsaved work, and cards that repeat an id stay apart.
- **Extensions**: `cold-lava-host` and `cold-lava-views` are their own crates; `workspace/stat`,
  `workspace/delete`, `workspace/restore` and `workspace/create-folder`, behind `files.delete`.
- **News**: subscribers hear of every change once and in order, checked by a harness in the
  engine.

### Added

- Extensions can delete, restore and make folders (part of #193). `workspace/delete` moves a
  file or a folder to the trash the workspace uses, never for good, and is refused if it
  changed since the extension read it; `workspace/restore` puts it back, never over a file
  that is there now; `workspace/create-folder` makes a folder; `workspace/stat` gives the
  version of any file or folder, with a folder's contents to show first. Deleting and
  restoring need the new permission `files.delete` in the extension's manifest, which
  `files.write` does not include. An extension restores only what it deleted itself. The status line says who deleted what
  (`trash-can: deleted Inbox/Old.md`). The menu's own `d`, `u` and `N` now go the same way,
  and behave as before, except that `u` pressed while another workspace is open now says
  which workspace the delete was in instead of restoring into a workspace you aren't
  looking at, and that the agenda and other views hear of a delete once instead of reading
  everything again. The Python helper has `ext.stat`, `ext.delete`, `ext.restore` and
  `ext.create_folder`.
- Whiteboards join the workspace (part of #92). A `.canvas` file is read by the engine's own
  canvas owner: a task on a sticky or a text card is a task in the agenda, the type table,
  `cold-lava items` and `items/query`, and its links and tags count; a note card is a link from
  the board, so the note's backlinks list the board, the graph draws the edge and tidy doesn't
  call the note an orphan. Ticking a board's task in the agenda changes that card's text in the
  file and nothing else; `Enter` on it, or on the board's backlink, opens the whiteboard. In the
  example workspace, `Volcano Venn.canvas`'s sticky task and `Welcome.canvas`'s checklist are in
  the agenda, and Krakatoa lists both canvases with its card.
- The shape editor (`S` on a whiteboard): draw a shape and it writes the same `[[shapes]]` TOML
  into the workspace's `.cold-lava/shapes.toml`. Vertices are clicked into place and dragged;
  a side's middle bends it into a `curve`; circles, ellipses and rectangles have their own tools.
  Guides snap to 90°, 45°, 60° and 30°, equal sides, the next corner of a regular polygon, level
  and plumb, and equal circles, and are never saved. A click inside an area names it as a region
  (`left & right`, `left - right`), a click on a vertex or a point names an anchor; forms set the
  inputs, what each region shows and behaviour, and a preview shows the shape bound to real data.
  Editing a workspace shape rewrites only its block and keeps the rest of the file as text,
  refused if the file changed since it was read. With a placed shape or a group selected, `S`
  makes a new shape of them.

### Changed

- A footnote's definition holds blocks (#212): after `[^label]:`, the lines under it that are
  blank or indented four or more are the footnote's, a second paragraph, a list, a task or a
  fenced block among them, as in Obsidian. They were indented code until now, for everything
  but the reader. So links and tags there count: a note linked only from a footnote's second
  paragraph has that backlink, shows in the graph and follows a rename, and a task there is
  in the agenda. A definition also ends the paragraph above it, so `Text ^a` and `[^1]: Note
  ^b` on consecutive lines are two blocks with their own ids, where they were one block named
  `b`. Some ids change with it, all in notes with footnote definitions. A block whose `^id`
  is on a definition's line right under other text keeps its id and spans that line only. A
  task or a list item indented under a definition that comes right after a list item was
  nested in that item and is the footnote's now, with a new id. And `[^1]: text` with `===`
  or `---` on the next line was a heading for the index, never on screen: that heading is
  gone. Notes without footnote definitions, and flavours without footnotes, read as before.
- The reader draws from the same syntax tree the index and the items read (#212, part of
  #140), so what it shows as a link, a tag or a task is what backlinks, the graph and the
  agenda count. Notes read on screen as before, with these differences. A frontmatter whose
  first line is blank, as the Kanban plugin writes it, is hidden (it showed as a rule and
  text). A `---` pair in the middle of a note no longer hides the lines between. A `#` right
  after a link, closing emphasis or code is no tag, and neither is an escaped `\#` nor one
  that starts a link's text. `[[A\|B]]` in a table cell links to `A`. Emphasis that opens in a callout's title
  ends there. One thing shows less than in Obsidian, because the index reads it so: a link
  definition written over two lines is text.
- What the index and the items read changes in four places, so that they and the reader
  read as Obsidian does (#212). A backslash escapes `[`, `]`, `` ` ``, `$`, `%`, `!` and `|`
  for links, code, math and comments too: `\[[Note]]` and `\[text](Note.md)` are text, so
  they are no backlink, don't keep a note from being an orphan in `tidy`, and a rename
  leaves them alone. A link's text holds no link: in `[a [b](Inner.md) d](Outer.md)` the
  link is `Inner`, where it was `Outer`. Math is what the reader has always shown as math, for
  `items` and the agenda too: `$…$` and `$$…$$` inside one paragraph, heading, list item or
  table cell. Tasks, headings, list items and block ids after a stray `$$`
  now count: `items`, the agenda and tables dropped every line from one that starts with
  `$$` to the next `$$` anywhere in the note, or to its end, while the reader showed them.
  Backlinks and tags always counted the links and tags there, and still do. A `$$` block with a blank line in it is two
  paragraphs of text, as on screen. A line right under a callout's title that has no `>`
  is the callout's.
- Extensions are answered while the shape editor (`S`) or the board picker (`w`) is open (part
  of #173): both run in the menu's loop now, as the whiteboard does. A status word such as the
  example's count of open tasks keeps up under the picker and shows in the shape editor's
  status line, and so do notifications. The picker is drawn over the menu instead of an empty
  screen. An extension can't take the whiteboard away from work that isn't saved: while a card
  is being typed or the shape editor holds unsaved changes, a note, view or command it asks
  for is not opened or run, and the status line says what it wanted. Before, a card being
  typed could be lost that way.
- A whiteboard's file is changed the way an extension's file is (closes #183): a rename, a
  merge and a recipe ask the canvas owner for the file's new text and read it back before
  anything is written, as a tick in the agenda already did. Renames and merges look and write
  as before. A recipe's plan now shows a canvas as `Board.canvas (through canvas)` with the
  changed lines of its cards instead of a diff of the file's JSON, in the app and in
  `cold-lava recipe` (`"owner": "canvas"` with `--json`). A recipe run by a shape's step reads
  the canvases it changed at once, with the notes. A canvas that isn't JSON can't be searched
  for links: a rename used to pass it by without a word, and now its plan names it
  ("Broken.canvas can't be read (…), so links in it aren't updated"). Cards that share an id,
  as another program may write them, are now each read and ticked on their own.
- Items are read from the note's own syntax tree instead of a second Markdown parser (part of
  #140): every task, list item, heading and block keeps its id, rev and fields, and
  `cold-lava items`, `agenda` and `table` print the same, as fast (3,000 notes on a GitHub
  runner). Two things read differently, as Obsidian shows them: a frontmatter whose first line
  is blank, as the Kanban plugin writes it, is frontmatter, so nothing in it is an item; and a
  code span that its line doesn't close goes on into the next lines of its paragraph, so the
  links and tags inside it don't count for backlinks, the graph or `rename-tag`.
- The whiteboard runs in the menu's loop instead of its own (closes #92). Extensions keep being
  answered while a drawing is open, and their status line words show in the whiteboard's.
  Opening a board from a task in the agenda or the table, or from a backlink, selects the node
  the task or the link is on; Back and Forward return to a board with the node that was
  selected. A board that isn't a canvas says so in the status line instead of ending
  cold-lava. The shape editor (`S`) and the board picker still pause extensions while open.
- The graph lays out large workspaces much faster: nodes push each other apart through a
  Barnes–Hut quadtree instead of pair by pair, and a node's braille dot is filled dot by dot
  instead of drawn as rings of circles. On a GitHub runner, 2,000 notes settle in 5.3 s
  instead of 11.7 s, 5,000 in 8.1 s instead of 36.3 s, and 10,000 in 12.8 s instead of 106 s.
- Renames rewrite a board's note cards and the links in its text cards through the canvas
  owner, only in nodes' `file` and `text`. The delete warning says "files link to them", since
  a board can be one.
- `cold-lava item set` on a whiteboard (or a file an extension owns) edits the item through its
  owner, as the agenda does, instead of reading the file as a note; `--dry-run` is refused
  there. `task toggle FILE:LINE` on such a file refuses and says to use `item set`.
- The whiteboard reads a card without `x`, `y`, `width` or `height` as at 0 instead of refusing
  the board.
- A note's links and tags are read from its Markdown syntax tree (part of #140), so they follow
  the Markdown the reader shows: a link to a heading of the same note (`[text](#heading)`) is no
  longer the tag `heading`, and links and tags in indented code, in a ``` ``code`` ``` span, in an
  HTML block or past a fence line that doesn't close the fence no longer count.
- Renames, tidy and a task's `tags` and `links` read the same syntax tree as backlinks and the
  graph (#203, part of #140), so they agree on what a link is. A rename therefore leaves
  alone what the reader shows as no link: text in indented code, in a ``` ``code`` ``` span or a
  code span that goes on to the next line, in an HTML block, in a fence inside a quote or past
  a fence line too short to close its fence, a Markdown link in the frontmatter, `](Note.md)`
  with no `[` before it, and a `[text](…)` cut in two by a table's `|`. It now finds links it
  missed: after a line that only looked like a fence (` ```js``` ` on one line, or a fence
  indented four spaces inside a paragraph). These stay links, for a rename and for backlinks:
  a Markdown link whose text goes on to the next line, an image inside a link's text
  (`[![alt](pic.png)](Note.md)`) and a wikilink on a line that looks like a definition
  (`[Source]: [[Book]]`).

### Fixed

- A Markdown link whose destination is followed by something that is no title
  (`[a](B.md and more)`) named the note `B` in a type's `links` and nothing in backlinks. It
  names no note in either now: the items ask the tree where a link's destination is, as the
  index does (#212). pulldown-cmark is no longer a dependency of the engine, only of its
  tests.
- What an extension asks for while the whiteboard asks it about a shape it ships is done now:
  its notification shows in the whiteboard's status line, its status word changes, and a note
  it opens opens (part of #173). The whiteboard used to discard all of it. `cold-lava board`,
  which has no menu behind it, shows the notifications and keeps the status words in its status
  line; a note, a view or a command the extension asks for there is refused, and the status
  line says so (#215).
- The whiteboard's notice that an extension's request was refused (`open-tasks wanted to open
  Lava.md: not while a card is being edited`) can't be missed any more (#215). While a card is
  typed the editing hint has its line, so the notice now waits and shows for its 3 seconds when
  the edit ends; it used to run out under the hint. Requests refused before the last notice was
  read are counted in it (`(2 requests refused)`), and the menu no longer repeats a whiteboard
  notice after the whiteboard is left. A command an extension asked for is also refused when a
  card was begun between the request and its run.
- A whiteboard opened from a task or a backlink on a card that repeats another card's id (a
  file another program wrote) selects that card (#215). Nothing was selected before, and with
  such a card selected by hand, an edit went to the first card of that id and a delete took
  every card of it. The file keeps the ids it had.
- A tag right after the markers that open emphasis, a strikethrough or a highlight is a tag,
  as in Obsidian: `**#tag**`, `_#tag_`, `~~#tag~~` and `==#tag==` (#181). Backlinks' tag
  searches, the graph's tag groups, `rename-tag` and the types that read a note's tags see it.
  `a#b`, `a**#b**`, a `#` in a URL, `[[#Heading]]` and a link's destination still hold none.
- Reference links are links (#181): `[text][ref]`, `[ref][]` and `[ref]` point where the note's
  `[ref]: Note.md` says, so the note they name gets the backlink and the graph's edge, and the
  types that read a note's links see them.
- A rename rewrites a reference link's definition (#203): `[ref]: Old.md "Title"` becomes
  `[ref]: New.md "Title"`, keeping its label, its title and the way its path was written. It
  used to report "0 links" and leave the link broken. `tidy` counts reference links, so two
  notes joined only by one are no longer orphans.
- A rename, backlinks, the graph and `tidy` find three more ways to write a link (#206): an
  aliased wikilink in a table, where Obsidian escapes the pipe (`[[Note\|alias]]`, which read
  as a link to `Note\`); a Markdown link with a title (`[a](Note.md "title")`, with `'title'`
  and `(title)` too); and a destination in angle brackets (`[a](<Note.md>)`,
  `[a](<My Note.md>)`). A destination with parentheses is read whole as well
  (`[a](Notes%20(old)/Note.md)`). A rename rewrites the target alone: the `\|`, the alias, the
  title with its quotes and the `<>` stay, and a new name's spaces are written `%20`, between
  `<>` too, as in every Markdown link a rename writes. Each of these used to be left
  pointing at nothing. In a whiteboard's text card too.
- A task's own `tags` and `links` (and any type's that reads them from a block) are the ones
  the note's syntax tree reads on its lines (#203): `- [ ] **#urgent** call` has the tag, a
  reference link is in `links`, and what sits in a wikilink's alias or a link's destination
  is no tag.
- The agenda and the type table no longer list a task twice when it comes in the first change
  after cold-lava created, renamed, moved, merged or deleted files (#172). Those read the
  workspace again, and the `items/reset` that sends waited for the next change and came with
  it; it is sent at once now, to extensions too.
- An extension's late answer about one of its shapes on the whiteboard no longer lands on
  another shape after the shape editor saved or the board was reopened (#172).
- The type table and the agenda keep their rows in one order: notes first, then whiteboards
  and other owned files, as `cold-lava items` prints them. After a change they sorted every
  file by name instead, so a whiteboard's tasks moved up among the notes' until the next
  full read moved them back (#199).
- A change read right after extensions were restarted is no longer told twice (#199). The
  restart can queue an `items/reset`, which reached subscribers together with that change's
  `items/changed`; a subscriber that queried again on the reset and then applied the change
  had the new item twice. While a reset waits, the core now leaves out the later
  `items/changed` it already covers, whichever call queued the reset.

## [0.5.0] - 2026-11-10

Shapes, usable end to end: a canvas engine draws any shape from its geometry, shapes are
defined in TOML like types (built in, in a workspace, or shipped by extensions), bound to live
data with a small region language, and placed on the whiteboard, where they stay safe alongside
Obsidian. The graph now runs inside the menu, as an extension.

- **Shapes**: `[[shapes]]` definitions with parts, regions, curved paths and anchors; bindings
  to queries, notes or text, kept live; regions that show set expressions (`a & b`, `where`,
  `sort by`, `limit`); behaviour that turns clicks and drops into checked edits; cards, stickies
  and "make it a note". `cold-lava shapes --render --bind` draws one from the command line.
- **Placed shapes**: on the whiteboard, saved in `.canvas` as JSON Canvas nodes Obsidian keeps;
  move, stretch, rotate, bind; the example workspace's `Volcano Venn.canvas`.
- **Scenes**: the graph runs in the menu's loop and is a built-in extension; `links/query` and
  the `scene` view kind on the gateways.

### Added

- Curved paths in shapes. A `path` can be a `start` and `segments`: `line`s, cubic `curve`s and
  `quad`s, besides `points`. A segment can name the vertex it ends at (`id = "tip"`), an anchor
  later parts use (`bubble.tip`), as are `start` and `end`. A path may have at most 2048 points
  once its curves are flattened, so a screen of shapes still draws within a frame; a bigger one,
  from a file or an extension, is refused with its size. The example workspace has a speech
  bubble, and the extensions guide shows an extension shipping a spiral it computes.
  `cold-lava shapes --render` takes `--rotate DEG`.

- Placed shapes on the whiteboard. A node with a `cold-lava` field is a placed shape: its
  shape, what its inputs are bound to, its turn and the fractions Obsidian rounds away. The
  whiteboard draws it with the canvas engine (a Venn shows which items fall in each region,
  live as notes change), and it can be moved, stretched from any corner and turned (`r`, or
  drag its `↻` grip). In Obsidian it stays a sensible node: a card is the note's `file` node, a
  sticky a `text` node with its text, other shapes a `text` node with their title. When a
  program saves the canvas without cold-lava's fields, the whiteboard keeps the shapes, saves
  them back and says so.
- Placing and using shapes on the whiteboard: `s` picks any loaded shape and a small form
  binds its inputs (a query, a note, labels or text, in `--bind`'s short form); `b` binds the
  selected one again. A click or a drop on a shape runs what its `on` declares, through the
  workspace gateway; `x` lists its actions, and a sticky's "Make it a note" makes the note and
  turns the sticky into a card on it. Enter edits a sticky's text in place, opens a card's
  note (Back returns to the whiteboard), or binds any other shape again. The example
  workspace has a `checklist` shape whose click ticks a task.
- The example workspace's `Volcano Venn.canvas`, embedded from `Guide/Whiteboard`: a Venn
  of the volcanoes that erupted and the stratovolcanoes, a card on Krakatoa, a sticky with a
  task, an arrow and the field season's checklist, all placed shapes.


- A `scene` view on the view gateway: an extension sends nodes, edges between them by id,
  colour groups and flags (`{"kind": "scene", ...}`), and cold-lava lays them out with its
  force layout and draws them as it draws the graph, in braille or Kitty pixels, with the
  graph's keys. At most 50,000 nodes.
- `links/query` on the workspace gateway: every file links come from (notes and files other
  owners read), where each link leads or the name it names when it leads nowhere yet, and
  Obsidian's graph colour groups. `links/subscribe` sends `links/changed` when it may have
  changed.
- `cold-lava drive` drags and scrolls the mouse: `drag ROW COL ROW2 COL2` presses the left
  button, moves through every cell on the way and releases, `scroll up|down ROW COL [N]` turns
  the wheel, and `expect-in ROW COL ROW2 COL2 TEXT` checks that a part of the screen shows
  TEXT. `graph.drive` and `board.drive` use them to pin the graph view and the whiteboard as
  they are: selecting, dragging nodes and cards, panning, zooming, the local graph, search,
  colour groups and filters, notes changed on disk, editing cards, and merging a canvas changed
  outside while a card is being edited.
- Shape definitions: `[[shapes]]` in TOML, built in, from extensions, in `config.toml` and in a
  workspace's `.cold-lava/shapes.toml`, loaded in the order types are, a broken one dropped with
  its error while the rest load. Parts (rect, circle, ellipse, line, arrow, path, text, other
  shapes), anchors, inputs, regions and behaviour (`on`: gestures and actions as gateway steps,
  stored and checked; they run with bindings). Built in, written in the same TOML: `text`,
  `rect`, `circle`, `line`, `arrow`, `card`, `sticky` (with a "make it a note" action), `table`,
  `venn2` and a set of glyphs (`:check:`).
- `cold-lava shapes [WORKSPACE] [--json]` lists them with their origins and errors;
  `--render ID [--size WxH]` draws one as text.
- Text at an anchor (a shape's title) never overlaps another: a later one slides along its row,
  then is cut with "…", then left out.
- The example workspace has shapes of its own: a three-circle Venn and an eruption timeline.
- Shape bindings: a shape's inputs bound to a gateway query, a note, labels or text, kept up to
  date as notes change; its regions show what their `shows` picks from the inputs (`a & b`,
  `a | b`, `a - b`, matched on the shape's `key` with accents and case folded), and
  `${a.title}` shows the input's title. `cold-lava shapes WORKSPACE --render ID --bind
  INPUT=SOURCE` draws a shape bound to data, and `--json` prints what each region shows.
- The region language: after a region's sets, `where` (comparisons on numbers, dates and folded
  text, `and`, `or`, `not`), `sort by FIELD [desc]`, `limit N` and `label FIELD`, checked when
  the shape loads; a query input takes a `where` too (`type=volcano where vei >= 6`). The
  example workspace's `tallest` shape lists the five tallest of a collection.
- Shape behaviour: a gesture (a drop between regions, a click, a key, a named action) runs the
  shape's `on` steps through the workspace gateway: `edit` an item (`set-field`, `toggle`) or
  run a recipe, `create` a note where Obsidian puts new notes, `rebind` the shape as another.
  The user's shapes edit as `canvas`; an extension's shape edits as the extension, within its
  permissions. The sticky's "make it a note" creates the note and becomes a card bound to it.
  `cold-lava shapes WORKSPACE --render ID --bind … --do GESTURE` runs one from the command line.
- Extensions can compute a region of a shape they ship, or handle a gesture on it
  (`extension = true`), answering `shape/region` with labels and `shape/gesture` with steps.

### Changed

- The graph view runs in the menu's loop instead of its own. Extensions keep being answered
  while it is open, and their part of the status line (`ui/status`) and their messages show in
  the graph's. The graph is kept while the workspace is open: going back to it, or pressing
  `g` again, finds the layout where it was instead of laying it out again. `cold-lava graph
  WORKSPACE` opens the menu on the graph; `q` there still quits.
- The graph is a built-in extension: it reads the link graph through the workspace gateway and
  sends it as a scene through the view gateway, like the agenda and the table. `g` and the
  graph's keys are the same. Enter on an unresolved link makes its note at the workspace's
  root, as before; `note.edit` and `note.open-in-obsidian` now take a `note` from an extension.

### Fixed

- `cold-lava item set` and `task toggle` find a note's workspace by a `.cold-lava` folder as well
  as `.obsidian`, and then by the workspaces you've opened or Obsidian lists, before taking the
  note's own folder. When they had to guess and something isn't found, they say so and why, since
  ids from `cold-lava items` on another folder won't match.

## [0.4.0] - 2026-11-09

The engine's other half: everything is reachable through the gateways, other kinds of file
join the workspace, and whole-workspace changes become previewable recipes.

- **Extensions in any language**: a stdio JSON-RPC transport, the workspace gateway (items,
  types, files, style) and the view gateway (lists, tables, images); the agenda and the table
  run as extensions on it.
- **File types and owners**: an extension can own a kind of file (`.csv` in the example), give
  its items and links to the workspace, and take edits and renames back; Markdown is the first
  owner; extensions can show Markdown notes their own way (a Kanban board) and preview files
  inside notes.
- **Recipes** (`rename-tag` first) and **workspace style**: changes previewed as a diff before
  anything is written, and new text written the way your notes already are.
- **Links**: one resolver, both directions, with `./` and `../` meaning the same file
  everywhere.

### Added

- Workspace style: cold-lava counts how the notes are written as it reads them (list markers,
  wikilinks or Markdown links and their paths, emoji or Dataview task fields, heading style,
  frontmatter lists, `#` in `tags`, the order of frontmatter keys, each field's date form) and
  keeps the majority, with Obsidian's own settings winning where it has one (`useMarkdownLinks`,
  `newLinkFormat`, the Tasks plugin's format). `cold-lava style WORKSPACE [--json]` shows what
  was found and the counts behind it; `[style]` in `config.toml` and `.cold-lava/style.toml` in
  the workspace override any of it.
- New text follows the workspace's style. A field `item set` or the table view adds to a task
  is a Dataview field or an emoji marker as the workspace writes them (the line's own way
  first); a done or cancelled date added by a tick is written the same way; a new link value is
  a wikilink or a Markdown link as the workspace writes them; a new frontmatter key goes where
  the notes put it; a new `tags` or `aliases` list (adding a tag with `#`, merging notes) is
  written `  - a`, `- a` or `[a]`, with `#` on tags when the workspace's have one.
- `workspace/style` on the workspace gateway: an extension (with `items.read`) asks how the
  workspace writes and gets the same JSON as `cold-lava style --json`, to write new text the
  same way. `cold_lava.py` has `style()`.
- Recipes: named changes to the whole workspace, shown as a diff per file before anything is
  written. `cold-lava recipe WORKSPACE rename-tag --from a --to b [--dry-run] [--json]` renames
  a tag and the tags nested under it, in the text (never in code) and in frontmatter `tags` in
  each form Obsidian reads, whatever their case, keeping the new name as typed; renaming into
  an existing tag merges them. Tags in files extensions own are renamed through the extension.
  The write is refused if a file changed since the plan. `cold-lava recipe --list` names the
  recipes. In the app, `recipe.run` in the palette picks a recipe, asks for its options and
  shows every file's diff before applying it; `u` undoes the whole recipe.
- An image view. An extension can send `view/open` a picture, `{"kind": "image", "title",
  "path"}` with a workspace path or `"png"` with a PNG's bytes in base64 (at most 8 MB). In a
  terminal with Kitty graphics (Kitty, Ghostty, WezTerm) the PNG's pixels show under the
  view's text; elsewhere the view shows the picture's name, kind, size and a thumbnail of half
  blocks. A picture embedded in a note (`![[lava-lake.png]]`) now shows its kind, size and, for
  a PNG, that thumbnail under its name in the reader. Only PNG is decoded; other formats show
  their name, kind and size.
- Previews of embedded files. A note that embeds a file an extension owns
  (`![[Eruptions.csv]]`) shows the owner's small view of it in the reader, under the embed's
  name: cold-lava asks `file/preview {path, rev, width, height}` and draws the list or table it
  answers (at most 200 rows, cut to 8 lines). The reader never waits: it says it is asking
  until the answer comes, keeps the answer for that version of the file, and asks again when
  the file changes, showing the old preview meanwhile. csv-table answers with the header and
  the first rows. An embed nobody owns stays its name, as before.
- Extensions that show Markdown notes. A manifest's `[[shows]]` claims notes the way a type's
  `where` matches them (frontmatter `property`, `tag`, `folder`): opening such a note (Enter in
  the tree, the quick switcher, a link, a bookmark, `workspace/open`) hands it to the extension
  as `file/open` instead of the reader, while Markdown still reads its items and links and
  writes its edits, so Obsidian opens it as before. The most specific claim wins; two equal
  claims are reported like command conflicts. `→` in the tree still reads the note, `v` in the
  extension's view opens it in the reader, and an open that points into the text (a heading
  link, a backlink, a search hit, a task's line) goes to the reader. The new example
  `examples/extensions/kanban-board` shows Obsidian Kanban boards (`kanban-plugin` in the
  frontmatter) as their lanes and cards and ticks cards with `items/edit`; the example
  workspace has one, `Inbox/Field Season.md`. `items/query` and `items/subscribe` take a
  `note` filter for one note's items.
- `cold-lava --version`, and the help's title, name the commit the binary was built from
  (`cold-lava 0.3.0 (1a2b3c4d5e6f)`), so a build says exactly which code it is.
- Types shipped by owners. An extension's manifest defines types as `[[types]]`, each an `id`
  in its namespace (`csv-table.row`, like its commands) and a definition in the language of
  any type. They are listed in `types/list`, the table's type picker and `cold-lava types`
  (origin `extension NAME`), and match an owner's Markdown pieces and typed items. An id
  outside the namespace or given twice is left out and reported like a command conflict; a
  definition that doesn't read drops that type only. A shape can say which kind of file it
  matches in, `where.file-type = ["csv"]`. csv-table ships `csv-table.row` for the CSV rows
  none of your types matches, and the example workspace has `Volcanoes/Observatories.csv`,
  whose rows are that type while `Eruptions.csv`'s stay `eruption`s. `cold-lava types` counts
  owned files' items too.
- An owned file's backlinks. While an owner's view of a file is open (csv-table showing
  `Volcanoes/Eruptions.csv`), the backlinks pane sits under it, as under the reader: every note
  and every other owned file that links to it, kept up to date. `Tab` moves between the view and
  the pane, `Enter` opens the source at the link (a note in the reader, an owned file in its
  owner at the link's place), `b` hides and shows it, and Back returns to the view. An
  extension says which file a view shows with `view/open`'s new optional `file`.
- `view/show {view}` brings an extension's open view back to the front, from wherever you are,
  and Back returns there. csv-table uses it: opening a `.csv` whose table is already open shows
  that table as you left it (selection, sort, filter) instead of closing and reopening it.
- A list or table can ask to be filtered as you type (`"filter": "typing"`), with no `/`
  first. Every character then goes into the filter, so the view's letter keys and the list's
  own `keys` are out of reach (a list that binds keys can't ask for it); the arrows, `Enter`,
  `Esc`, `Backspace` and `Ctrl-u` still work. The table's type picker (`i`) filters as you
  type again, as it did before the table became an extension.

- File types. An extension's manifest can claim a kind of file (`[[file-types]]` with
  `extension`, `title` and `new`, the command that makes one). Files of that kind show in the
  note tree and the quick switcher, and Enter on one, or following a link to it
  (`[[Eruptions.csv]]`), sends its owner `file/open`; the owner shows it in a view. The owner
  reads and writes it through the workspace gateway: `workspace/read` (`{text, rev}`),
  `workspace/write` (refused as `changed` unless the file is still the version it read),
  `workspace/create` (where Obsidian puts a new file, numbered when the name is taken, never
  overwriting) and `workspace/files`, and hears `file/changed` from cold-lava's file watcher,
  whoever changed the file. An owner may read and write its own kinds without asking; any
  other file needs the new permissions `files.read` and `files.write`. Paths stay inside the
  workspace and out of hidden folders. The first extension to claim a kind gets it; a second
  claim, or one on `.md` or `.canvas`, is left out and reported, and `cold-lava ext list` says
  who opens what. Files nobody owns behave as before.
- `examples/extensions/csv-table`, a second example extension in Python, which owns `.csv`:
  each file opens as a table, an edited cell is saved (read again and set again when the file
  changed meanwhile), changes made elsewhere show up, and *new CSV table* makes one. The example
  workspace has `Volcanoes/Eruptions.csv`, linked from *Start Here*.
- `cold-lava drive` scripts can `write FILE TEXT`, changing a file in the workspace the way
  another program would.
- Owners give items and links. cold-lava asks a file's owner `file/read` (`path`, `rev`,
  `text`), and the answer's items join `items/query` and `items/subscribe`, so the agenda and
  the type tables show them: Markdown pieces (`{place, text}`), where `task` and your types match
  as in a note, and typed items (`{place, type, fields}`) of a loaded type. Its links, and the
  links in its pieces, are backlinks (Enter opens the file in its owner at the link's place),
  edges in the graph and part of tidy; a link to an owned file resolves. What an owner gave is
  kept by file version under `~/.cache/cold-lava/owners/`, so a workspace opens with it at once,
  and an owner is asked again only when a file changes; one that fails or doesn't answer in
  10 s is reported and its files keep what they had. `workspace/open` takes an `item`, which
  opens a file another owner reads at that item, and `view/open` a `select`. csv-table gives
  each row as a one-row Markdown table and each Markdown cell as Markdown, and the example
  workspace's `Volcanoes/Eruptions.csv` now has `Volcano, Year, VEI, Notes` with links and a
  task, which the `eruption` type matches with a second table-row shape.
- Edits and renames through owners. `items/edit` (`set-field`, `toggle`) on an item of a file
  another owner reads works as on a note's: the engine makes the change (a Markdown piece's new
  text, or `{field, value}` for a typed item) and asks the owner `file/edit` (`path`, `rev`,
  `text`, `changes`) for the file's new text, has the owner read that text (`file/read`), and
  writes it only when it reads as asked, refused if the file changed since. The edit is
  attributed like any other. Its answer comes once the owner has answered: over the wire as
  the same JSON-RPC response, later; for built-in extensions through `Extension::edited`. An
  owner that refuses, writes something else or doesn't answer in 10 s gets a clear refusal,
  and nothing is written; one edit of a file waits for its owner at a time. Renaming or
  moving a note also rewrites links in owned files (`file/edit` with the pieces' new text, or
  `{place, link, to}` for a link the owner reported), and the preview lists those files.
  csv-table puts pieces back in their cells, so the agenda ticks its task, the table sets a
  row's VEI and renaming Krakatoa rewrites `Eruptions.csv`.

- Extensions in their own process. A folder in `~/.config/cold-lava/extensions/` with an
  `extension.toml` (`name`, `version`, `description`, `run`, `permissions`, `[[commands]]`,
  `[types.*]`) is started when a workspace opens, and spoken to in JSON-RPC 2.0 over its stdin
  and stdout, framed as LSP frames it. It can call the workspace gateway (`items/query`,
  `items/subscribe`, `items/edit`, `types/list`, within its permissions), `ui/notify`,
  `ui/status` (its own text in the status line) and `command/run`, and its commands join the
  palette, keys and `config.toml` like the core's. Nothing waits for it: a crash is reported
  once with the last line of its log (`~/.cache/cold-lava/extensions/NAME.log`) and its
  commands stop, and "restart extensions" (`extensions.restart`) starts it again.
  `cold-lava ext list [WORKSPACE]` prints every extension, its permissions and commands, and
  why any didn't load. Extensions inside a workspace aren't run.
- `examples/extensions/open-tasks`, an example extension in Python with no dependencies: a
  count of open tasks in the status line, kept current as notes change, and a command that moves
  overdue tasks to today. [docs/extensions-guide.md](docs/extensions-guide.md) explains how to
  write one.
- Views from extensions. An extension sends a list or a table with `view/open` (`view/update`
  replaces it, `view/close` closes it), and cold-lava draws it over the main area with its own
  keys, folding, filtering and sorting, and tells the extension what you did as `view/event`
  (Enter, `x` on a checkbox, a key the list declares, a table's edit) and `view/closed` (Esc or
  `q`). `workspace/open` opens a note at a line. An open view is a place in Back's history, kept
  as it was. Views are bounded (50,000 rows, 64 columns, 32 keys, 8 open per extension, updates
  drawn once a frame) and need no permission. The example extension's *list the open tasks by
  note* (`open-tasks.list`) shows one: `x` ticks, `t` sets the due date to today, Enter opens the
  task's note.
- An item that answers to `task` always has its `status` and `status_type` over the workspace
  gateway, even when its type reads neither, so the `status` filter finds such a subtype.

- The item messages of the workspace gateway, in-process (`crates/engine/src/gateway.rs`,
  through `Host`): `items/query` (a kind or type, a filter on folder, tag, status, a date range,
  text or a property, and pages), `items/subscribe` with `items/changed` and `items/reset`,
  `items/unsubscribe`, `items/edit` (`set-field` and `toggle`, checked by `rev` and read back,
  refused with a reason: `changed`, `refused`, `read-back`, `io`, `forbidden`) and `types/list`.
  A manifest's `permissions` say what an extension may do (`items.read` by default,
  `items.edit` to change items), and every edit names the extension that made it on the status
  line. The table view reads and edits its type through them, and ticking in the agenda and
  the reader goes through `items/edit`; neither looks or behaves differently.

- The agenda screen. `A` (`agenda.open`) lists the workspace's tasks (anything that answers to
  `task`, subtypes too) by day: Overdue, Today, each of the next seven days, Later, and Undated
  (folded) last, filed by the date `cold-lava agenda` uses (due, scheduled, start, else the
  daily note's). Rows show the checkbox, the words with links as text, `🔁`, the priority chip
  and the type's looks, the date and the note. `x` ticks a task through `tasks::toggle` (refused
  with the reason when it changed elsewhere), `Enter` opens its note at the task and `Ctrl-o`
  comes back, `h` / `l` / `Tab` fold groups, `/` filters, `d` shows done tasks. Tasks closed
  today stay in place, dimmed. The agenda follows changes on disk, keeping the selection.
- The declarative list view (`crates/cold-lava/src/list.rs`): groups of rows by id, with a
  checkbox, text pieces with looks, a right side and nesting depth; selection, folding and
  filtering in the view; `activated`, `toggled` and the sender's own key commands back. Serde
  like the table, shaped to cross the view gateway.
- `x` in the reader ticks a task the way `task toggle` does (done and cancelled dates, the next
  occurrence of a recurring task, custom statuses), refused with the reason on the status line
  when the task changed since the note was read. It ticks the task the selected link is in, else
  the one on the highlighted line, else the first task on screen below it. On the last page of
  a note, or in a note shorter than the pane, `j` / `k` move the highlighted line instead of
  doing nothing, so every task can be reached. The reader keeps its place.
- `COLD_LAVA_TODAY=YYYY-MM-DD` pins today for the agenda, `task toggle` and ticking, for
  scripted runs and tests.

- The table view. `i` (`table.open`) picks a type, listed with how many items each has, and
  shows its items across the workspace as a table: a column per field in the type's order, then the
  note, values drawn with the type's looks. `s` / `S` sort by the selected column (numbers,
  dates, enums in their declared order, text with accents ignored; empty values last), `/`
  filters rows, `Enter` opens the item's note at its line (`Ctrl-o` comes back to the table as
  it was), and `e` edits a value through `edit::set_field`: refused with the reason on the
  status line when it doesn't fit, the field is read-only or the item changed elsewhere (the
  table is read again then). The table follows changes on disk, keeping the selected row; wide
  tables scroll sideways. The table itself is a declarative view (`crates/cold-lava/src/table.rs`:
  columns with a value type and a look, rows by id, events back), shaped to cross the view
  gateway. `cold-lava table WORKSPACE TYPE [--sort FIELD] [--json]` prints the same rows.
- The reader draws typed values with their types' looks: `badge` as a chip, `icon`, a colour by
  name (one table in the frontend, so a light terminal can swap it), `dim`, `strike`, `bold`,
  `hide`, `link` (the value becomes a link `Tab` and `Enter` follow), `whole` (the whole item)
  and `by-value`. The note's typed frontmatter fields with a look go on a line above it. A look
  is skipped rather than drawn in the wrong place when its value can't be found exactly in the
  rendered text. The built-in task fades and strikes through done and cancelled tasks and puts
  its priority on a chip coloured by urgency; the example workspace's `vei` chip covers the whole
  `VEI 6`. With `NO_COLOR`, chips are reverse video and colours are left out. `by-value` on a
  `${tasks.statuses}` field takes the status types (`done`, `cancelled`, …).
- Inline types: a type with `on = "inline"` is found by its `match` anywhere in the text of
  paragraphs, list items (tasks too), headings and table rows, any number of times per block,
  and never in code spans, code blocks, math or frontmatter. Each match is an item of that type
  with its `parent` the block it sits in, its fields read from the pattern's groups, the
  section's heading or the note; `where` takes the note's folder, properties and tags, the
  section, and `inside` (the block's primitive or kind, such as `task`). `cold-lava items WORKSPACE
  TYPE` lists them and `cold-lava types` counts them. The example workspace's `vei` type finds a VEI
  written in prose (`its 1883 eruption (VEI 6, …)`).
- `cold-lava item set FILE ID FIELD VALUE` (`--remove`, `--rev REV`, `--dry-run`, `--json`)
  sets one field of a typed item through the source it is read from: a marker, a Dataview field
  (brackets kept), a flag or flags glyph, a table cell (not re-padded), a frontmatter property
  (line by line; nested YAML refused) or a pattern's group. A missing value is added the way the
  line writes its others, and `--remove` takes one out. The note is read again in memory first,
  and nothing is written unless the item reads back exactly the new value with nothing else in
  the note changed. A stale rev exits with code 3. The engine's `edit::set_field` does it, for
  the coming `items/edit set-field`.
- The task is a type: `crates/engine/types/task.toml` defines `task` in the same TOML as
  anyone's types, with the workspace's Tasks plugin statuses and global filter filled in, and every
  task is read through it; `cold-lava types` lists it. `cold-lava items`, `agenda` and `task
  toggle` read the same tasks with the same ids, and `agenda --json` and `task toggle` print the
  same as before. A type that `extends = "task"` (an `idea` with `where.tag = ["idea"]`) takes
  the tasks it matches, still answers to `task`, is in the agenda and is ticked by `task
  toggle`.
- `task toggle` follows the Tasks plugin's custom statuses: the status goes to its
  `nextStatusSymbol` (` ` → `/` → `x`, say), with the done date when the next one is done.
- `cold-lava item set` works on tasks: `status " "` unticks through the checkbox (a space is a
  status), `status_type done` writes the workspace's done symbol, and dates, priority and recurrence
  go through their markers or Dataview fields. A field read from the same text (a status's type)
  may change with the one set.
- Types can read a task's checkbox (`checkbox = true`, a space included), leave strings out of
  the rest of the text (`rest = { without = ["#task"] }`), read the date of the daily note they
  are in (`note = "date"`), take an enum's values from `${tasks.statuses}`, and name fields a
  block must have (`required = ["text"]`; without one, the next type that matches takes it).
  `${tasks.global-filter}`, `${tasks.statuses}`, `${tasks.format}` and `${daily.format}` in a
  pattern now stand for the workspace's settings. A marker reads past an emoji's variation selector
  (`⏳️ 2026-11-05`), and a `flags` source leaves every one of its glyphs out of the rest.

### Changed

- Ticking a task whose line writes its fields as emoji adds an emoji done date even when the
  Tasks plugin is set to Dataview, and the other way round: the line's own way wins, as it does
  for `item set`. A line with no fields gets the workspace's way, the Tasks plugin's where it is
  set.
- An extension's manifest writes its types as `[[types]]` tables with an `id`, like its
  `[[commands]]`; `[types.NAME]` in a manifest now leaves the extension out and says how to
  write it.
- A shape's `note = "path"`, `"name"` and `"folder"`, and `where.folder`, read the file a
  piece of an owned file comes from (`Volcanoes/Eruptions.csv`), not the file with the
  owner's place appended.
- A rename or move whose links reach a file another owner reads asks that owner for the file's
  new text before anything moves. Once every owner has answered and its text reads back as
  asked, the notes move and every file is written; an owner that refuses, writes something else
  or doesn't answer in 10 s cancels the whole rename with its reason, and nothing is changed
  (before, the notes had moved and only the owned file kept its old links). While owners
  answer, the rename dialog says what it is waiting for, the menu keeps drawing, and Esc stops
  waiting.
- Merging a note into another also rewrites links in files other owners read, through their
  owners, the way a rename does: the preview lists those files, the owners are asked before
  anything is written, and a refusal cancels the merge with nothing changed. Merging Vesuvius
  into Mauna Loa rewrites `Eruptions.csv`'s row.
- The command line sees files extensions own. `cold-lava items`, `agenda` and `table` include
  them from what their owners gave before (the cache in `~/.cache/cold-lava/owners/`), and name
  on stderr a file the cache has nothing for as it is now. `cold-lava rename` starts the owners
  it needs, so renaming Krakatoa also rewrites `Eruptions.csv` through csv-table; an owner that
  refuses or can't start ends the rename with its reason, and nothing is changed.
- Items are faster and much smaller on large workspaces. A typed item (every task) keeps only
  the fields it has and shares its type's field names, and reading one copies and parses less.
  On a 3,000-note workspace with 33,000 tasks, the items kept for every note take 79 MB instead
  of 153 MB; `cold-lava items` and `cold-lava agenda` no longer keep every note's items while
  they go through them, so `items WORKSPACE task` takes 1.4 s and 30 MB instead of 1.9 s and
  202 MB, and `agenda --json` 1.6 s and 76 MB instead of 2.0 s and 248 MB. Output is unchanged.
  `scripts/bench-items.py` measures these on a generated workspace.
- The agenda (`A`) is a built-in extension: it reads its tasks through `items/query` and
  `items/subscribe`, ticks through `items/edit` and shows its list through `view/open`, the path
  an extension in Python takes. It looks and works as before. `cold-lava ext list` names it.
- The table (`i`) is a built-in extension too: it reads a type's items through `items/query`
  and `items/subscribe`, edits cells through `items/edit set-field` and shows the type picker
  and the table through `view/open`. The picker is now a table of the types and their item
  counts over the main area: `/` filters it (typing no longer filters it straight away) and `s`
  sorts it. The way back to a table now reads `⟵ table` rather than `⟵ volcano table`.
  `cold-lava ext list` names it.
- cold-lava calls a folder of notes a **workspace** now, not a vault: in the menu, the status
  line, `--help` (`cold-lava items WORKSPACE KIND`) and the docs. "Vault" is left for Obsidian's
  own. The commands `vault.add`, `vault.rescan` and `vault.tidy` are now `workspace.add`,
  `workspace.rescan` and `workspace.tidy`; keys set under the old names in `config.toml` still
  apply, with a warning to rename them (not an error: `cold-lava keys` still exits 0). `cold-lava agenda --json` names the folder `"workspace"`
  instead of `"vault"`, `cold-lava types --json` gives a type from `.cold-lava/types.toml` the
  origin `{"kind": "workspace"}` instead of `"vault"`, and the extension message
  `vault/saved` is now `workspace/saved`. The list of opened workspaces moves from `vaults.json` to
  `workspaces.json` in the config folder; the old file is read once and left in place. The
  example is now `example-workspace`.
- `item set` (and editing in a table) adds a date a task doesn't have the way its line writes its
  other fields, else in the workspace's Tasks plugin format: `📅 2026-12-01` in an emoji workspace, where
  it used to add `[due:: 2026-12-01]` everywhere.
- Tasks in `cold-lava items --json` list their tags without `#` (`"tags": ["home"]`), as every
  type's tags are; the other keys, their order and the ids stay.
- Tasks are read as the Tasks plugin and the workspace read them where the old reading differed:
  nothing in a code span is a date or a tag; of a field written twice (`📅 a 📅 b`) the first
  counts and both leave the words; an unbracketed `due:: x` in the middle of a task's line is
  words (Dataview reads it only at a line's start); `#2026` is no tag and `(#paren)` is.
- Type matching: an enum's written form that isn't in its table doesn't parse (`null`, counted
  by `cold-lava types`) instead of reading `other`; an unbracketed Dataview field (`key:: value`)
  is read only at the start of a line; no source reads in code spans or math; every source of a
  field, and every occurrence of a marker or Dataview field, is left out of the rest of the text;
  a type without a `priority` has its base's.
- The built-in task has `priority = 1`, so a type on list items takes a task only when it
  extends `task` or sets a higher priority (before, tasks were never typed).
- Removing a marker with an emoji variation selector (`⏳️ 2026-11-05`) through `item set`
  takes the selector too.
- A typed item whose shape has a `key` with a text field is identified by that text as written,
  not as a JSON string, as tasks are; such items get a new id once.

### Fixed

- Tidy no longer lists a link to a file that exists but isn't a note or a known attachment
  (`[[Eruptions.csv]]`) under unresolved links.
- A custom status whose next status is cancelled (`nextStatusSymbol`) now gets the cancelled date
  (`❌ 2026-11-07`, or `[cancelled:: …]` in the Dataview format) and loses its done date, as the
  Tasks plugin does; `setCancelledDate: false` leaves it out.
- The agenda follows the day: left open past midnight, it files tasks by the new day (Today,
  Tomorrow, Overdue) instead of keeping yesterday's groups until a task changed.
- A rename or merge that leaves a relative link pointing into its own note's folder writes it
  with `./` (`[x](./Top.md)`, `[[./Top]]`) when a bare name would lead to another file, such as
  a shallower `Top.md`. It used to write `[x](Top.md)`, which then opened the other note.
- A link that starts from its note's folder (`[[../Top]]`, `[t](../Top.md)`, `[[./Top]]`)
  leads to the note its path names in backlinks, the graph, the reader and when you follow it,
  as it already did for renames, merges and tidy. Before, all of these but rename found the
  shallowest note with that name, so the backlinks pane could show a link going to a different
  note than the one a rename updated.

## [0.3.0] - 2026-11-07

### Added

- Tidying a vault. `T` (`vault.tidy`) lists orphan notes (no links in or out; `a` adds notes
  only without backlinks, daily notes are grouped last), attachments no note, embed or
  whiteboard links to, empty folders (not the ones the vault's settings name), and unresolved
  links by target with where each is. `space` marks rows, `d` deletes the marked or selected
  ones to the trash after one prompt that lists each, `u` undoes the whole batch, `Enter` opens
  the note, link, attachment or folder, and `n` creates the missing note for an unresolved link.
  `cold-lava tidy VAULT_DIR [--json] [--no-backlinks]` prints the same, read-only.

- Duplicating and merging notes. `D` (`note.duplicate`) copies the selected note or whiteboard
  to `Name 1` beside it (Obsidian's naming) and opens it. `M` (`note.merge`) merges the
  selected note into another picked in the switcher, like Obsidian's Note composer: its text is
  appended after a blank line, its frontmatter merged line by line (missing keys copied, lists
  such as `tags` and `aliases` joined, conflicting values kept as the target has them and listed),
  its name added to the target's aliases, every link to it rewritten to the target in its own
  style (headings, embeds, Markdown and relative links, frontmatter, whiteboard cards,
  bookmarks), and it goes to the trash. A preview shows all of it first; nothing is written if a
  file changed since. `u` undoes the whole merge.

- Bookmarks, shared with Obsidian: they live in Obsidian's own `.obsidian/bookmarks.json`, so
  a bookmark made in either app shows in both, and a change Obsidian makes shows at once. `B`
  (`note.bookmark`) bookmarks the selected note or folder, or in the reader the heading you are
  in; `Ctrl-b` (`search.bookmark`) in the search bookmarks what is typed; again removes it. `'`
  (`bookmarks.open`) lists them with Obsidian's groups (`←`/`→` fold), type to filter, `Enter`
  opens one (a note at its heading or block, a folder in the tree, a search, the graph, a board,
  a link) and Back returns, `Del` removes one. Bookmarked notes and folders get a `★` in the tree.
  Renaming or moving a note or folder moves its bookmarks with it; a deleted note's bookmark is
  kept, like Obsidian keeps it, and shown as missing. Items and fields cold-lava doesn't know are
  written back as they were, in their order.

- Several notes at once: `space` (`note.mark`) marks notes and folders in the tree, shown with a
  `✓`, and `Esc` clears the marks. With marks, `m` moves them all into one folder as one plan
  (one preview, links between the moved notes kept right, refused if two would land on the same
  path or a folder would move into a marked one), `d` deletes them all to the trash after one
  prompt and `u` brings the whole batch back. `#` (`note.tag`) adds a tag to the marked notes'
  frontmatter, or the selected note's, completing from the vault's tags; `-tag` removes it. The
  `tags` list keeps its style (`[a, b]` or `- a` lines), only that line changes, and a note
  whose `tags` isn't a plain list is left alone and named.

- Folders in the main menu: `R` renames the selected folder and `m` moves it into another, with
  every link into it, out of it and inside it kept correct (by path, by name, relative Markdown
  links, attachments, frontmatter, whiteboard cards), shown in a preview first. A `[[Name]]`
  link becomes a path only when the move would make it lead to another note. `d` deletes a
  folder to the trash after saying how many notes it holds and which links into it will break,
  and `u` puts it back. `N` (`note.new-folder`) makes a new folder in the selected one. Neovim
  follows a note whose folder moved and drops one whose folder was deleted, and Back/Forward
  follow moved notes. `cold-lava rename VAULT FOLDER NEW` renames or moves a folder too.

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
- Delete a note: `d` (`note.delete`) in the menu, or in the reader, asks first and lists the
  notes that link to it, with the line, since their links will become unresolved (they aren't
  rewritten, as in Obsidian). `y` deletes, `n` cancels. Whiteboards can be deleted the same way;
  folders are refused for now. Nothing is deleted for good: the note goes where the vault's
  Obsidian setting (`trashOption` in `.obsidian/app.json`) says, the system trash by default
  (the freedesktop trash on Linux, `~/.Trash` on macOS) or the vault's `.trash` folder for
  "local". A vault set to delete permanently gets `.trash` too, and the status line says so. If
  the note changed on disk after the prompt opened, it asks again with the new version. A note
  open in Neovim is saved first and its buffer dropped.
- Undo a delete: `u` (`note.undo-delete`) puts the last deleted note back where it was (up to
  20, this session). If something new is at that path, it offers a free name (`Lava 1.md`).
- `cold-lava drive --home DIR` runs the program with DIR as its home and keeps it afterwards,
  so a test can look at what the program left there (the system trash, for one).
- New notes from templates: if the vault has templates (the folder in Obsidian's core Templates
  settings, `.obsidian/templates.json`, or Templater's templates folder), `n` lists them under the
  name prompt, with "No template" first. `Tab` / `Shift-Tab` (or `↓`/`↑`) picks one. The note is
  filled the way Obsidian fills it: `{{title}}`, `{{date}}`, `{{time}}` (in the settings' formats,
  `YYYY-MM-DD` and `HH:mm` by default), `{{date:FORMAT}}` and `{{time:FORMAT}}` (moment.js).
  Other `{{…}}` text and Templater's `<% … %>` are kept as written.
- The example vault has a templates folder set up, with a new **Volcano** template.

### Changed

- `cold-lava rename --json`: the plan lists what moves as `moves: [{from, to, folder}]` instead
  of `from`, `to` and `folder` at the top, since a plan can now hold several moves.
- `Esc` (`filter.clear`) clears the tree's marks first, then the filter.
- Notes in the templates folders aren't read as items: a template's tasks and typed blocks no
  longer show up in the agenda or `cold-lava items`.
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

- Markdown links with percent-encoded names, the way Obsidian writes non-ASCII names
  (`[x](P%C4%81hoehoe.md)`), now lead to their note or attachment: in backlinks, the reader,
  the peek and following links. Before, only `%20` was decoded. Renaming, moving and merging now
  update these links too, and keep how they were written: a link that encoded non-ASCII
  characters gets the new name encoded, one that only encoded spaces gets only spaces encoded.
- Opening a note at a heading, block or line (a link, a search result, a bookmark) in a note
  short enough not to scroll no longer loses where it landed: the reader clamped the jump to a
  page height it didn't know yet, so it fell back to the top and the breadcrumb lost the heading.
- The menu no longer crashes when the last row of the note tree disappears (deleted here or in
  Obsidian): the selection was read through rows of the old tree.
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

[Unreleased]: https://github.com/AlenEngineer/cold-lava/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/AlenEngineer/cold-lava/releases/tag/v0.1.0
