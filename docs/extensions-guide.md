# Writing an extension

An extension is a program that cold-lava starts and talks to over its stdin and stdout. It can
read the workspace's notes as data (tasks, list items, headings, typed items), follow them as they
change, change one item at a time through cold-lava's safe edits, add commands to the menu,
show messages in the status line, show a list or a table that cold-lava draws, and open a kind
of file cold-lava doesn't read itself or show some notes its own way. It never
draws and never sees your keystrokes: cold-lava owns the screen, and tells the extension what
you did with its rows.

This guide builds on the example in
[`examples/extensions/open-tasks`](../examples/extensions/open-tasks): 140 lines of Python in
`main.py`, and `cold_lava.py`, a helper with no third-party packages that you can copy. Each
example carries its own copy of the helper, so it can be installed alone. The protocol itself is in
[extensions.md](extensions.md#transport-as-built).

## Install the example

Extensions live in one folder each under `~/.config/cold-lava/extensions/` (or
`$XDG_CONFIG_HOME/cold-lava/extensions/`). The folder's name must be the extension's name:

```bash
mkdir -p ~/.config/cold-lava/extensions
cp -r examples/extensions/open-tasks ~/.config/cold-lava/extensions/
cold-lava ext list
```

`cold-lava ext list` prints every extension, where it runs from, what it may do, and why any
didn't load. Open a workspace in the main menu. The status line shows `☐ 6 open tasks`, and the
command palette (`:`) has *move every overdue open task to today* and *list the open tasks by
note*.

## Security

An extension runs as you, with your files, like an Obsidian plugin or a Neovim plugin. Install
only extensions you trust. Its `permissions` limit what it may ask cold-lava for (read items,
edit items). They don't stop the program itself from opening files: the operating system does
not sandbox it.

For that reason cold-lava loads extensions only from your own config folder. A workspace can
carry extensions in `.cold-lava/extensions/` (a cloned repository could), and cold-lava never
runs those. `cold-lava ext list WORKSPACE` names them, so you can read one and copy it to your
config folder if you want it. Loading them after a one-time trust prompt is future work.

## The manifest

`extension.toml` says who the extension is and how to start it:

```toml
name = "open-tasks"              # lowercase letters, digits and dashes; also the folder's name
title = "Open tasks"             # optional, for people
version = "0.1.0"
description = "Counts the workspace's open tasks in the status line, moves overdue ones to today, and lists them by note"
run = ["python3", "main.py"]     # the program and its arguments, run in this folder
permissions = ["items.read", "items.edit"]

[[commands]]
id = "open-tasks.reschedule"     # must start with the extension's name and a dot
title = "move every overdue open task to today"
keys = ["R"]                     # optional default keys; config.toml can change them
```

| Field | Meaning |
| --- | --- |
| `permissions` | `items.read` (the default) for `items/query`, `items/subscribe`, `items/unsubscribe`, `types/list` and `links/query`, `links/subscribe`, `links/unsubscribe`; `items.edit` for `items/edit`; `files.read` and `files.write` to read and write files of kinds it doesn't own; `files.delete` to move a file or folder to the trash and put it back. A request without its permission is refused as `forbidden`. |
| `[[file-types]]` | Kinds of file it opens: `extension`, `title`, and `new`, one of its commands that makes one ([Owning a kind of file](#owning-a-kind-of-file)). |
| `transport` | How cold-lava talks to it. Only `stdio` exists, and it is the default. |
| `api` | The protocol version it was written for. Only `1` exists, and it is the default. |
| `[[types]]` | Types it defines: an `id` that starts with its name and a dot, like a command's (`csv-table.row`), and a definition like any type's ([Shipping a type](#shipping-a-type)). |
| `[[shows]]` | Notes it shows instead of the reader: a `title` and a `where` with `property`, `tag` or `folder`, as a type's note shape writes them ([Showing Markdown notes](#showing-markdown-notes)). |

A mistake in a manifest leaves that extension out and says why in `cold-lava ext list`. The rest
load.

## The conversation

cold-lava starts the program when you open a workspace, and stops it when you open another one
or quit. Each message is JSON-RPC 2.0, framed as LSP frames it:

```
Content-Length: 52\r\n
\r\n
{"jsonrpc":"2.0","method":"ui/notify","params":{...}}
```

`cold_lava.py` does the framing. Copy it next to your `main.py`. A run of the example goes like
this:

1. cold-lava sends `initialize` with `apiVersion`, `alphabet`, the workspace's `root` and `name`, the
   extension's `permissions`, and `capabilities.methods`, the methods it may call.
2. The extension subscribes to open tasks (`items/subscribe`), counts them (`items/query`, a
   page at a time), shows the count (`ui/status`), and answers `initialize`.
3. When a note changes, in cold-lava, in Obsidian or in git, cold-lava sends `items/changed`
   with what was added, updated and removed. The extension updates its count.
4. You run *move every overdue open task to today*. cold-lava sends `command/run`. The
   extension queries the overdue tasks and sets each one's `due` with `items/edit`. Each edit is
   checked against the `rev` it read, so a task changed elsewhere meanwhile is refused as
   `changed` and left alone. A task in a file another extension owns (a CSV's) is edited the
   same way; its answer only takes longer, while that owner writes it.
5. On quit, cold-lava sends `shutdown`, then `exit`, then closes its input.

```python
import sys

from cold_lava import Extension, Refused

ext = Extension()

@ext.on("initialize")
def initialize(params):
    ext.subscribe("task", {"status": ["todo", "in_progress"]})
    ext.status(f"☐ {len(ext.query('task', {'status': ['todo']}))} open")
    return {}

@ext.command("open-tasks.reschedule")
def reschedule(args):
    for task in ext.query("task", {"date": {"field": "due", "to": "2026-11-06"}}):
        try:
            ext.edit(task, "set-field", field="due", value="2026-11-07")
        except Refused as e:
            print(e.reason, e, file=sys.stderr)

ext.run()
```

A refusal comes back as a JSON-RPC error whose `data` is `{reason, message}`. `Refused.reason`
is one of `changed`, `refused`, `read-back`, `io`, `forbidden`, `no-workspace` or `exists`.

## Views

An extension shows something bigger than a status line by sending a **view**: a list (groups of
rows, each with text, an optional checkbox and a right side), a table (typed columns, rows of
cells), an image (`{"kind": "image", "title": …, "path": "…png"}`, or `"png"` with base64
bytes; pixels in Kitty-graphics terminals, else half blocks) or a scene (`{"kind": "scene",
"title": …, "nodes": [{"id", "label"}], "edges": [{"from", "to"}]}`: nodes and links that
cold-lava lays out and draws like the graph, with the graph's keys; Enter is `activated` with
the node's id, and the graph's other commands come as `command`), as JSON. `links/query`
gives every file and where its links lead, to make one of. cold-lava draws it over
the main area and owns everything you do in it: moving,
folding, filtering, sorting. The extension hears what you did to which row, by the row's id.
The example's *list the open tasks by note* (`open-tasks.list`) is one:

```python
view, listed = None, {}

def task_list():
    groups = {}
    for task in ext.query("task", {"status": ["todo", "in_progress"]}):
        listed[task["id"]] = task
        row = {"id": task["id"], "text": [{"text": task["text"]}], "check": "unchecked"}
        groups.setdefault(task["note"], []).append(row)
    return {
        "kind": "list",
        "title": "Open tasks",
        "groups": [{"id": n, "title": n, "rows": rows} for n, rows in groups.items()],
        "keys": [{"key": "t", "command": "today", "title": "due today"}],
    }

@ext.command("open-tasks.list")
def show_list(args):
    global view
    view = ext.open_view(task_list())          # view/open, answered with its id

@ext.on("items/changed")
def changed(params):
    if view is not None:
        ext.update_view(view, task_list())     # view/update: cold-lava keeps the selection

@ext.on("view/event")
def view_event(params):
    task = listed[params["row"]]
    if params["event"] == "activated":         # Enter
        ext.open_note(task["note"], task["line"])
    elif params["event"] == "toggled":         # x on a row with a checkbox
        ext.edit(task, "toggle")
    elif params["event"] == "command":         # one of the list's keys: t
        ext.edit(task, "set-field", field="due", value="2026-11-07")

@ext.on("view/closed")
def view_closed(params):                       # Esc or q
    global view
    view = None
```

- **Events.** `view/event` says `activated` (Enter), `toggled` (`x` on a row with a checkbox),
  `command` with the `command` of a key the list declares, or, in a table, `edit` with the
  `column` and the `value` typed. The view's own keys come first, so a key you declare that it
  uses (`x`, `j`, `/`, …) never reaches you. `view/closed` comes when Esc or `q` closed it.
- **Updates.** Send the whole list again. cold-lava keeps the selection, the folds, the sort
  and the filter by id, and draws only the last update of each frame, so sending one on every
  `items/changed` is fine. `update_view(view, content, select=id)` also selects a row.
- **Going to a note.** `open_note(note, line)` (`workspace/open`) opens a note at a line. Back
  (`Ctrl-o`) returns to your view as it was, as long as it is open.
- **Coming back to a view.** `show_view(view)` (`view/show`) brings one of your open views to
  the front from wherever the user is, as it was left (selection, folds, sort, filter), and Back
  returns to where they were. Use it instead of opening a second view of the same thing.
- **Filtering as you type.** A list or table filters after `/` by default. Add
  `"filter": "typing"` to the content for a view that is only for picking a row: typing filters
  at once and Enter opens the selected row. Every character then goes into the filter, so the
  view's letter keys (`j`, `k`, `x`, `s`, `e`, `q`, …) don't work there: the user moves with the
  arrows and closes with Esc (the first Esc clears the filter). A list that filters as you type
  can't declare `keys`; cold-lava refuses it.
- **Limits.** A view has at most 50,000 rows and groups, 64 columns and 32 keys; a bigger one is
  refused. An extension has at most 8 views open; a ninth closes its oldest.
- **No permission** is needed to show a view. Reading the tasks still needs `items.read`, and
  ticking them `items.edit`.

The JSON of each view and event is in [extensions.md](extensions.md#the-view-gateway-as-built).

## Owning a kind of file

An extension can open a kind of file cold-lava doesn't read itself: a `.csv`, a `.kanban`, an
`.excalidraw`. The example in
[`examples/extensions/csv-table`](../examples/extensions/csv-table) opens `.csv` files as
tables. Its manifest claims the kind:

```toml
name = "csv-table"
version = "0.2.0"
run = ["python3", "main.py"]
permissions = []                 # an owner needs none for its own kind

[[file-types]]
extension = "csv"                # without the dot
title = "CSV table"              # what the tree and the switcher call one
new = "csv-table.new"            # the command that makes one

[[commands]]
id = "csv-table.new"
title = "new CSV table"
```

Install it beside `open-tasks` and `.csv` files show in the note tree and the quick switcher.
Enter on one, or on a link to one (`[[Eruptions.csv]]`), sends the extension `file/open` with
the file's path, relative to the workspace. What it shows is up to it, as a list or a table:

```python
@ext.on("file/open")
def file_open(params):
    path = params["path"]
    read = ext.read(path)                       # workspace/read: {"text", "rev"}
    tables[path] = {"rev": read["rev"], "text": read["text"]}
    tables[path]["view"] = ext.open_view(content(path, parse(read["text"])), file=path)
```

`file=path` says the view shows that file: cold-lava then lists who links to it under the
view, as it does under a note, and `Enter` there opens the linking note or file.

- **Saving.** `ext.write(path, text, expect=rev)` (`workspace/write`) replaces the file only if
  it is still the version you read, and answers the new `rev`. If Obsidian, git or anything else
  changed it meanwhile, it is refused as `changed`: read it again, make your change to what is
  there now, and write again. The example does that for an edited cell.
- **Changes made elsewhere.** `file/changed` comes with the `path` and its `rev` now (`None`
  when it was deleted), from cold-lava's file watcher. Your own writes come back too, with the
  `rev` your write answered, so compare it with yours and ignore those.
- **Making one.** `ext.create(text, name="Untitled", extension="csv")` (`workspace/create`) puts
  a new file where Obsidian puts new notes, as `Untitled 1.csv` when `Untitled.csv` is taken, and
  answers its `path` and `rev`. `ext.create(text, path="Data/New.csv")` makes it exactly there,
  refused as `exists` when something is. Nothing is ever overwritten.
- **Listing.** `ext.files("csv")` (`workspace/files`) lists every file of a kind, for a picker.
- **Deleting.** `ext.delete(path, rev)` (`workspace/delete`) moves a file or a folder to the
  trash the workspace uses, never for good, and is refused as `changed` unless it is still
  the version `rev`. `ext.stat(path)` (`workspace/stat`) gives that `rev` for any file, an
  image too, and for a folder, with `inside`, what is in it, to show before you ask (up to
  1000 entries, `more` counting the rest and `hidden` the ones under hidden names, which
  are never listed). A symlink is deleted as the link it is. The
  answer's `trashed` is a token, yours alone: `ext.restore(trashed)` (`workspace/restore`)
  puts the file back where it was, refused as `exists` when something is there; with `to`
  it goes under another name, which needs `files.write` too. The token is
  gone once cold-lava restarts; the file stays in the trash. `ext.create_folder(path)`
  (`workspace/create-folder`) makes an empty folder.
- **Previews.** A note that embeds one of your files (`![[Eruptions.csv]]`) shows a small view
  of it in the reader. cold-lava asks `file/preview` with the `path`, its `rev` and the room it
  has (`width` columns, `height` lines), and draws what you answer: a list or a table, as you
  would send to `ext.open_view`, at most 200 rows and cut to `height` lines. It never waits:
  it says it is asking until you answer, keeps your answer for that `rev`, and asks again when
  the file changes. The example answers with the header and the first rows:

  ```python
  @ext.on("file/preview")
  def file_preview(params):
      table = content(params["path"], parse(ext.read(params["path"])["text"]))
      table["rows"] = table["rows"][: max(params["height"] - 1, 1)]
      return table
  ```
- **Bringing a table to the front.** When `file/open` names a file you already show, update
  its view if the file changed and bring it to the front with `ext.show_view(view)`
  (`view/show`): the table comes back as the user left it, filter and sort included. The
  example does this.
- **One owner per kind.** The first extension to claim a kind gets it; a second one is left out
  of that kind, with a line on the status line and in `cold-lava keys`. `.md` and `.canvas`
  can't be claimed. `cold-lava ext list` prints who opens what.
- **Permissions.** An owner reads, writes, creates and lists its own kind without asking.
  Reading any other file, a note included, needs `files.read`; writing or creating one, or
  making a folder, needs `files.write`. Deleting and restoring need `files.delete`, for your
  own kind too: `files.write` doesn't include it.
  Paths are relative to the workspace and can't leave it: `..`, absolute paths,
  hidden folders (`.obsidian`) and symlinks that lead outside are refused.

## Showing Markdown notes

Some Obsidian plugins keep their data in ordinary notes: the Kanban plugin writes a board as a
`.md` file with `kanban-plugin: basic` in its frontmatter, lanes as headings and cards as tasks.
An extension can show such notes its own way while they stay Markdown. The example in
[`examples/extensions/kanban-board`](../examples/extensions/kanban-board) shows boards as a
list with a group for each lane:

```toml
name = "kanban-board"
version = "0.1.0"
run = ["python3", "main.py"]
permissions = ["items.read", "items.edit"]

[[shows]]
title = "Kanban board"
where.property = { kanban-plugin = "basic" }     # as a type's note shape writes it
```

Opening a note the claim matches (Enter in the tree, the quick switcher, a link, a bookmark)
sends `file/open` with the note's path, `.md` included, instead of showing the reader. You don't
read or parse the note: cold-lava already reads it, so ask for its items and change them through
the gateway, and every edit is checked and written the way the reader's are.

```python
@ext.on("file/open")
def open_board(params):
    path = params["path"]
    lanes = ext.query("heading", {"note": path})     # the note filter: one note's items
    cards = ext.query("task", {"note": path})        # each with its section, the lane
    ext.subscribe("task", {"note": path})            # items/changed when a card changes
    view = ext.open_view(board(lanes, cards), file=path)
```

`x` on a card is `ext.edit(card, "toggle")`, and Enter is `ext.open_note(card["note"],
card["line"])`, which opens the reader at that line.

- **`where`** takes `property` (frontmatter values), `tag` (the note's tags, frontmatter's and
  body's; a nested tag counts) and `folder`, and every condition given must hold. A claim
  with none of them, or with conditions only blocks have (`header`, `section`), is left out:
  a claim can't take every note.
- **The most specific claim shows a note**: the one with the most conditions, counted as for
  types' shapes. Two extensions with the same `where` conflict; the first loaded keeps it and
  the other is reported on the status line and in `cold-lava keys`. Of two different claims
  with as many conditions, the first loaded wins.
- **The reader is always a key away.** `→` in the tree reads the note, `v` in your view opens
  it in the reader, and anything that opens the note at a place in its text (a link to a
  heading, a backlink, a search hit, `ext.open_note` with a line) opens the reader there.
- **Your view shows the note's backlinks** under it when you pass `file=path`, as for a file you
  own.

## Giving items and links

An owner can also say what its files hold to the rest of the workspace, so a row of a CSV is a
row of a type's table, a chore in a format of your own is a task in the agenda, and a link in
a cell shows in the note's backlinks and in the graph. cold-lava asks `file/read` with the
file's `path`, its `text` and its `rev`, and you answer its items and links. You never know
about types:

```python
@ext.on("file/read")
def file_read(params):
    return given(params["text"])    # {"items": [...], "links": [...]}
```

There are three ways to give an item, and one answer may mix them. Which one to use depends on
what your format holds at that spot, not on what you would like it to become.

| What the file holds there | Give | Why |
| --- | --- | --- |
| something in your own format: a row, a chore, a heading, a card | a **piece** | you say what it is, and no character of it is read as anything else |
| text that *is* Markdown: a notes column, a description field people write Markdown in | a **Markdown piece** | cold-lava reads it as it reads a note, so tasks, tags and links in it work as they do there |
| a record you have already read into a type the workspace has | a **typed item** | the shortest way when you know the type and its fields by name |

**Give a piece** for everything your format says itself. That is the usual case, and the only
one where nothing is parsed twice.

```python
from cold_lava import piece, link, label

piece("plumber", "item", ["Call the plumber ", label("home")],
      {"task": True, "checkbox": "open", "due": "2026-11-20"})
piece("Krakatoa", "row", cells=[("Volcano", [link("Krakatoa")]), ("VEI", ["6"])])
```

- **`place`** is your own name for the spot: a row's key, a chore's name, a card's id. Keep it
  stable when other things in the file change, since an item's id comes from its file, its kind
  and its place. One place holds one piece.
- **`role`** is what it is, one of nine: `document`, `section`, `text`, `item`, `row`, `aside`,
  `verbatim`, `embed`, `opaque`. A type matches a piece by its role (`on = "item"`), so a chore
  given as an `item` with `task` and `checkbox` is a `task` with no type of yours.
- **Text is runs.** A string is plain characters, whatever they are: `#42`, `$5`, `[[x]]`,
  `**x**` and `- [ ] y` make no tag, no math, no link, no bold and no task. What your format
  does say, say as a run: `link("Krakatoa")`, `label("home")`, `marked("hot", "strong")`,
  `field("rating", 5)`. You never escape anything.
- **Attributes** are what it carries beyond its text, under any names: text, a number, `True`
  or `False`. A type reads them (`attribute = "due"`). A `checkbox` says `open`, `done`,
  `in-progress` or `cancelled`, and `task` says that you count the item as a task.
- **Relations.** `inside=` is the place of the piece this one is in, given before it. `cells=`
  are a row's, by column name. A position is attributes (`x`, `y`). A link from one piece to
  another (an arrow, a cross-reference) is an entry of `links`:
  `{"from": "a", "to": "b", "label": "needs"}`, which cold-lava carries and shows nowhere yet.
  Give that last one only when `initialize` said `alphabet` 3 or more, and pieces when it said
  2 or more.
- What a piece can say, in full, is in [extensions.md](extensions.md#pieces-as-built).

**Give a Markdown piece** only where the text is Markdown already:
`{"place": "Krakatoa#Notes", "text": "- [ ] Read the report 📅 2026-11-20"}`. It is read in the
workspace's flavour, so the built-in `task` and the user's types match in it and its
`[[links]]` are links. Don't build Markdown out of something else to get an item: then you
have to escape every character that means something in Markdown and parse it all back on an
edit, and a cell holding `#42` becomes a tag. This is what a *fragment* will be (text in
another format, handed to that format's owner, #244), for the one format it works for today.

**Give a typed item** when the record maps onto a type by name:
`{"place": "4", "type": "eruption", "fields": {"vei": 6}}`. Each value is read as its field's
type (`"6"` or `6` for a number, `"Krakatoa"` or `"[[Krakatoa]]"` for a link). A type that isn't
loaded, or a field it doesn't have, is reported and the item left out. It says less than a
piece (no text of its own, no links, nothing a second type could read), so prefer a piece
when the user's types should be able to match what you give.

Then, for all three:

- **Links.** A link in a piece is a link run. One outside any item is an entry of `links`:
  `{"target": "Krakatoa", "place": "4", "text": "Krakatoa,1883,6"}`, with the text around it
  for backlinks. Links in Markdown pieces need no entry.
- **Opening at a place.** When the user opens one of the file's items (in the agenda, a table)
  or a backlink from it, `file/open` comes with its `place`, and you show the file there:
  `ext.open_view(content, select=row)`.
- **When you're asked.** Only when a file's `rev` is one you haven't answered for: what you gave
  is kept under `~/.cache/cold-lava/owners/`, by workspace, your name and your `version`, so a
  workspace opens with your files' items before you have started. Change `version` when you
  change what you give. Answer within 10 s; an error, a late answer or an answer that doesn't
  read is said on the status line, and the file keeps what you gave before.
- **An answer is checked.** A piece with an empty place, a place another piece has or a
  `checkbox` that isn't one of the four words is reported and left out, and the rest is read.
  A key or a role cold-lava doesn't know refuses the whole answer.

### Two worked examples

**A format of its own: chores** (`crates/cold-lava/tests/chores-owner/main.py`, the owner the
tests run). A `.chores` file has a chore a line, `name | state | due | words`, where `@word` is
a label and `>Word` a link:

```
plumber | todo | 2026-11-20 | Call the plumber @home
```

Every chore is one piece, and nothing in the file is Markdown, so nothing is given as
Markdown:

```python
attributes = {"task": True, "checkbox": STATES.get(state)}    # "todo" is "open"
if due:
    attributes["due"] = due
items.append(piece(name, "item", runs(words), attributes))    # runs: strings, label(), link()
```

The agenda lists it beside a note's tasks, with its date, and `@home` is the tag `home`.

**A table with a notes column: csv-table** (`examples/extensions/csv-table/main.py`). A CSV is
rows of cells, so each row is a `row` piece and a cell is plain characters. The header says
what else a column holds:

```
Volcano (link),Year,VEI,Notes (markdown)
Krakatoa,1883,6,- [ ] Read the report on [[Krakatoa]] 📅 2026-11-20
```

```python
for name, kind, value in zip(names, kinds, row):
    if kind == "markdown":                       # the cell is Markdown: hand it over as such
        cells.append((name, []))
        if value.strip():
            markdown.append({"place": f"{place}#{name}", "text": value})
    elif kind == "link" and value.strip():       # the cell names a file
        cells.append((name, [link(value)]))
    else:                                        # characters
        cells.append((name, [value] if value else []))
items.append(piece(place, "row", cells=cells))
```

- A type whose row shape names the columns (`where.header = ["Volcano", "VEI"]`, `column =
  "VEI"`) matches the row, as it matches a table in a note.
- `Krakatoa` in the link column is a link: it is in Krakatoa's backlinks, and a rename
  follows it. `[[Krakatoa]]` typed into a plain column is twelve characters. A link cell
  may go on after the name, `Krakatoa#Eruptions` or `Krakatoa|the big one`: the link run
  is to `Krakatoa`, with the words after `|` as its text, and a rename rewrites the name
  and keeps the rest of the cell.
- The notes column is the one place the example gives Markdown, because that is what people
  write there. Its task is a `task`, at the place `Krakatoa#Notes`.
- A row's place is its first cell when no other row starts the same. Rows that start alike
  are named by that cell and their own number in the file (`a (row 3)`), and so is a row
  whose first cell itself reads like such a name. So a place with a number belongs to that
  row alone, no two rows share a place, and a row is never left out because of what its
  cells say.
- A place is looked up, never taken apart. The example keeps one function from a row and a
  column to a Markdown cell's place, and one table from every place back to its row and
  column. A column's name may then hold any character.
- Choosing a place is a trade. cold-lava checks an edit by reading your new text back, and
  a place it knows must then hold the piece it expects. So an edit of one piece must never
  give another piece a place that a different piece had. Numbering alike rows `a`, `a (2)`
  breaks that: set the first row's first cell and the second row becomes `a`. The row's own
  number doesn't. What it costs: rows that start alike get new places, and their items new
  ids, when a row is added above them. A row with a first cell of its own keeps its place
  wherever it moves. Pick the scheme whose edits always go through.
- It asks what it may give. `ext.alphabet` is the alphabet's version from `initialize`, and
  the example gives rows as pieces only when it is 2 or more (`given(text, pieces=ext.alphabet
  >= 2)`). To an older cold-lava it gives the Markdown cells alone.

## Shipping a type

An owner can ship types of its own in its manifest, for what its files hold whatever the
user's types say. A `[[types]]` table is an `id` in your namespace and a definition written as
in `types.toml` ([extensions.md](extensions.md#types)):

```toml
[[types]]
id = "csv-table.row"
on = "table-row"                    # the role `row`, under the name older releases read too
where.file-type = ["csv"]           # only in .csv files, never a table in a note
priority = -1                       # below the user's types: their row types win
fields.name = { pattern = '^([^\t]+)' }    # a row's words are its cells, a tab between them
fields.links = { inlines = "links", type = "link" }
```

- It matches your pieces and your Markdown pieces by its shapes, like any type, and your typed
  items name it (`{"place": "4", "type": "csv-table.row", "fields": {…}}`).
- A field is written back where its type reads it, so read what should be editable from an
  attribute or from a cell by its column's name. `name` above is read by a `pattern` over the
  row's words, which shows it and can't set it: the user sets that cell in the CSV table, or
  through a type of their own that names the column.
- `where.file-type` keeps a shape to your files. `priority = -1` makes it a fallback: a user's
  type that matches the same row has priority 0, so it wins.
- It shows in `types/list`, in the table's type picker (`i`) and in `cold-lava types`, with
  origin `extension csv-table`. A user's `config.toml` or workspace can replace it by its id.
- An id outside your namespace, or one you give twice, is left out and said on the status line
  and in `cold-lava keys`. A definition that doesn't read is dropped, and said in
  `cold-lava types`; your other types and the extension still load.

## Shipping a shape

A shape says how something is drawn on the canvas ([shapes.md](shapes.md#shape-definitions)).
Ship one in your manifest the same way, a `[[shapes]]` table with an `id` in your namespace:

```toml
[[shapes]]
id = "csv-table.sheet"
title = "A CSV sheet"
inputs = ["rows"]
aspect = 3.0
parts = [
  { rect = { id = "header", from = [0, 0], to = [1, 0.25] } },
  { text = { at = "header.top", value = ":dot: ${rows.title}" } },
  { rect = { id = "body", from = [0, 0.25], to = [1, 1] } },
]
regions = [{ id = "rows", area = "body", shows = "rows" }]
```

- `cold-lava shapes WORKSPACE` lists it with origin `extension csv-table`; `cold-lava shapes
  WORKSPACE --render csv-table.sheet --size 50x12` draws it as text, to check it without a canvas.
- A user's `config.toml` (`[[shapes]]`) or workspace (`.cold-lava/shapes.toml`) can replace it
  by its id, and nest it in shapes of their own (`{ shape = { id = "s", use = "csv-table.sheet",
  from = [0, 0], to = [1, 0.5] } }`).
- The namespace and broken definitions work as for types: an id outside it is left out and said
  in `cold-lava keys`, a definition that doesn't read is dropped with its error in `cold-lava
  shapes`, and your other shapes still load.

### Curves and spirals

A shape has no formulas: a spiral, a wave or a gear is computed by you, once, and shipped as a
`path` of `line`s and `curve`s ([shapes.md](shapes.md#paths)). This Python prints an Archimedean
spiral of three turns as twelve cubic curves, a quarter turn each, with each curve's control
points a third of the way along its tangents:

```python
import math

def spiral(turns=3, per_turn=4, r=0.48):
    """An Archimedean spiral from the centre out to radius r, as cubic curves."""
    k = r / (2 * math.pi * turns)  # how far it moves out per radian
    step = 2 * math.pi / per_turn
    def at(t):
        return (0.5 + k * t * math.cos(t), 0.5 + k * t * math.sin(t))
    def lean(t):  # its direction at t, a third of a step long: where a control point goes
        return (k * (math.cos(t) - t * math.sin(t)) * step / 3,
                k * (math.sin(t) + t * math.cos(t)) * step / 3)
    curves = []
    for i in range(turns * per_turn):
        (x0, y0), (x1, y1) = at(i * step), at((i + 1) * step)
        (a, b), (c, d) = lean(i * step), lean((i + 1) * step)
        curves.append(((x0 + a, y0 + b), (x1 - c, y1 - d), (x1, y1)))
    return at(0), curves

def xy(p):
    return f"[{p[0]:.4f}, {p[1]:.4f}]"

start, curves = spiral()
print('[[shapes]]\nid = "spirals.spiral"\ntitle = "Spiral"\naspect = 1.0')
print(f'parts = [{{ path = {{ id = "coil", start = {xy(start)}, segments = [')
for c1, c2, to in curves:
    print(f"  {{ curve = {{ c1 = {xy(c1)}, c2 = {xy(c2)}, to = {xy(to)} }} }},")
print("] } }]")
```

Paste what it prints into your manifest:

```toml
[[shapes]]
id = "spirals.spiral"
title = "Spiral"
aspect = 1.0
parts = [{ path = { id = "coil", start = [0.5000, 0.5000], segments = [
  { curve = { c1 = [0.5133, 0.5000], c2 = [0.5209, 0.5267], to = [0.5000, 0.5400] } },
  { curve = { c1 = [0.4791, 0.5533], c2 = [0.4333, 0.5419], to = [0.4200, 0.5000] } },
  # … ten more
] } }]
```

`cold-lava shapes WORKSPACE --render spirals.spiral --size 40x20` draws it (`--rotate 45` turns
it). Twelve curves are 289 points once flattened; a path may have at most 2048, and one over
that is refused when your manifest loads, with its size. Fewer, longer curves draw as smoothly
as more, shorter ones (each is drawn as 24 lines), so use as few as keep the shape true. The
cold-lava tests draw this exact output (`crates/cold-lava/tests/curves`).

### Computing a region or handling a gesture

When the region language (`a & b where vei >= 6 sort by vei desc`) or the `on` steps can't say
what you mean, say `extension = true` and answer it yourself:

```toml
[[shapes]]
id = "csv-table.tally"
inputs = ["rows"]
parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
regions = [{ id = "total", area = "box", extension = true }]   # you compute what it shows

[[shapes.on]]
click = "total"
extension = true                                                # you decide what a click does
```

cold-lava then asks you, with the shape's inputs and their members:

- `shape/region` (`placed`, `shape`, `region`, `inputs`): answer `{"items": [{"label": "12
  rows"}]}`, what the region shows. You are asked again when an input changes.
- `shape/gesture` (`placed`, `shape`, `gesture`, `inputs`): answer `{"do": [ … ]}`, steps in
  the language of `on`, such as `{"edit": {"item": "clicked", "set-field": {"field": "status",
  "value": "done"}}}`. They run as you, within your permissions: `items.edit` to edit an item,
  `files.write` to create a note. They are checked as a shape's own `do` is: if one can't
  run (a `set-field` with no `value`, `add` or `remove`, say), none runs and the user is told
  what is wrong with your answer.

Being asked needs `items.read`, since the inputs come with the question. Check yours without a
canvas: `cold-lava shapes WORKSPACE --render csv-table.tally --bind rows='type=eruption'` asks
for the region, and `--do click:total` asks for the click and runs your steps (on a copy of the
workspace: they write). The messages and their JSON are in
[extensions.md](extensions.md#shapes-as-built).

## Writing items back

When an extension edits an item of one of your files (`items/edit`), or the user renames a note
your files link to, cold-lava makes the change to what you gave and asks `file/edit` with the
file's `path`, `rev` and `text` and a list of `changes`, each at one of your places. You answer
the file's new `text`; cold-lava writes it.

```python
@ext.on("file/edit")
def file_edit(params):
    text = params["text"]
    for change in params["changes"]:
        ...                          # write what change asks, at change["place"]
    return {"text": text}
```

A change comes in the terms you gave the thing in. For a piece it is in the model's terms,
never text in another format to parse. `change_kind(change)` in `cold_lava.py` names which one
it is:

| `change_kind` | The change | Asked when | You |
| --- | --- | --- | --- |
| `"attribute"` | `{"place": "plumber", "attribute": "checkbox", "value": "done"}` | a task is ticked, or a field a type reads from an attribute is set; `"value": null` takes the attribute off | write that attribute your format's way (`todo` becomes `done`) |
| `"runs"` | `{"place": "plumber", "text": ["Call the electrician ", {"label": "home"}]}` | a piece's words are set | write the runs as your format writes text, a label and a link your own way; `words(runs)` gives the plain characters |
| `"runs"`, with `cell` | `{"place": "Krakatoa", "cell": "VEI", "text": ["7"]}` | a field read from a row's cell is set | write that cell |
| `"link"` | `{"place": "Krakatoa", "link": "Krakatoa", "to": "Krakatau"}` | a file you link to there was renamed or merged | make every link with that target at the place lead to `to` |
| `"field"` | `{"place": "4", "field": "vei", "value": 7}` | a typed item's field is set; `null` removes it | write the field |
| `"text"` | `{"place": "Krakatoa#Notes", "text": "- [x] Read the report ✅ 2026-11-08"}` | a Markdown piece was ticked, set or relinked, the way a note is | put the string back where the piece came from |

- **Every change names a place of the file as you read it.** One request may hold several
  (a rename sends one per link), and one of them may change what a later one's place is
  named by. Find every spot first, in the text you were sent, then write.
- **Change only what was asked.** Write the one cell, the one word, the one line, and leave
  every other byte of the file as it is: its quoting, its line endings, its blank lines. The
  user's file is theirs, and other programs read it too. The example finds each cell where
  it is written and replaces those bytes; it never writes the whole file out again.
- **Refuse what your format can't hold.** Raise an error and the edit is refused with your
  message, with nothing written. A chore's words can hold a label and a link and no bold, so
  runs with a mark are refused. A CSV row has no attributes, so an attribute is refused. A
  link column's cell is one link, so words there are refused. Don't squeeze a change into a
  shape that reads back as something else: the next step would refuse it anyway.
- **Read back, then written.** cold-lava asks `file/read` for your new text before it writes it,
  and writes it only when it reads as what you gave before with the changes made: the same
  items and links, and the same pieces in the same order, each saying what was asked and
  inside the same piece. Otherwise nothing is written and whoever asked hears why. So an edit
  that also changes another piece, or lands on the wrong one of two that look alike, never
  reaches the file. cold-lava writes the text itself, refused if the file changed since `rev`,
  so don't write the file yourself; `file/changed` then comes with the new `rev`, like after
  your own saves.
- **Places can change.** A rename can change a row's key, and so its place; cold-lava compares
  what you give, not what you call it.
- **Not asked yet.** No change adds, moves or removes a piece, so ticking a recurring task in a
  piece is refused before you are asked. Answer within 10 s, else the edit is refused as not
  answered.

The two examples, again. The chores owner writes each change into one part of one line:

```python
if kind == "attribute" and change["attribute"] == "checkbox":
    lines[i] = with_part(lines[i], 1, WORDS[change["value"]])    # "done" is written `done`
elif kind == "attribute" and change["attribute"] == "due":
    lines[i] = with_part(lines[i], 2, change["value"] or "")
elif kind == "runs" and "cell" not in change:
    lines[i] = with_part(lines[i], 3, written(change["text"]))   # @label, >Link, else refuse
elif kind == "link":
    lines[i] = lines[i].replace(">" + change["link"], ">" + change["to"])
else:
    raise ValueError(f"a chore can't hold that change ({kind})")
```

csv-table writes each into one cell: a plain cell takes characters, a link cell takes one
link (the rename's `to`), a Markdown cell takes the Markdown piece's new string, and a row
that would lose its first cell is refused, since that cell names it.

## When something goes wrong

- **Logs.** What the program writes to stderr goes to
  `~/.cache/cold-lava/extensions/NAME.log`, new on each start. Never write anything but
  messages to stdout.
- **A crash** is shown once in the status line with the last line of the log, and the
  extension's commands stop working until you run *start the extensions that stopped again*
  (`extensions.restart`) from the palette.
- **A slow answer** never freezes cold-lava. An extension that doesn't answer `initialize` in
  10 seconds is stopped. A command it doesn't answer in 10 seconds is reported, and the extension
  keeps running.
- **Testing.** `cold-lava drive` runs the real program in a hidden terminal and checks the
  screen; its `write FILE TEXT` step changes a file the way another program would. `crates/cold-lava/tests/drive/open-tasks.drive` installs this example in an empty
  home, runs its command from the palette and waits for its status line;
  `open-tasks-list.drive` opens its list, ticks a task with `x`, opens a note with Enter, comes
  back with `Ctrl-o` and closes the list.
