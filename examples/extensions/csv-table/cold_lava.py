"""A small helper for cold-lava extensions in Python, with nothing to install.

cold-lava starts an extension in its own folder and speaks JSON-RPC 2.0 over its stdin and
stdout. Each message is framed as LSP frames it: a `Content-Length: N` header, an empty line,
then N bytes of UTF-8 JSON. Anything written to stderr goes to the extension's log
(`cold-lava ext list` says where), so print debugging output there, never to stdout.

Copy this file next to your extension's `main.py`. docs/extensions-guide.md explains the rest.
"""

import json
import sys
import traceback


class Refused(Exception):
    """cold-lava refused a request. `reason` is one of changed, refused, read-back, io,
    forbidden, no-workspace or exists (docs/extensions.md, "The workspace gateway, as built")."""

    def __init__(self, error):
        data = error.get("data") or {}
        self.code = error.get("code")
        self.reason = data.get("reason")
        super().__init__(error.get("message", ""))


class Extension:
    def __init__(self):
        self.workspace = None
        self._handlers = {}
        self._commands = {}
        self._next_id = 0
        self._backlog = []
        self._input = sys.stdin.buffer
        self._output = sys.stdout.buffer

    # What cold-lava asks of the extension.

    def on(self, method):
        """Handle a request or notification from cold-lava: `initialize`, `items/changed`,
        `items/reset`, `view/event`, `view/closed`, `file/open`, `file/changed`, `file/read`,
        `file/edit`, `file/preview`, `shutdown`.
        The handler gets the params; a request is answered with what it returns."""

        def register(handler):
            self._handlers[method] = handler
            return handler

        return register

    def command(self, command_id):
        """Run `command_id` (declared in extension.toml) when it is chosen in cold-lava. The
        handler gets the command's args."""

        def register(handler):
            self._commands[command_id] = handler
            return handler

        return register

    def run(self):
        """Answer cold-lava until it says `exit` or closes the connection."""
        while True:
            message = self._backlog.pop(0) if self._backlog else self._read()
            if message is None:
                return
            self._handle(message)

    # What the extension asks of cold-lava.

    def request(self, method, params=None):
        """Send a request and wait for its answer. Raises `Refused` when cold-lava refuses it."""
        self._next_id += 1
        request_id = self._next_id
        self._send(_message(method, params, request_id))
        while True:
            message = self._read()
            if message is None:
                raise EOFError("cold-lava closed the connection")
            if "method" not in message and message.get("id") == request_id:
                if "error" in message:
                    raise Refused(message["error"])
                return message.get("result")
            self._backlog.append(message)

    def notify(self, method, params=None):
        self._send(_message(method, params))

    def show(self, message):
        """A message in the status line, for a few seconds."""
        self.notify("ui/notify", {"message": message})

    def status(self, text):
        """The extension's own part of the status line, kept until the next call; None clears."""
        self.notify("ui/status", {"text": text})

    def query(self, kind, filter=None, limit=500):
        """Every item of `kind` (task, list, heading, block or a type) that `filter` keeps, read
        a page at a time."""
        items, after = [], None
        while True:
            page = {"limit": limit}
            if after is not None:
                page["after"] = after
            params = {"kind": kind, "page": page}
            if filter:
                params["filter"] = filter
            found = self.request("items/query", params)
            items += found["items"]
            after = found["next"]
            if after is None:
                return items

    def subscribe(self, kind, filter=None):
        """Hear `items/changed` and `items/reset` about these items from now on."""
        params = {"kind": kind}
        if filter:
            params["filter"] = filter
        return self.request("items/subscribe", params)["subscription"]

    def edit(self, item, op, **fields):
        """Change one item, refused as `changed` if it isn't the version you read."""
        params = {"id": item["id"], "rev": item["rev"], "op": op, **fields}
        return self.request("items/edit", params)

    def read(self, path):
        """A file's text and its version, {"text", "rev"}. Paths are relative to the workspace."""
        return self.request("workspace/read", {"path": path})

    def write(self, path, text, expect):
        """Replace a file's text, refused as `changed` unless it is still the version `expect`
        (the `rev` you read). Returns {"rev"}, the version now on disk."""
        return self.request("workspace/write", {"path": path, "text": text, "expect": expect})

    def create(self, text="", path=None, name=None, extension=None):
        """Make a file at `path`, or named `name` with `extension` where Obsidian puts new files
        (`Untitled 1.csv` when `Untitled.csv` is taken). Never overwrites: a `path` that is
        taken is refused as `exists`. Returns {"path", "rev"}."""
        params = {"text": text}
        if path is not None:
            params["path"] = path
        else:
            params.update(name=name, extension=extension)
        return self.request("workspace/create", params)

    def files(self, extension):
        """Every file in the workspace whose name ends in `.extension`."""
        return self.request("workspace/files", {"extension": extension})["files"]

    def stat(self, path):
        """The version of a file or a folder without its text: {"rev"}, and for a folder
        {"rev", "inside"}, everything in it. It is the `expect` a delete needs."""
        return self.request("workspace/stat", {"path": path})

    def delete(self, path, expect):
        """Move a file or a folder to the trash (never for good), refused as `changed` unless
        it is still the version `expect`. Needs the `files.delete` permission. Returns
        {"trashed", "went"}: the token `restore` takes, and `system` or `workspace`."""
        return self.request("workspace/delete", {"path": path, "expect": expect})

    def restore(self, trashed, to=None):
        """Put back what `delete` answered as `trashed`, where it was or at `to`. Never
        overwrites: a taken path is refused as `exists`. The token is gone once cold-lava
        restarts. Returns {"path"}."""
        params = {"trashed": trashed}
        if to is not None:
            params["to"] = to
        return self.request("workspace/restore", params)

    def create_folder(self, path):
        """Make an empty folder, with any missing parents; `exists` when something is there."""
        return self.request("workspace/create-folder", {"path": path})

    def style(self):
        """How the workspace writes (list marker, links, task fields, ...), each as
        {"value", "from", "counts"}: for text the extension writes itself."""
        return self.request("workspace/style")

    def open_view(self, content, select=None, file=None):
        """Show a list or a table ({"kind": "list", ...}) in cold-lava, which draws it and owns
        its keys, at the row `select`; when it shows a workspace `file`, cold-lava lists who
        links to that file under it. Returns the view's id, which `view/event` and
        `view/closed` name."""
        params = {"content": content}
        if select is not None:
            params["select"] = select
        if file is not None:
            params["file"] = file
        return self.request("view/open", params)["view"]

    def update_view(self, view, content, select=None):
        """What the view shows now. cold-lava keeps the selection, folds, sort and filter by
        row id, or selects the row `select`."""
        params = {"view": view, "content": content}
        if select is not None:
            params["select"] = select
        self.request("view/update", params)

    def close_view(self, view):
        self.notify("view/close", {"view": view})

    def show_view(self, view):
        """Bring one of your open views to the front, as `open_view` does with a new one. Back
        returns to where the user was."""
        self.notify("view/show", {"view": view})

    def open_note(self, note, line=None):
        """Open a note in cold-lava (an item's `note`, or a path inside the workspace), at a
        line. Back returns to where the user was."""
        params = {"note": note}
        if line is not None:
            params["line"] = line
        self.notify("workspace/open", params)

    # The wire.

    def _handle(self, message):
        method, request_id = message.get("method"), message.get("id")
        if method is None:
            return
        params = message.get("params") or {}
        try:
            if method == "exit":
                sys.exit(0)
            if method == "initialize":
                self.workspace = params.get("workspace")
            if method == "command/run":
                handler = self._commands.get(params.get("id"))
                if handler is None:
                    raise LookupError(f"no command {params.get('id')}")
                result = handler(params.get("args"))
            elif method in self._handlers:
                result = self._handlers[method](params)
            elif method in ("initialize", "shutdown") or request_id is None:
                result = None
            else:
                raise LookupError(f"no method {method}")
        except Exception as e:
            traceback.print_exc(file=sys.stderr)
            if request_id is not None:
                error = {"code": -32603, "message": str(e) or type(e).__name__}
                self._send({"jsonrpc": "2.0", "id": request_id, "error": error})
            return
        if request_id is not None:
            self._send({"jsonrpc": "2.0", "id": request_id, "result": result})

    def _send(self, message):
        body = json.dumps(message, ensure_ascii=False).encode("utf-8")
        self._output.write(b"Content-Length: %d\r\n\r\n" % len(body))
        self._output.write(body)
        self._output.flush()

    def _read(self):
        length = None
        while True:
            line = self._input.readline()
            if not line:
                return None
            line = line.strip()
            if not line:
                break
            name, _, value = line.decode("ascii").partition(":")
            if name.strip().lower() == "content-length":
                length = int(value)
        body = self._input.read(length or 0)
        return json.loads(body.decode("utf-8")) if body else None


def _message(method, params=None, request_id=None):
    message = {"jsonrpc": "2.0", "method": method}
    if request_id is not None:
        message["id"] = request_id
    if params is not None:
        message["params"] = params
    return message
