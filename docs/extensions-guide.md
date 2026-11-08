# Writing an extension

An extension is a program that cold-lava starts and talks to over its stdin and stdout. It can
read the workspace's notes as data (tasks, list items, headings, typed items), follow them as they
change, change one item at a time through cold-lava's safe edits, add commands to the menu,
show messages in the status line, show a list or a table that cold-lava draws, and open a kind
of file cold-lava doesn't read itself. It never
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
didn't load. Open a workspace in the main menu. The status line shows `☐ 3 open tasks`, and the
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
| `[types.NAME]` | Types it defines, loaded as `<name>.NAME` ([extensions.md](extensions.md#types)). |

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
   `changed` and left alone.
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
rows, each with text, an optional checkbox and a right side) or a table (typed columns, rows of
cells), as JSON. cold-lava draws it over the main area and owns everything you do in it: moving,
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
    tables[path]["view"] = ext.open_view(content(path, tables[path]["rows"]))
```

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
- **Bringing a table to the front.** Only `view/open` shows a view, so when `file/open` names a
  file you already show, close its view and open it again.
- **One owner per kind.** The first extension to claim a kind gets it; a second one is left out
  of that kind, with a line on the status line and in `cold-lava keys`. `.md` and `.canvas`
  can't be claimed. `cold-lava ext list` prints who opens what.
- **Permissions.** An owner reads, writes, creates and lists its own kind without asking.
  Reading any other file, a note included, needs `files.read`; writing or creating one needs
  `files.write`. Paths are relative to the workspace and can't leave it: `..`, absolute paths,
  hidden folders (`.obsidian`) and symlinks that lead outside are refused.

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
- **Not yet.** Items in your files can't be edited through `items/edit`, and renaming a note
  doesn't rewrite links in your files: both are refused or left alone for now.

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
