# Foundations

The ideas cold-lava is built on. This page states them and nothing else: it doesn't say what
exists yet, what is planned, or in which order. For that, [engine.md](engine.md) describes
what is built, and the pinned roadmap issue lists the work.

When another document and this page disagree about an idea, this page is right and the other
one is out of date.

## What cold-lava is

A typed, live layer over the plain files a person thinks in. It is built the way a code editor
is built, for human notes: formats have owners as languages have language servers, types are
the type system, and extensions are the plugins. The files stay the person's own, and stay
readable by other tools.

It can be a layer over every kind of file a person uses. No format is privileged in principle:
whatever a file is written in, an owner can bring what it holds into the same types, links and
index.

## The engine

### 1. Plain files are the store, and they are never harmed

- An edit made elsewhere is never overwritten: a write is refused when the file changed since
  it was read.
- Files are written atomically, and deleted only to the trash.
- What the engine doesn't understand in a file, it keeps.
- A note stays a valid Markdown file that other tools can open. New meaning comes from
  readings of text other tools already accept, never from syntax they would choke on.

This is about being a good citizen on files that other programs also edit. It belongs to no
other product's conventions.

### 2. The core knows data, not syntax

The core holds items, links, types, identity, the index and safe writes. How a file is written
belongs to that format's owner. Markdown is one owner among others: it may be built in and
fast, but it gets no private doors.

When a choice comes up, take the option that doesn't make Markdown more special.

### 3. Separated by file type, united by type data

An owner translates one format into items and links, and one edit back into the file. A task
is a task wherever it is written: in a note, on a canvas card, in a row of a table. Nothing
above the owners needs to know which file an item came from.

### 4. Types are the shared contract

- Meaning is declared as data, not as code: a type says what a thing is and which fields it
  has.
- A shared vocabulary lets two extensions that have never heard of each other agree on what
  an address or a person is.
- What crosses a boundary is typed data, not text that has to be read again. An owner should
  never have to speak Markdown to say what its file holds.

### 5. Identity is precious

An item has an address (where it is now) and an anchor (a name written into the file the first
time something keeps a reference to it). An address may move; an anchor doesn't. Changing an
item's identity breaks what refers to it, so it is never done quietly: it is the user's
decision.

### 6. One reading of a note

One lossless tree is what the reader, the index, items, renames and every other reader see.
They can't disagree about what is a link, a tag or a task, because there is nothing else to
read.

### 7. Interpretation is a dialect

- A **flavour** is how a format's text is read: which constructs exist, and how each edge
  case goes. A **profile** is a flavour together with an ecosystem's habits: its settings
  files, how a short link resolves, where a new note goes.
- cold-lava has its own flavour, chosen on its merits. Another product's reading, quirks
  included, is a profile a workspace can choose. It never defines the core.
- Nothing a person types may silently hide the rest of a note. What is unclosed is shown as
  text, and said.

### 8. One door

- Every program reaches the engine through one versioned protocol, with permissions. No
  frontend and no extension has another way in.
- Every change names who made it.
- A change to the whole workspace is a plan first: what would change, shown before anything
  is written, then applied.
- A client acts on items by identity and relation (this item; before, after or under that
  one), never by line number.

### 9. One live workspace

There is one watcher. Every change is announced once, in order, whoever made it. The engine
never waits on an extension: what an extension hasn't answered yet is simply not there yet.

### 10. Extensible by data

Four things extend without code: types (what things mean), shapes (what they look like),
flavours (how text is read) and file formats, through owners. Extensions bring data, formats,
types and simple views. They never see keystrokes and never draw, so a broken one can't trap
anyone.

### 11. Features are not the core

What is built on the workspace (tasks, daily notes, whole-workspace changes, drawings bound to
data) uses the same door an extension uses. A feature that needs something the door doesn't
offer is a reason to widen the door, not to go around it.

### 12. A drawing means something only through bindings

What a drawing means is an ordinary edit to typed data, made through an explicit binding.
Where a thing sits and what colour it has stay in the drawing's file. Nothing is inferred from
geometry. What a bound drawing shows is also saved as plain text, so another tool that opens
the file shows something readable.

### 13. A closed alphabet, open attributes

What a person can write, in any format, is a small, closed set: pieces of writing with a
role, the text inside them, and how they stand to one another (inside, after, beside, placed,
linked). Every piece carries open attributes, and that is where a format's own details go.
Types are the words written in that alphabet, and anyone can coin a word.

cold-lava can never be shaped for everyone, so each person shapes it for themselves: by
choosing how a format is read, by declaring what a thing means or looks like in a few lines
of data, or by installing an owner for the files they use. Only the alphabet itself is the
engine's. ([neutral-model.md](neutral-model.md) has the design.)

## Frontends

### 1. A frontend is a client

It asks the engine and draws. It has no private access to files. A terminal app, a graphical
app and a language server are peers on the same door.

### 2. Built-in surfaces, extension views

The surfaces a person works in (the page, with its reader and editor, and the whiteboard) are
built into the frontend: typing, selection, drawing and undo need every frame. What extensions
show (lists, tables, scenes) is declared as data, and each frontend draws it its own way.

### 3. One loop

Everything on screen runs in one loop, so extensions are answered whatever is in front. No
screen gets a loop of its own.

### 4. Unsaved work is never taken away

Nothing an extension does can close a surface that holds typed or drawn work. A request that
is refused for that reason is shown, with the reason.

### 5. A scene is the container, a shape is the contents

A scene says which things are shown, how they are linked, and who decides where they go: the
person, or the program. A whiteboard and a graph are the same kind of thing with a different
answer to that question. A shape says what one thing looks like and what data fills it. One
renderer draws them. Lists and tables stay lists and tables.

### 6. Geometry belongs to no frontend

What a drawing looks like is computed without a screen, so every frontend computes the same
picture and only paints it.

### 7. The notebook

Writing, drawing and mapping happen on one page, without switching tools: a note rendered as
it is typed, a drawing started where the cursor is, a map that follows the note's structure.
The page is still a plain file.

## The order of work

The engine-facing work comes first and the experience last. Each layer is finished, and has
one way of doing each thing, before the next is built on it.

## How the code is built

### Tools, called by a simple loop, all the way down

A good architecture is a set of tools and a loop that uses them. The loop only initialises
its tools and calls them, so it is short and can be read from top to bottom.

The rule is recursive. Every unit of code is one of two things:

- **A caller.** It does its work by calling its tools in order. It holds no logic of its own
  beyond deciding what to call next.
- **A leaf.** It needs nothing, or only a dependency the project doesn't control (the
  standard library, an external crate, the operating system).

A tool owns its state. A caller can ask it to do something; it cannot reach inside. Two tools
that need the same thing are each handed it by their caller, and neither goes through the
other.

Complexity is allowed where speed needs it, as a decision. Where code is tangled on purpose,
it says so and says why, at the place where it is tangled. Complexity nobody decided on is a
defect, however well it works.

When a unit is neither a caller nor a leaf, the fix is to split it until each part is one or
the other. Adding to it is never the fix.

## Open questions

- **Proof.** An idea on this page counts as proven once someone has lived in it.
