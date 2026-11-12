"""CSV tables: opens the workspace's .csv files as tables cold-lava draws, saves an edited cell,
follows changes made elsewhere, and makes new tables. It also tells cold-lava what each file
holds, in cold-lava's alphabet and not as Markdown: every row is a `row` piece whose cells are
plain characters, so `#42`, `$5`, `[[x]]` and `- [ ] y` in a cell are those characters. What a
column's cells are beyond that, the header says:

    Volcano (link),Year,VEI,Notes (markdown)
    Krakatoa,1883,6,- [ ] Read the report on [[Krakatoa]]

- `(link)`: each cell is the name of a file of the workspace, and a link to it. A heading
  or the words to show may follow the name, as in `Krakatoa#Eruptions` or `Krakatoa|the big one`;
- `(markdown)`: each cell is Markdown of its own, handed over as a Markdown piece, so a task
  in it is a task. That is the one place this file speaks Markdown: because the cell does.

When an item of a file is edited, or a note it links to renamed, cold-lava says what changed in
the model's terms (a cell's new text, a link's new target) and this writes that cell, and no
other byte of the file. An example cold-lava extension that owns a kind of file
(docs/extensions-guide.md, "Owning a kind of file", "Giving items and links" and "Writing
items back")."""

from cold_lava import Extension, Refused, change_kind, link, piece, words

ext = Extension()
# Each table on screen, by path: its view, and the version and text it shows.
tables = {}
paths = {}
NEW = "name,notes\n,\n"
# What a header can say its column's cells are, after the column's name.
KINDS = {" (link)": "link", " (markdown)": "markdown"}
# cold-lava takes a place of up to 1024 bytes, a name of up to 256 and a link's target of up
# to 4096: a row's place and a column's name are cut to fit, and a longer link is characters.
LONGEST = 200
LONGEST_NAME = 60


def spans(text):
    """The file's rows, each a list of cells `(start, end, value)`: where the cell is written
    in `text`, quotes included, and what it says. A blank line is a row with no cells. With
    them, where each row ends (before its line break), for a cell added to a short row."""
    rows, ends = [], []
    i, n = 0, len(text)
    while i < n:
        row = []
        if text[i] not in "\r\n":
            while True:
                start = i
                value = []
                if i < n and text[i] == '"':
                    i += 1
                    while i < n:
                        if text[i] == '"':
                            if text[i + 1 : i + 2] == '"':
                                value.append('"')
                                i += 2
                                continue
                            i += 1
                            break
                        value.append(text[i])
                        i += 1
                while i < n and text[i] not in ",\r\n":
                    value.append(text[i])
                    i += 1
                row.append((start, i, "".join(value)))
                if i < n and text[i] == ",":
                    i += 1
                    if i == n or text[i] in "\r\n":
                        row.append((i, i, ""))
                        break
                    continue
                break
        rows.append(row)
        ends.append(i)
        if text[i : i + 2] == "\r\n":
            i += 2
        elif i < n:
            i += 1
    return rows, ends


def parse(text):
    """The file's rows as lists of what each cell says."""
    return [[value for _, _, value in row] for row in spans(text)[0]]


def written(value):
    """A cell as CSV writes it: in quotes only when it has to be."""
    if any(c in value for c in ',"\r\n'):
        return '"' + value.replace('"', '""') + '"'
    return value


def set_cell(text, row, column, value):
    """`text` with one cell saying `value`: that cell's bytes change, and nothing else. A row
    too short to have the cell gets the commas up to it."""
    rows, ends = spans(text)
    cells = rows[row]
    if column < len(cells):
        start, end, _ = cells[column]
        # A row's only cell, emptied, is written `""`: an empty line would be no row.
        new = written(value) or ('""' if len(cells) == 1 else "")
        return text[:start] + new + text[end:]
    commas = column - len(cells) + (1 if cells else 0)
    return text[: ends[row]] + "," * commas + written(value) + text[ends[row] :]


def columns(rows):
    """The header row's names as written, each once (a table's edits name their column), and a
    name for each column past it."""
    header = rows[0] if rows else []
    width = max([len(r) for r in rows] or [0])
    names = []
    for i in range(width):
        name = header[i] if i < len(header) and header[i] else f"column {i + 1}"
        while name in names:
            name += "'"
        names.append(name)
    return names


