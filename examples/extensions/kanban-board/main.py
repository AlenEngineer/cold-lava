"""Kanban board: shows Obsidian Kanban boards, notes with `kanban-plugin` in their frontmatter,
as a list with a group for each lane (a heading) and a row for each card (a task under it).
`x` ticks a card and Enter opens the note at the card's line. The board stays a Markdown note:
cold-lava reads its tasks and links and writes the ticks, and Obsidian still opens it. An
example cold-lava extension (docs/extensions-guide.md)."""

from cold_lava import Extension, Refused

ext = Extension()
# Each board shown, by its note: {"view", "cards"}; and the note each view shows.
boards = {}
notes = {}
# The notes whose items it follows, so it subscribes once per note.
followed = set()

CHECKS = {"done": "checked", "cancelled": "crossed", "in_progress": "partial"}


def lanes(path):
    """The board as a list view: its headings in order, each with the tasks under it."""
    headings = ext.query("heading", {"note": path})
    cards = ext.query("task", {"note": path})
    groups, by_lane = [], {}
    for heading in headings:
        group = {"id": heading["id"], "title": heading["text"], "rows": []}
        groups.append(group)
        by_lane.setdefault(heading["text"], group)
    loose = {"id": "loose", "title": "Not in a lane", "rows": []}
    for card in cards:
        lane = by_lane.get(card["section"][-1]) if card["section"] else None
        check = CHECKS.get(card.get("status_type"), "unchecked")
        row = {"id": card["id"], "text": [{"text": card["text"]}], "check": check}
        (lane or loose)["rows"].append(row)
    if loose["rows"]:
        groups.append(loose)
    content = {
        "kind": "list",
        "title": path.removesuffix(".md").rsplit("/", 1)[-1],
        "groups": groups,
        "empty": "no lanes: a lane is a heading",
    }
    return content, {card["id"]: card for card in cards}


def follow(path):
    if path not in followed:
        followed.add(path)
        ext.subscribe("task", {"note": path})
        ext.subscribe("heading", {"note": path})


def refresh():
    for path, board in boards.items():
        content, board["cards"] = lanes(path)
        ext.update_view(board["view"], content)


@ext.on("file/open")
def open_board(params):
    path = params["path"]
    content, cards = lanes(path)
    board = boards.get(path)
    if board is not None:
        board["cards"] = cards
        ext.update_view(board["view"], content)
        ext.show_view(board["view"])
        return
    follow(path)
    view = ext.open_view(content, file=path)
    boards[path] = {"view": view, "cards": cards}
    notes[view] = path


@ext.on("items/changed")
def changed(params):
    refresh()


@ext.on("items/reset")
def reset(params):
    refresh()


@ext.on("view/event")
def view_event(params):
    path = notes.get(params["view"])
    card = boards[path]["cards"].get(params.get("row")) if path else None
    if card is None:
        return
    try:
        if params["event"] == "toggled":
            ext.edit(card, "toggle")
        elif params["event"] == "activated":
            ext.open_note(card["note"], card["line"])
    except Refused as e:
        ext.show(f"not changed: {e}")


@ext.on("view/closed")
def view_closed(params):
    path = notes.pop(params["view"], None)
    boards.pop(path, None)


ext.run()
