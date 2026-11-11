"""Moves the file named in the command's args to the trash, and says how it went. The tests
in ../permissions.rs run it with and without files.delete, beside a copy of the examples'
cold_lava.py."""

from cold_lava import Extension, Refused

ext = Extension()


@ext.command("trash-can.delete")
def delete(args):
    path = args["path"]
    try:
        gone = ext.delete(path, ext.stat(path)["rev"])
        ext.show(f"deleted {path} to the {gone['went']} trash")
    except Refused as e:
        ext.show(f"refused ({e.reason}): {e}")


ext.run()
