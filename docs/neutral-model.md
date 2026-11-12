# The neutral model

Status: **a design note for review.** Nothing here is decided or built. It answers the open
question in [foundations.md](foundations.md): the different things a person can write and the
data they can share, named without reference to any one format, and whether that set can be
extended.

## Why

cold-lava can never be shaped for everyone. People don't think along the same paths: one
writes outlines, another prose, another tables, another a board of cards. And they don't use
the same files: Markdown, a word processor's documents, LaTeX, spreadsheets.

So the aim is not a model that fits everyone. It is a model small enough that everyone's way
of writing can be brought into it, with enough left open that each person can shape the rest
for themselves, without waiting for cold-lava to do it for them.

## What has to be true

1. **A person can shape it without writing engine code.** By choosing (a setting), by
   declaring (a few lines of data), or by installing something that someone else wrote.
2. **What is written in one format is usable from every view.** A task is a task wherever it
   was typed.
3. **Files are never harmed.** An edit made through the model changes only what it names, and
   the rest of the file stays byte for byte.
4. **The built-in format stays fast.** Reading a note costs what it costs today.

## The proposal in one picture

```
 your files           .md      .canvas      .csv      .tex      …
                       │          │           │         │
 OWNERS                ▼          ▼           ▼         ▼        open: anyone can write one
 (one per format;   ┌──────────────────────────────────────┐
  a flavour is      │ THE ALPHABET                         │    closed and small
  how one reads)    │ pieces · text · relations            │
                    │ each with open ATTRIBUTES            │    open: any owner adds them
                    └──────────────────┬───────────────────┘
                                       ▼
 TYPES              what a thing means: a task, a book, an address    open: data you write
                                       ▼
 items, links, the index, safe edits                                  the engine
                                       ▼
 SHAPES and VIEWS   what a thing looks like, and where it is shown    open: data you write
```

The one closed part is the alphabet. Everything a person would want to change sits in a part
that is open.

## The alphabet

The alphabet is the set of things a person can write, in any format. It has three kinds of
letter.

### Pieces

A piece is one unit of written content. Each has a **role**:

| Role | What it is | Markdown | A canvas | A table file | LaTeX |
| --- | --- | --- | --- | --- | --- |
| `document` | the whole file | the note | the board | the sheet | the file |
| `section` | a title and what is under it | a heading and its body | a group | — | `\section{}` |
| `text` | a run of prose | a paragraph | a text card | a cell | a paragraph |
| `item` | one entry in a list; may hold more | a list item | — | — | `\item` |
| `row` | one record, in cells | a table row | — | a row | a `tabular` row |
| `aside` | set apart from the flow | a quote, a callout | — | — | `quote`, a margin note |
| `verbatim` | text kept exactly | a code block | — | — | `verbatim` |
| `embed` | something from elsewhere, shown here | `![[x]]`, an image | a file card | — | `\includegraphics`, `\input` |
| `opaque` | kept, not understood | an HTML block | an unknown node | — | a macro nobody declared |

`opaque` is the first foundation applied to the model: what an owner can't express, it hands
over as a piece that is carried through untouched.

### Text

What is inside a piece: plain runs, and runs with a **mark** (emphasis, strong, struck,
highlighted, code, math). Three things can sit in text: a **link** (to a file, to a piece, or
out to the web), a **label** (a tag), and a **field** (a named value written inline).

### Relations

How pieces stand to one another. This is what lets different ways of thinking share one model:

| Relation | Meaning | Who thinks this way |
| --- | --- | --- |
| **contains** | a piece is inside another | outlines, sections, nested lists |
| **follows** | a piece comes after another | prose, steps |
| **beside** | pieces are cells of one record | tables |
| **placed** | a piece sits at a position | boards, maps |
| **links to** | a piece points at another, perhaps with a label | wikis, arrows on a board, citations |

A note is mostly *contains* and *follows*. A board is mostly *placed* and *links to*. A sheet
is *beside*. None of them is the normal case with the others as exceptions.

### Attributes

Every piece can carry attributes: a name and a typed value. This is where a format's own
details go, and it is open: an owner may attach any attribute.

