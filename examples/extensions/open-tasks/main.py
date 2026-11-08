"""Open tasks: a count of the workspace's open tasks in the status line, kept current as notes
change, a command that moves overdue tasks to today, and a list of the open tasks by note that
cold-lava draws. An example cold-lava extension (docs/extensions-guide.md)."""

import datetime
import os

from cold_lava import Extension, Refused

ext = Extension()
OPEN = {"status": ["todo", "in_progress"]}
open_ids = set()
# The list's view while it is open, and the tasks it shows by id.
view = None
listed = {}


def today():
    # COLD_LAVA_TODAY pins the day for scripted runs, as it does for cold-lava's agenda.
    return os.environ.get("COLD_LAVA_TODAY") or datetime.date.today().isoformat()


def overdue():
    yesterday = datetime.date.fromisoformat(today()) - datetime.timedelta(days=1)
    due = {"field": "due", "to": yesterday.isoformat()}
    return ext.query("task", {**OPEN, "date": due})


def plural(n, word):
    return f"{n} {word}" + ("" if n == 1 else "s")


def show_count():
    ext.status(f"☐ {plural(len(open_ids), 'open task')}")


def count():
    open_ids.clear()
    open_ids.update(task["id"] for task in ext.query("task", OPEN))
    show_count()


def task_list():
    """The open tasks as a list view, a group for each note."""
    listed.clear()
    groups = {}
    for task in ext.query("task", OPEN):
        listed[task["id"]] = task
        check = "partial" if task["status_type"] == "in_progress" else "unchecked"
        row = {"id": task["id"], "text": [{"text": task["text"]}], "check": check}
        if task.get("due"):
            row["right"] = [{"text": task["due"]}]
        groups.setdefault(task["note"], []).append(row)
    return {
        "kind": "list",
        "title": "Open tasks",
        "groups": [
            {"id": note, "title": note.removesuffix(".md"), "rows": rows}
            for note, rows in groups.items()
        ],
        "keys": [{"key": "t", "command": "today", "title": "due today"}],
        "empty": "no open tasks",
    }


@ext.on("initialize")
def initialize(params):
    ext.subscribe("task", OPEN)
    count()
    return {}


@ext.on("items/changed")
def changed(params):
    open_ids.update(item["id"] for item in params["added"] + params["updated"])
    open_ids.difference_update(params["removed"])
    show_count()
    if view is not None:
        ext.update_view(view, task_list())


@ext.on("items/reset")
def reset(params):
    count()
    if view is not None:
        ext.update_view(view, task_list())


@ext.command("open-tasks.count")
def say_count(args):
    ext.show(f"{plural(len(open_ids), 'open task')}, {len(overdue())} overdue")


@ext.command("open-tasks.reschedule")
def reschedule(args):
    moved, refused = 0, []
    for task in overdue():
        try:
            ext.edit(task, "set-field", field="due", value=today())
            moved += 1
        except Refused as e:
            refused.append(f"{task['text']}: {e}")
    message = f"moved {plural(moved, 'overdue task')} to today"
    if refused:
        message += f"; {len(refused)} refused ({refused[0]})"
    ext.show(message)


@ext.command("open-tasks.list")
def show_list(args):
    global view
    if view is not None:
        ext.close_view(view)
    view = ext.open_view(task_list())


@ext.on("view/event")
def view_event(params):
    """Enter opens the task's note at its line, x ticks it, t makes it due today. The ticked
    task leaves the list when its items/changed comes."""
    task = listed.get(params.get("row"))
    if params["view"] != view or task is None:
        return
    try:
        if params["event"] == "activated":
            ext.open_note(task["note"], task["line"])
        elif params["event"] == "toggled":
            ext.edit(task, "toggle")
        elif params["event"] == "command" and params["command"] == "today":
            ext.edit(task, "set-field", field="due", value=today())
    except Refused as e:
        ext.show(f"not changed: {e}")


@ext.on("view/closed")
def view_closed(params):
    global view
    if params["view"] == view:
        view = None
        ext.show("closed the list of open tasks")


ext.run()