def shape(rows):
    """What each column is, from the header: its name without what follows it in brackets,
    and `link`, `markdown` or nothing for plain characters."""
    names, kinds = [], []
    for name in columns(rows):
        kind = ""
        for suffix, said in KINDS.items():
            if name.endswith(suffix) and len(name) > len(suffix):
                name, kind = name[: -len(suffix)], said
        name = name[:LONGEST_NAME]
        while name in names:
            name += "'"
        names.append(name)
        kinds.append(kind)
    return names, kinds


def places(rows):
    """A place for each row under the header that stays when rows are added above it: its first
    cell, numbered when another row has the same, or its number when it has none. A blank
    line is no row and has no place."""
    taken = set()
    out = []
    for n, row in enumerate(rows[1:], 1):
        if not row:
            out.append(None)
            continue
        key = row[0][:LONGEST] or f"row {n}"
        # No two rows share a place, whatever their cells say: a row whose first cell is
        # literally `a (2)` takes that place, and the second `a` gets the next number free.
        place, number = key, 1
        while place in taken:
            number += 1
            place = f"{key} ({number})"
        taken.add(place)
        out.append(place)
    return out


def link_cell(value):
    """A link cell's two parts: the file it names, and what follows the name, a heading
    (`Krakatoa#Eruptions`) or the words to show (`Krakatoa|the 1883 one`). The link is to
    the file; a rename changes the name and keeps what follows."""
    cut = min([i for i in (value.find("#"), value.find("|")) if i >= 0] or [len(value)])
    return value[:cut], value[cut:]


def link_run(value):
    """A link cell as a link run: to the file it names, showing the words after `|` when it
    has some. `None` for a cell that names no file."""
    target, rest = link_cell(value)
    if not target.strip() or len(value) > 1000:
        return None
    return link(target, rest.partition("|")[2] or None)


def given(text, pieces=True):
    """What a file holds, for `file/read`: each row a `row` piece with its cells by column, a
    plain cell as characters and a link cell as a link. A Markdown cell is empty in the row
    and is a Markdown piece of its own, at `<row's place>#<column>`. A cold-lava that takes
    no pieces yet (`pieces` false) is given the Markdown cells alone."""
    rows = parse(text)
    names, kinds = shape(rows)
    items = []
    for place, row in zip(places(rows), rows[1:]):
        if place is None:
            continue
        row = row + [""] * (len(names) - len(row))
        cells, markdown = [], []
        for name, kind, value in zip(names, kinds, row):
            if kind == "markdown":
                cells.append((name, []))
                if value.strip():
                    markdown.append({"place": f"{place}#{name}", "text": value})
            elif kind == "link" and link_run(value):
                cells.append((name, [link_run(value)]))
            else:
                cells.append((name, [value] if value else []))
        if pieces:
            items.append(piece(place, "row", cells=cells))
        items.extend(markdown)
    return {"items": items}


@ext.on("file/read")
def file_read(params):
    # Pieces are for a cold-lava whose alphabet is 2 or more: `initialize` says which it is.
    return given(params["text"], pieces=ext.alphabet >= 2)


def cell_text(name, kind, runs, now):
    """What a cell that says `now` says once its text is `runs`, as cold-lava asked. A plain
    cell holds characters and a link cell one link, so runs that say more than the column can
    hold are refused: cold-lava then writes nothing and shows why."""
    runs = [run for run in runs if run != ""]
    if kind == "markdown":
        raise ValueError(f"{name} holds Markdown: change what is written in it as its own item")
    if kind == "link":
        if not runs:
            return ""
        # The link it has, or a link to another file with nothing after its name. Words are
        # not a link, and cold-lava can't yet be given one from a field's new value.
        if runs == [link_run(now)]:
            return now
        one = runs[0] if len(runs) == 1 and isinstance(runs[0], dict) else {}
        if set(one) != {"link"} or link_run(one["link"]) != one:
            raise ValueError(
                f"{name} is a column of links, and each cell is the name of a file: "
                "set it in the CSV table, or rename the file it names"
            )
        return one["link"]
    if any(not isinstance(run, str) for run in runs):
        raise ValueError(f"{name} holds plain characters: no link, label or mark")
    return words(runs)