| Written as | Becomes |
| --- | --- |
| `- [ ] call the plumber` | an `item` with `checkbox = open` |
| `\todo{call the plumber}` | an `item` with `checkbox = open` |
| `> [!warning] Hot` | an `aside` with `callout = warning` |
| `## Method` | a `section` with `level = 2` |
| a card at (40, 120) on a board | a `text` with `x = 40`, `y = 120` |
| `📅 2026-11-20` on a task | the attribute `due = 2026-11-20` |

HTML extends the same way (nobody adds a tag; they add `class` and `data-…` to the ones there
are), and so does Pandoc's document model, which has converted between dozens of formats for
fifteen years on a set of blocks about this size.

## Why the alphabet is closed

If anyone could add a role, nothing else would understand it: no type could match it, no view
could show it, no other format could store it. The new role would exist for one extension
only.

What a person wants when they ask for a new role is almost always a new **word**, and words
are open. A slide is not a new letter. It is a `section` with `kind = slide`, a type that says
what a slide has, and a shape that says how it looks. Everything that understands sections
understands it at once.

Changing the alphabet itself is the engine's job, done rarely, and versioned with the
protocol.

## What is open, and what it takes

| To change… | You… | It takes |
| --- | --- | --- |
| how a format's text is read | choose a flavour or a profile | a setting |
| what a thing means | declare a **type** | a few lines of TOML |
| what a thing looks like | declare a **shape** | a few lines of TOML |
| what a value is (an address, a rating) | declare a **value type** | a few lines of TOML |
| which details a format carries | add **attributes** | an owner that emits them |
| which formats take part | install or write an **owner** | a small program, in any language |
| the alphabet | — | a change to the engine |

The first four rows need no programming. That is the accommodation this is for.

## Types over the alphabet

The type language already has two halves, and the split is the right one:

- the **schema**: what a thing is, in fields (`due = "date"`, `status = "text"`). This is
  already neutral.
- **`from`**: how it is read from something written (`on = "list-item"`, then where each field
  comes from). This is where a format's specifics live today.

`on` already names pieces: `note`, `heading`, `list-item`, `paragraph`, `table-row`, `inline`.
Those become the alphabet's roles. What changes is what `from` is allowed to look at:

```toml
# Today: the task is found by matching Markdown's own characters.
[[types.task.from]]
on = "list-item"
match = '^\[[^\n]\]\s+.*'
fields.status = { checkbox = true }
fields.due = [{ inline = "due" }, { marker = "📅" }]

# Proposed: the task is found by what the piece is. Any owner can produce it.
[[types.task.from]]
on = "item"
where.attribute = "checkbox"
fields.status = { attribute = "checkbox" }
fields.due = { attribute = "due" }
```

The emoji and the `[ ]` don't disappear. They move to where they belong: the Markdown owner,
which reads `- [ ] … 📅 2026-11-20` and emits an `item` with `checkbox` and `due`. A LaTeX owner
emits the same piece from `\todo`. The type is written once.

