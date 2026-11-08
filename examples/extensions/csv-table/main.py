"""CSV tables: opens the workspace's .csv files as tables cold-lava draws, saves an edited cell,
follows changes made elsewhere, and makes new tables. It also tells cold-lava what each file
says: every row as a one-row Markdown table, so the workspace's table-row types match it and
its [[links]] are links, and a cell that is Markdown of its own (a task) as Markdown. An example
cold-lava extension that owns a kind of file (docs/extensions-guide.md, "Owning a kind of file"
and "Giving items and links")."""

import csv
import io

from cold_lava import Extension, Refused

ext = Extension()
# Each table on screen, by path: its view, and the version and rows it shows.
tables = {}
paths = {}
NEW = "name,notes\n,\n"


def parse(text):
    return list(csv.reader(io.StringIO(text)))


def unparse(rows):
    out = io.StringIO()
    csv.writer(out, lineterminator="\n").writerows(rows)
    return out.getvalue()


def columns(rows):
    """The header row's names, each once (a table's edits name their column), and a name for
    each column past it."""
    header = rows[0] if rows else []
    width = max([len(r) for r in rows] or [0])
    names = []
    for i in range(width):
        name = header[i] if i < len(header) and header[i] else f"column {i + 1}"
        while name in names:
            name += "'"
        names.append(name)
    return names


def places(rows):
    """A place for each row under the header that stays when rows are added above it: its first
    cell, numbered when another row has the same."""
    seen = {}
    out = []
    for row in rows[1:]:
        key = row[0] if row else ""
        seen[key] = seen.get(key, 0) + 1
        out.append(key if seen[key] == 1 else f"{key} ({seen[key]})")
    return out


def is_markdown(value):
    """A cell that is Markdown of its own: a task or a list."""
    return value.startswith("- ")


def table_cell(value):
    return value.replace("|", "\\|").replace("\n", " ")


def given(text):
    """What a file says, for `file/read`: each row as a one-row Markdown table under the header,
    and each Markdown cell as Markdown of its own, at `<row's place>#<column>`."""
    rows = parse(text)
    names = columns(rows)
    header = "| " + " | ".join(table_cell(n) for n in names) + " |"
    rule = "|" + " --- |" * len(names)
    items = []
    for place, row in zip(places(rows), rows[1:]):
        row = row + [""] * (len(names) - len(row))
        cells = ["" if is_markdown(v) else table_cell(v) for v in row]
        line = "| " + " | ".join(cells) + " |"
        items.append({"place": place, "text": "\n".join([header, rule, line])})
        for name, value in zip(names, row):
            if is_markdown(value):
                items.append({"place": f"{place}#{name}", "text": value})
    return {"items": items}


@ext.on("file/read")
def file_read(params):
    return given(params["text"])


def content(path, rows):
    names = columns(rows)
    return {
        "kind": "table",
        "title": path,
        "columns": [{"name": n, "type": "text"} for n in names],
        "rows": [
            {"id": str(i), "cells": r + [""] * (len(names) - len(r))}
            for i, r in enumerate(rows[1:], 1)
        ],
    }


def show(path, place=None):
    """Read the file and show it: in its table when it is open, else in a new one; at the row
    `place` names (its own, or one of its cells')."""
    read = ext.read(path)
    rows = parse(read["text"])
    keys = places(rows)
    row = place if place in keys else (place or "").rsplit("#", 1)[0]
    select = str(keys.index(row) + 1) if row in keys else None
    table = tables.get(path)
    if table is None:
        view = ext.open_view(content(path, rows), select)
        tables[path] = {"view": view, "rev": read["rev"], "rows": rows}
        paths[view] = path
    else:
        table.update(rev=read["rev"], rows=rows)
        ext.update_view(table["view"], content(path, rows), select)


@ext.on("file/open")
def file_open(params):
    # A view comes to the front only when it opens, so a table already open opens again.
    table = tables.get(params["path"])
    if table is not None:
        ext.close_view(table["view"])
        forget(table["view"])
    show(params["path"], params.get("place"))


@ext.on("file/changed")
def file_changed(params):
    path, rev = params["path"], params["rev"]
    table = tables.get(path)
    if table is None or rev == table["rev"]:
        return  # not on screen, or our own save
    if rev is None:
        ext.close_view(table["view"])
        forget(table["view"])
        ext.show(f"{path} is gone")
        return
    show(path)
    ext.show(f"{path} changed: showing it as it is now")


def save(path, row, column, value):
    """Set one cell and write the file, expecting the version on screen. When it changed
    meanwhile, read it again and set the cell in what is there now."""
    table = tables[path]
    for _ in range(3):
        rows = [list(r) for r in table["rows"]]
        if row >= len(rows):
            return ext.show(f"not saved: row {row} of {path} is gone")
        rows[row] += [""] * (column + 1 - len(rows[row]))
        rows[row][column] = value
        try:
            written = ext.write(path, unparse(rows), table["rev"])
        except Refused as e:
            if e.reason != "changed":
                return ext.show(f"not saved: {e}")
            read = ext.read(path)
            table.update(rev=read["rev"], rows=parse(read["text"]))
            continue
        table.update(rev=written["rev"], rows=rows)
        ext.update_view(table["view"], content(path, rows))
        return ext.show(f"saved {path}")
    ext.show(f"not saved: {path} keeps changing")


@ext.on("view/event")
def view_event(params):
    path = paths.get(params["view"])
    if path is None or params["event"] != "edit":
        return
    names = columns(tables[path]["rows"])
    save(path, int(params["row"]), names.index(params["column"]), params["value"])


def forget(view):
    tables.pop(paths.pop(view, None), None)


@ext.on("view/closed")
def view_closed(params):
    forget(params["view"])


@ext.command("csv-table.new")
def new(args):
    made = ext.create(name="Untitled", extension="csv", text=NEW)
    show(made["path"])
    ext.show(f"made {made['path']}")


ext.run()
