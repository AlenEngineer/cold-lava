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
| `permissions` | `items.read` (the default) for `items/query`, `items/subscribe`, `items/unsubscribe` and `types/list`; `items.edit` for `items/edit`; `files.read` and `files.write` to read and write files of kinds it doesn't own. A request without its permission is refused as `forbidden`. |
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

1. cold-lava sends `initialize` with `apiVersion`, the workspace's `root` and `name`, the
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
cells) or an image (`{"kind": "image", "title": …, "path": "…png"}`, or `"png"` with base64
bytes; pixels in Kitty-graphics terminals, else half blocks), as JSON. cold-lava draws it over
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
version = "0.1.0"
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
    tables[path] = {"rev": read["rev"], "rows": parse(read["text"])}
    tables[path]["view"] = ext.open_view(content(path, tables[path]["rows"]), file=path)
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
  Reading any other file, a note included, needs `files.read`; writing or creating one needs
  `files.write`. Paths are relative to the workspace and can't leave it: `..`, absolute paths,
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

An owner can also say what its files mean to the rest of the workspace, so a task in a CSV is a
task in the agenda, a row is a row of a type's table, and `[[Krakatoa]]` in a cell shows in
Krakatoa's backlinks and in the graph. cold-lava asks `file/read` with the file's `path`, its
`text` and its `rev`, and you answer its items and links; you never parse Markdown or know about
types:

```python
@ext.on("file/read")
def file_read(params):
    return given(params["text"])    # {"items": [...], "links": [...]}
```

- **Markdown pieces.** `{"place": "Krakatoa#Notes", "text": "- [ ] Read the report 📅 2026-11-20"}`
  is read the way a note is, in the workspace's flavour: the built-in `task` type and the user's
  types match in it, and its `[[links]]` are links. The example hands each row over as a one-row
  Markdown table under the file's header, so a type whose table-row shape names those columns
  (`where.header`) matches it as it would a table in a note, and a cell that is a task or a list
  as Markdown of its own.
- **Typed items.** `{"place": "4", "type": "eruption", "fields": {"vei": 6}}` is an item of a type
  the workspace has loaded, for formats without Markdown (`.ics`, `.bib`): each value is read as
  its field's type (`"6"` or `6` for a number, `"Krakatoa"` or `"[[Krakatoa]]"` for a link). A
  type that isn't loaded, or a field it doesn't have, is reported and the item left out.
- **Links.** `{"target": "Krakatoa", "place": "4", "text": "Krakatoa,1883,6"}` for a link outside
  the Markdown pieces, with the text around it for backlinks. Links in pieces need no entry.
- **Places.** A place is yours: a row's key, a card's id. Keep it stable when other rows change,
  since an item's id comes from its file, its kind and its place. When the user opens one of the
  file's items (in the agenda, a table) or a backlink from it, `file/open` comes with its
  `place`, and you show the file there: `ext.open_view(content, select=row)`.
- **When you're asked.** Only when a file's `rev` is one you haven't answered for: what you gave
  is kept under `~/.cache/cold-lava/owners/`, by workspace, your name and your `version`, so a
  workspace opens with your files' items before you have started. Change `version` when you
  change what you give. Answer within 10 s; an error, a late answer or an answer that doesn't
  read is said on the status line, and the file keeps what you gave before.

## Shipping a type

An owner can ship types of its own in its manifest, for what its files hold whatever the
user's types say. A `[[types]]` table is an `id` in your namespace and a definition written as
in `types.toml` ([extensions.md](extensions.md#types)):

```toml
[[types]]
id = "csv-table.row"
on = "table-row"
where.file-type = ["csv"]           # only in .csv files, never a table in a note
priority = -1                       # below the user's types: their row types win
fields.name = { pattern = '^\|\s*(.*?)\s*\|' }
fields.links = { inlines = "links", type = "link" }
fields.tags = { inlines = "tags", type = "tag" }
```

- It matches your Markdown pieces by its shapes, like any type, and your typed items name it
  (`{"place": "4", "type": "csv-table.row", "fields": {…}}`).
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

## Writing items back

When an extension edits an item of one of your files (`items/edit`), or the user renames a note
your files link to, cold-lava makes the change to what you gave and asks `file/edit` with the
file's `path`, `rev` and `text` and a list of `changes`, each at one of your places. You answer
the file's new `text`; cold-lava writes it. Again you never parse Markdown or know about types:

```python
@ext.on("file/edit")
def file_edit(params):
    rows = parse(params["text"])
    for change in params["changes"]:
        ...                          # put change["text"] back at change["place"]
    return {"text": unparse(rows)}
```

- **A Markdown piece's new text.** `{"place": "Krakatoa#Notes", "text": "- [x] Read the report
  📅 2026-11-20 ✅ 2026-11-08"}`: cold-lava ticked or set it the way it edits a note, Tasks'
  done dates and all, or rewrote its links for a rename. Put the text back where the piece came
  from. The example parses a row's one-row table back into its cells.
- **A typed item's field.** `{"place": "4", "field": "vei", "value": 7}`, in the form items carry
  it or as text its type reads; `null` removes it.
- **A link you reported.** `{"place": "4", "link": "Krakatoa", "to": "Krakatau"}`: the target as
  you gave it, and what it becomes.
- **Read back, then written.** cold-lava asks `file/read` for your new text before it writes it,
  and writes it only when it reads as what you gave before with the changes made: the same items
  and links, wherever they are. Otherwise nothing is written and whoever asked hears why. It
  writes the text itself, refused if the file changed since `rev`, so don't write the file
  yourself; `file/changed` then comes with the new `rev`, like after your own saves.
- **Refusing.** Raise an error (the example refuses a row without its first cell) and the edit
  is refused with your message. Answer within 10 s, else it is refused as not answered.
- **Places can change.** A rename can change a row's key, and so its place; cold-lava compares
  what you give, not where.

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
