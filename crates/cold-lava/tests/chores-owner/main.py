"""Chores: the owner of a made-up format that has nothing to do with Markdown, for
tests/owned.rs, which installs it beside a copy of the examples' cold_lava.py.

A `.chores` file has a chore a line, `name | state | due | words`, and a line that starts with
`;` is a remark. In the words, `@word` is a label and `>Word` a link to the file `Word`. The
states are the format's own: todo, done, doing, dropped.

It tells cold-lava what a file holds as pieces (docs/extensions.md, "Pieces, as built"), and
writes back the changes cold-lava asks for ("Changes in the model's terms, as built"). It
never writes, escapes or parses a character of Markdown."""

from cold_lava import Extension, change_kind, label, link, piece

ext = Extension()
STATES = {"todo": "open", "done": "done", "doing": "in-progress", "dropped": "cancelled"}
WORDS = {word: state for state, word in STATES.items()}


def chore(line):
    """A line's four parts, when it is a chore."""
    parts = line.split("|", 3)
    if line.startswith(";") or len(parts) != 4:
        return None
    return [part.strip() for part in parts]


def runs(words):
    """A chore's words as runs: plain characters, labels and links."""
    out, plain = [], ""
    for i, token in enumerate(words.split(" ")):
        if i:
            plain += " "
        if len(token) > 1 and token[0] in "@>":
            if plain:
                out.append(plain)
            out.append(label(token[1:]) if token[0] == "@" else link(token[1:]))
            plain = ""
        else:
            plain += token
    if plain:
        out.append(plain)
    return out


@ext.on("file/read")
def file_read(params):
    items = []
    for line in params["text"].splitlines():
        parts = chore(line)
        if parts is None:
            continue
        name, state, due, words = parts
        attributes = {"task": True, "checkbox": STATES.get(state)}
        if due:
            attributes["due"] = due
        items.append(piece(name, "item", runs(words), attributes))
    return {"items": items}


def with_part(line, n, new):
    """One part of a chore's line replaced, the space around it kept, so that no other byte of
    the file moves."""
    parts = line.split("|", 3)
    old = parts[n]
    if not old.strip():
        parts[n] = f" {new} " if new else old
    else:
        lead, trail = len(old) - len(old.lstrip()), len(old) - len(old.rstrip())
        parts[n] = old[:lead] + new + old[len(old) - trail :]
    return "|".join(parts)


def written(text):
    """Runs as the format writes them."""
    out = ""
    for run in text:
        if isinstance(run, str):
            out += run
        elif "label" in run:
            out += "@" + run["label"]
        elif "link" in run:
            out += ">" + run["link"]
        else:
            raise ValueError("a chore's words can't hold a mark or a field")
    return out


@ext.on("file/edit")
def file_edit(params):
    lines = params["text"].split("\n")
    for change in params["changes"]:
        place, kind = change["place"], change_kind(change)
        at = [i for i, line in enumerate(lines) if (chore(line) or [None])[0] == place]
        if not at:
            raise ValueError(f"no chore called {place}")
        i = at[0]
        if kind == "attribute" and change["attribute"] == "checkbox":
            lines[i] = with_part(lines[i], 1, WORDS[change["value"]])
        elif kind == "attribute" and change["attribute"] == "due":
            lines[i] = with_part(lines[i], 2, change["value"] or "")
        elif kind == "runs" and "cell" not in change:
            lines[i] = with_part(lines[i], 3, written(change["text"]))
        elif kind == "link":
            lines[i] = lines[i].replace(">" + change["link"], ">" + change["to"])
        else:
            raise ValueError(f"a chore can't hold that change ({kind})")
    return {"text": "\n".join(lines)}


ext.run()