A type may still read text with a pattern (the example workspace's `VEI 6` in prose is one).
That stays: it is a reading of text, which every format has.

## The owner's side

An owner does three things.

**It reads.** Given a file's text, it emits pieces, each with a role, attributes, its text, its
relations, and a **place**: the owner's own name for that spot in the file, which the engine
hands back when it wants a change there. It emits them as a stream, in order, so the built-in
Markdown owner allocates nothing it doesn't today.

**It writes.** Given the file's text and a change, it returns the file's new text. A change is
always in the model's terms, never in a format's:

| Change | Example |
|---|---|
| set an attribute | `checkbox = done` at this place |
| set a field of a typed item | `due = 2026-11-21` |
| replace a piece's text | new runs, marks and links |
| add, move or remove a piece | a new `item` after this place |
| repoint a link | this link now leads there |

The engine then asks the owner to read its own answer back and checks that it says what was
asked, and only then writes. That step exists today and stays.

**It says what it can store.** A table file's cell can't hold a nested list. A plain text file
can't hold a mark. An owner declares which roles, marks, relations and attributes it can write
back. The engine then knows which edits are possible and refuses the others with the reason,
so no owner ever has to mangle something into a format that can't hold it.

### Fragments in another format

Files nest. A board's card holds Markdown. A note's frontmatter is YAML. A fenced block holds
a diagram in its own language.

An owner doesn't have to understand what it merely contains. It hands the fragment over,
naming its format, and that format's owner reads it:

```
canvas owner:   card "a" holds a fragment, format = markdown, text = "- [ ] Buy masks"
markdown owner: an item with checkbox = open, text "Buy masks"
```

Markdown pieces, as owners give them today, are this, for one format. In the general form
nothing is special about Markdown, and an owner that holds no Markdown never has to speak it.

## One task, four files

```
Home.md          - [ ] call the plumber 📅 2026-11-20
Board.canvas     a card whose text is:  - [ ] call the plumber 📅 2026-11-20
Chores.csv       task,due,done
                 call the plumber,2026-11-20,no
House.tex        \todo[due=2026-11-20]{call the plumber}
```

Each owner emits the same thing: an `item` (or a `row`, for the sheet) with
`checkbox = open`, `due = 2026-11-20` and the text "call the plumber". The type `task` matches
all four. The agenda lists four tasks and doesn't know which file each came from.

Ticking one sends its owner `checkbox = done` at its place. Each owner writes that the way its
format does: `[x]`, the same inside the card's text, `yes` in the `done` column, and whatever
the LaTeX package uses. The other bytes of each file stay as they were.

## From here to there

No step needs the ones after it, and Markdown keeps working throughout.

1. **Name the alphabet.** The roles are today's block kinds, renamed where needed. Write the
   list into the gateway's documentation, and version it.
2. **Attributes.** The Markdown owner emits `checkbox`, `level`, `callout` and the task's
   dates as attributes. A type's `from` can test and read an attribute. The built-in `task`
   moves from matching characters to reading attributes, with every id, rev and output
   unchanged.
3. **Owners emit pieces.** An extension's owner can give pieces with roles and attributes, and
   plain text that the engine doesn't read as Markdown. Edits come back in the model's terms.
4. **Fragments.** An owner hands a fragment to another owner by format. Today's Markdown
   pieces become that.
5. **What an owner can store.** Owners declare it; impossible edits are refused with a reason.
6. **A second text format, as the proof.** Something small with real structure (Org, Typst):
   if its tasks, headings and links join the workspace with no change to the engine, the
   boundary is real.

## What it costs

- **The lowest common denominator.** A model every format can speak says only what all of them
  can say. Attributes and `opaque` soften that; they don't remove it. A format's richest
  features will partly live outside the model, visible only through that format's own view.
- **A wrong letter is expensive.** The alphabet is the one closed part, so a mistake there is
  hard to undo. Staying close to a set that has been proven elsewhere lowers the risk.
- **It reaches the centre.** Types are matched against Markdown's structures today. Step 2
  moves that, and it has to be done with ids pinned, the way the tree migration was.
- **An editor can't be neutral.** Showing a note as it is typed needs deep knowledge of its
  syntax. The model makes every format's *content* shared. It doesn't make every format
  pleasant to write in.

## Decisions for you

1. **Closed alphabet, open attributes.** Is that the right place for the line?
2. **The roles.** Is the list of nine right? In particular: is a callout an `aside` with an
   attribute (as proposed), or a role of its own? Is a heading a `section` (a title and its
   body, as proposed), or a piece of its own with the body beside it?
3. **Relations in the alphabet.** Should *placed* and *links to* be letters, so a board is a
   first-class way of writing, or should a board stay "a file of cards", with position kept
   as the board's private detail?
4. **Naming a pattern.** Is a type enough for "I want a slide", or should a person be able to
   name a role of their own (`slide`) as shorthand for a role plus attributes?
5. **How strict is "what an owner can store"?** Refuse an edit the format can't hold
   (proposed), or let the owner store it the best it can and say what was lost?
6. **The proof.** Which second format, and when: before the notebook, or only when someone
   needs it?
