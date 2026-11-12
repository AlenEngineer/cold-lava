"""Follows the workspace's headings and keeps how much news it heard in the status line, for
delete-news.drive. tests/drive.rs installs it beside a copy of the examples' cold_lava.py."""

from cold_lava import Extension

ext = Extension()
heard = {"changed": 0, "reset": 0}


def say():
    ext.status(f"heard {heard['changed']} changed, {heard['reset']} reset")


@ext.on("initialize")
def initialize(params):
    ext.subscribe("heading")
    say()
    return {}


@ext.on("items/changed")
def changed(params):
    heard["changed"] += 1
    say()


@ext.on("items/reset")
def reset(params):
    heard["reset"] += 1
    say()


ext.run()
