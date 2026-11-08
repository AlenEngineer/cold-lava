"""Open tasks: a count of the workspace's open tasks in the status line, kept current as notes
change, and a command that moves overdue tasks to today. An example cold-lava extension
(docs/extensions-guide.md)."""

import datetime
import os

from cold_lava import Extension, Refused

ext = Extension()
OPEN = {"status": ["todo", "in_progress"]}
open_ids = set()


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


@ext.on("items/reset")
def reset(params):
    count()


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


ext.run()