def cells_changed(rows, names, kinds, at, asked):
    """The cells one of `file/edit`'s changes sets, as `(row, column, value)`."""
    place, kind = asked["place"], change_kind(asked)
    if kind == "text":
        # A Markdown cell's new text, at `<row's place>#<column>`.
        row_place, _, name = place.rpartition("#")
        markdown = [n for n, k in zip(names, kinds) if k == "markdown"]
        if row_place not in at or name not in markdown:
            raise ValueError(f"no Markdown cell at {place}")
        return [(at[row_place], names.index(name), asked["text"])]
    if place not in at:
        raise ValueError(f"no row at {place}")
    row = at[place]
    cells = rows[row] + [""] * (len(names) - len(rows[row]))
    if kind == "runs" and asked.get("cell") in names:
        column = names.index(asked["cell"])
        value = cell_text(names[column], kinds[column], asked["text"], cells[column])
        if column == 0 and not value:
            raise ValueError("a row needs its first cell: it names the row")
        return [(row, column, value)]
    if kind == "link":
        # The file a link cell names was renamed: every such cell of the row follows.
        named = [
            (c, link_cell(cells[c])[1])
            for c, k in enumerate(kinds)
            if k == "link" and link_cell(cells[c])[0] == asked["link"]
        ]
        if not named:
            raise ValueError(f"the row {place} has no link to {asked['link']}")
        # The file's new name, and what followed the old one (a heading, the words to show).
        return [(row, column, asked["to"] + rest) for column, rest in named]
    raise ValueError(f"a CSV row can't hold that change ({kind})")


@ext.on("file/edit")
def file_edit(params):
    """The file's new text with each change made in the cell it names. Every change names a
    place of the file as cold-lava read it, so the cells are all found first and written
    after: one change may set the cell another's row is named by."""
    text = params["text"]
    rows = parse(text)
    names, kinds = shape(rows)
    at = {place: n for n, place in enumerate(places(rows), 1) if place is not None}
    cells = []
    for asked in params["changes"]:
        cells += cells_changed(rows, names, kinds, at, asked)
    for row, column, value in cells:
        text = set_cell(text, row, column, value)
    return {"text": text}


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


@ext.on("file/preview")
def file_preview(params):
    """A note embeds the file (`![[Eruptions.csv]]`): its header and first rows, as many as fit
    in the lines the reader has."""
    table = content(params["path"], parse(ext.read(params["path"])["text"]))
    table["rows"] = table["rows"][: max(params["height"] - 1, 1)]
    return table


def show(path, place=None):
    """Read the file and show it: in its table when it is open, else in a new one; at the row
    `place` names (its own, or one of its cells')."""
    read = ext.read(path)
    rows = parse(read["text"])
    keys = places(rows)
    row = place if place in keys else (place or "").rsplit("#", 1)[0]
    select = str(keys.index(row) + 1) if row and row in keys else None
    table = tables.get(path)
    if table is None:
        view = ext.open_view(content(path, rows), select, file=path)
        tables[path] = {"view": view, "rev": read["rev"], "text": read["text"]}
        paths[view] = path
    else:
        table.update(rev=read["rev"], text=read["text"])
        ext.update_view(table["view"], content(path, rows), select)


@ext.on("file/open")
def file_open(params):
    show(params["path"], params.get("place"))
    ext.show_view(tables[params["path"]]["view"])


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
        if row >= len(parse(table["text"])):
            return ext.show(f"not saved: row {row} of {path} is gone")
        text = set_cell(table["text"], row, column, value)
        try:
            saved = ext.write(path, text, table["rev"])
        except Refused as e:
            if e.reason != "changed":
                return ext.show(f"not saved: {e}")
            read = ext.read(path)
            table.update(rev=read["rev"], text=read["text"])
            continue
        table.update(rev=saved["rev"], text=text)
        ext.update_view(table["view"], content(path, parse(text)))
        return ext.show(f"saved {path}")
    ext.show(f"not saved: {path} keeps changing")


@ext.on("view/event")
def view_event(params):
    path = paths.get(params["view"])
    if path is None or params["event"] != "edit":
        return
    names = columns(parse(tables[path]["text"]))
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
