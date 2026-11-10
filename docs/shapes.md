# Shapes and the canvas engine

*(design)*

The frontend is one smart canvas. Everything it shows is a drawing made of **shapes**: letters
and punctuation, lines, arrows, rectangles, tables, cards. The user doesn't need to know; to them
it is a note, a board, a table. Shapes are defined and tagged the way types are, and a user or an
extension can make their own: give it parts, say what data it takes, and what it does.

This is the mirror of types. A type is a *read shape*: it says how a piece of data looks in text,
so the engine can recognise it. A shape is a *draw shape*: it says how a piece of data looks on
the canvas, so the frontend can show it and let you change it. Separated by file type, united by
type data, and now drawn by shape.

## What it is for

- **Data you can see.** A type's rows drawn as a table you can stretch and rotate, still a table
  to the engine, because it stays bound to its rows.
- **Shapes you make.** Two overlapping circles named "simple Venn diagram", whose regions show
  what two collections share and don't. A timeline whose ticks are a type's dates. A kanban lane
  whose cards are tasks with a status.
- **Extensions with both halves.** A type for the data and a shape for how it is drawn and edited,
  shipped together, working in the engine and in the frontend alike.

## Who decides what

- **The user places.** Where a shape sits, its size, rotation and stacking, and what it is bound
  to, are the user's choices and are saved. The engine never moves a shape on its own.
- **The engine derives.** Everything that follows from a placement and the data is computed and
  never saved: a region's outline at the current size, which items fall in it, how their text
  flows, which cells and braille dots that becomes, what is under the mouse.
- **The engine places only where it was asked to:** inside a shape, for computed content (the
  items in a Venn region), and in layouts the user asked for (the graph's force layout).

What is saved is what the user meant; what is computed is how it is drawn, so neither can drift
from the other.

## Shape definitions

Defined like types: in `.cold-lava/shapes.toml` in a workspace, in `~/.config/cold-lava/config.toml`, built
in, and in extensions' manifests (`[[shapes]]`, ids `<extension>.<name>`), with the same origin
order (built in < extension < config < workspace).

```toml
[[shapes]]
id = "venn2"
title = "Simple Venn diagram"
inputs = ["a", "b"]          # collections, bound to queries when the shape is placed
key = "name"                 # what makes two items "the same" across inputs (default: the item)

parts = [
  { circle = { id = "left",  center = [0.38, 0.5], radius = 0.32 } },
  { circle = { id = "right", center = [0.62, 0.5], radius = 0.32 } },
  { text   = { at = "left.top",  value = "${a.title}" } },
  { text   = { at = "right.top", value = "${b.title}" } },
]

regions = [
  { id = "only-a", area = "left - right", shows = "a - b" },
  { id = "both",   area = "left & right", shows = "a & b" },
  { id = "only-b", area = "right - left", shows = "b - a" },
]

[[shapes.on]]                # behaviour: optional, declared, and always an edit the engine checks
drop = { from = "only-a", to = "both" }
edit = { item = "dropped", set-field = { field = "tags", add = "${b.tag}" } }
```

- **Parts** are primitives in the shape's own box (0..1 on each axis): `rect`, `circle`,
  `ellipse`, `line`, `arrow`, `path`, `text`, and other shapes (a shape can contain shapes).
  Anchors (`left.top`, `center`, a named point) let parts and connectors attach.
- **Inputs** are collections a placed shape is bound to: an `items/query` (a type, fields,
  tags, a note, a folder), a literal list, or another shape's output.
- **Regions** are areas computed from parts with `&` (both), `|` (either), `-` (but not), and
  what each one **shows**: an expression over the inputs. It is a small language, not a
  programming language: set operations (`&`, `|`, `-`), filters
  (`a where status = "done"`), sorting and limits, and fields of the items. Region content is
  laid out by the engine (a list, a grid, a table), and what doesn't fit says "+3 more".
- **Behaviour** (`on`) maps a gesture (a drop, a click, a cell edit, a key) to an edit on the
  workspace gateway (`set-field`, `toggle`, a recipe). Edits go through the engine's checks and
  are attributed, so a drawing can ask for changes but never corrupt a note.
- **Beyond the language,** a shape can name an extension that computes a region's content or
  handles a gesture, in any language over the gateway, the way extensions extend types and
  recipes today.
- **Built in:** `text` (a block of Markdown, laid out as glyphs), `rect`, `circle`, `line`,
  `arrow`, `card` (a note), `sticky` (text of the drawing's own), `table` (a type's rows, cells
  editable through `set-field`), `venn2`, and the glyph set. They are written in this same
  language, in a file the engine ships, and nothing in the code knows one of them by name.

### Card, sticky, and "make it a note"

- **`card`** is a placed shape bound to a note (a JSON Canvas `file` node): it shows the note's
  title and first lines, and opening it opens the note. It is a link, so the note gets a backlink
  from the drawing and the graph shows it.
- **`sticky`** is text that lives in the drawing's file itself (a JSON Canvas `text` node), with no
  file of its own. Its Markdown still joins the workspace through the drawing's owner: a task on a
  sticky is a `task`, `[[Lava]]` on it a backlink.
- **"Make it a note"** is an action any shape can declare in `on`, made of generic gateway steps:
  create a note with the shape's text where Obsidian puts new notes, then rebind this shape to it
  as a card, same place and size. Obsidian's canvas has the same move ("Convert to file"). The
  built-in `sticky` declares it, so the format shows it can say it:

  ```toml
  [[shapes.on]]
  action = "make-a-note"
  title = "Make it a note"
  do = [
    { create = { text = "${text}" } },
    { rebind = { shape = "card", inputs = { note = "created" } } },
  ]
  ```

### Shape definitions, as built

The format and the loader are in the engine (`crates/engine/src/shapes.rs`), beside types: the
engine already owns definitions, their origins, the order they load in and the way a broken one is
reported, and it reads TOML; the canvas crate stays pure geometry with no dependencies. The
loader builds each definition into a `cold_lava_canvas::Shape`, and the canvas crate draws it.

- **Where from.** `[[shapes]]` and `[glyphs]` in the built-in file
  (`crates/engine/shapes/built-in.toml`), then each extension's manifest (`[[shapes]]`, ids
  `<extension>.<name>`, left out with a line in `cold-lava keys` when outside the namespace or
  given twice, as types are), then `config.toml`, then the workspace's `.cold-lava/shapes.toml`
  (`version = 1` first, as `types.toml`). A later definition of an id or a glyph replaces the
  earlier one, goes last, and says so in a note.
- **Parts**, each one key: `rect` (`from`, `to`: two corners), `circle` (`center`, `radius`),
  `ellipse` (`center`, `radii`), `line` and `arrow` (`from`, `to`: a point, or an anchor of a
  part before it, like `frame.left`), `path` (lines and curves, `closed`: "Paths" below), `text` (`value`, and either
  `at` a point or anchor, a label, or `from` and `to`, a box it wraps in), and `shape` (`id`,
  `use` another shape's id, `from`, `to`: its box). Each can have an `id`.
- **Text at an anchor** is a `label`: it sits on the side its anchor faces (the row above
  `left.top`, below a `bottom`, beside `left` and `right`), or the `side` given (`above`,
  `below`, `left`, `right`, `center`), and labels never overlap ("Fitting" below has the rule).
- **Nesting.** A nested shape's parts are copied into its box and named `<id>.<part>` (a circle
  in a box that isn't square becomes an ellipse); its box's anchors are `<id>`, `<id>.top`, …,
  so `past.mark.bottom` is the bottom of the part `mark` of the nested `past`. Its regions stay
  its own: name its parts in a region of yours (`l.circle`).
- **`anchors`** names points of the shape's own (`tip = [0.9, 0.5]`); **`aspect`** is its width
  over height in world units, used when it is first placed and by `--render`.
- **Placeholders.** `${a.title}` in text names an input and its field; a binding fills them
  ("Bindings, as built"), and an unbound shape shows them as `a.title`. **Glyphs**: `:check:` in text is the
  glyph's symbol, one character that draws like a letter; an unknown `:name:` stays as written.
- **`on`.** Each entry is one gesture or action (`drop = { from, to }` between regions,
  `click` on a region or part, `key`, or a named `action` with a `title`) and its steps: `do = [
  … ]`, or one `edit` alone, or `extension = true` (an extension's shape only: its extension
  decides). Steps are `edit` (`item`, and one of `set-field` with one of `value`, `add`,
  `remove`; `toggle`; or `recipe` with its `options` and no `item`), `create` (a note: `text`,
  `name`), and `rebind` (`shape`, `inputs`). They run as "Behaviour, as built" says. A region
  can say `extension = true` instead of `shows`, for its extension to compute.
- **Checked at load,** one line each, naming the file and the shape: a definition that doesn't
  read (an unknown part kind or field, a segment with no `to`), no id, no parts, a path with
  both `points` and `segments`, segments with no `start`, fewer than two points or over the size
  limit ("Paths" below), a bad area (an unknown or stroke part,
  a stray operator), an anchor no earlier part has, two parts or anchors with one id, a
  placeholder that isn't an input, a `shows` that doesn't read or names something not an input
  ("The region language" below; its sets go through the area parser with the inputs as names), a
  gesture count other than one, a `drop` between regions that don't exist, an edit with no or
  two kinds, a `rebind` or a nested shape naming a shape that didn't load, and a cycle of nested
  shapes. The shape is dropped; its siblings, and shapes in other files, load.
- **`cold-lava shapes [WORKSPACE]`** lists every shape with its origin, inputs, parts, regions
  and behaviour, then the glyphs; notes and errors go to stderr, and errors exit 1. `--json` is
  the loaded set as data. `--render ID [--size WxH] [--rotate DEG]` draws one, unbound, in a
  WxH terminal (default 60×18), turned DEG degrees clockwise: braille outlines, text and labels,
  then its regions' tints by number.

```console
$ cold-lava shapes example-workspace --render venn2 --size 40x12

              a.title  b.title
                ⣀⣀⣀  ⣀⣀⣀
             ⡠⠒⠉ ⢀⠔⠋⠙⠢⡀ ⠉⠒⢄
           ⢀⠎   ⢠⠃    ⠘⡄   ⠱⡀
           ⡜    ⡇      ⢸    ⢣
           ⢣    ⡇      ⢸    ⡜
           ⠈⢆   ⠘⡄    ⢠⠃   ⡰⠁
             ⠑⠤⣀ ⠈⠢⣄⣠⠔⠁ ⣀⠤⠊
                ⠉⠉⠉  ⠉⠉⠉
```

#### Paths

A `path` is straight lines between `points`, or a `start` and `segments` that go on from it, each
a `line`, a cubic Bézier `curve` (two control points) or a quadratic `quad` (one, turned into the
cubic it equals). `closed = true` joins the end back to the start and makes it an area, so a
region can name it; an open path is a stroke.

```toml
parts = [
  { path = { id = "zigzag", points = [[0, 0], [0.5, 1], [1, 0]] } },
  { path = { id = "bubble", closed = true, start = [0.12, 0.02], segments = [
    { line = [0.88, 0.02] },
    { curve = { c1 = [0.935, 0.02], c2 = [0.98, 0.11], to = [0.98, 0.22] } },
    { quad = { c = [0.32, 0.86], to = [0.16, 0.98], id = "tip" } },
  ] } },
]
```

- **Anchors.** A path has `start` and `end` (`zigzag.end`), and a segment can name the vertex it
  ends at with an `id`, as parts are named: `{ line = { to = [x, y], id = "tip" } }` (a line
  given just as `[x, y]` has none). The vertex is the anchor `<path>.<id>` (`bubble.tip`), so the
  path needs an id too; `start` and `end` are taken. Later parts can start at it, and a nested
  shape's are `<nest>.<path>.<id>`.
- **Size.** A path may have at most 2048 points once its curves are flattened: each of `points`,
  the `start` and each `line` is one, each `curve` and `quad` is 24 (the canvas draws a curve as
  24 short lines). A path over it is refused at load, wherever it comes from (an extension's
  manifest too), with its size: simplify it or split it. The number comes from the canvas bench
  (`cargo run --release -p cold-lava-canvas --example bench`): its full-screen scene, 200
  shapes on a 200×60 terminal, takes about 11 ms; with its 50 blobs swapped for paths of 2048
  points it took 16 ms closed and 12 ms open, within a 60 Hz frame (16.7 ms), and with 4096
  points 18 and 19 ms, over it. To get there, a closed path's row bands (which edges a point
  inside it is tested against) went from at most 64 to at most 1024: with 64, one path of
  1024 points filling the screen took 13.5 ms against a Venn's 1.6 ms, and 7 ms with 1024.
  Open paths cost more per point (each line is walked), and they set the limit.
- **No formulas.** A spiral, a wave or a gear is computed by whoever ships it, as points or
  curves, and written as a path ([extensions-guide.md](extensions-guide.md#curves-and-spirals)).

The example workspace's `speech` is a speech bubble drawn this way, with its inside as a region:

```console
$ cold-lava shapes example-workspace --render speech --size 60x18 --bind 'text=text:Krakatoa erupted again this morning'

    ⡠⠔⠊⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠑⠢⢄
  ⢀⠎                                                    ⠱⡀
  ⡜                                                      ⢣
  ⡇                                                      ⢸
  ⡇                                                      ⢸
  ⡇   Krakatoa erupted again this morning                ⢸
  ⡇                                                      ⢸
  ⢇                                                      ⡸
  ⠘⡄                                                    ⢠⠃
   ⠈⠢⢄⡀                                              ⢀⡠⠔⠁
      ⠈⠉⠉⠉⠉⠉⠉⠉⠉⠉⡆     ⡠⠒⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠁
               ⡜  ⢀⡠⠒⠉
             ⡠⢊⡠⠔⠊⠁
           ⠤⠚⠉⠁
```

It is an example rather than a built-in: the built-ins are the primitives and the kinds of node a
JSON Canvas file has, and a bubble is neither, so it stays one workspace's. The shape editor
("The shape editor, as built") draws one like it: vertices clicked into place, sides bent into
curves.

### Bindings, as built

A placed shape's inputs are bound to data, and the engine says what it shows: `bind.rs` and
`shows.rs` in the engine, data and no UI. A frontend makes a `Binding` for a placed shape,
hands it the core's item news and events, and asks it what to draw.

- **Sources.** Each input is bound to one `Source`, saved with the placed shape as the user
  chose it:
  - a **query**: an `items/query` selection (`kind` and the gateway's `filter`: `tag`,
    `folder`, `note`, `status`, `property`, `text`, `date`), an optional `where` (a condition
    in the region language below, for what the gateway's filter can't say: `vei >= 6`), and
    an optional `title`;
  - a **note** (`Volcanoes/Krakatoa.md`): one member with its `title` and `name` (the file's
    stem), `path`, `tags` and `excerpt` (its first three lines of text, after the frontmatter
    and a first heading or a first line that is its name). This is a card's input;
  - **items**: labels written out, each a member whose `name` is the label;
  - **text** of the drawing's own: a sticky's.

  On the wire (and in `--bind` as JSON) a source is `{"query": {"kind": "volcano", "filter":
  {"tag": "eruption"}, "title": "Erupted"}}`, `{"note": "…"}`, `{"items": ["…"]}` or
  `{"text": "…"}`. Another shape's output as a source waits for placed shapes (#136), since it
  names a placed shape.
- **Live.** A query input is subscribed (`items/subscribe`) as the owner `canvas`; the frontend
  hands `canvas`'s news to its bindings (`Binding::heard`), and the input is queried again. A
  note input is read again on its `NoteChanged`, `NoteCreated` or `NoteRemoved`, and every input
  after `Rescanned` (`Binding::noticed`). `close` ends the subscriptions.
- **Members.** What an input holds: an item (its id, its `rev`, its fields; a list item's or a
  heading's `text`; its `note`), a note, or a label. A member's **label**, unless the region
  says otherwise, is its `name`, else its `title`, else its `text`, else its first text field,
  else its id.
- **Key.** The shape's `key` says what makes members of two inputs the same: the field's value
  folded with `search_key` (case, accents and ʻokina), so `kilauea` matches `Kīlauea`. Without
  a key, or for a member with no value for it, a member is only the same as itself (its id).
- **Placeholders.** `${a}` is the input's text: a query's or note's title, the labels joined
  with ", ", or the text. `${a.title}` is its **title**: a query's `title`, else the query read
  out (`volcano #eruption in Volcanoes`); a note's name; a list's input name; text's first line.
  `${a.count}` is how many members it has; a query's `${a.type}`, `${a.tag}`, `${a.folder}`
  and `${a.note}` are its selection's; any other field is its one member's (`${note.excerpt}`),
  and empty when it has more or none.
- **What it shows.** `Binding::shown` is each region in order with its members' ids and labels
  (what its `shows` picks), and each text and label part filled in; `Binding::canvas` is the
  shape with its text filled, to draw. `cold-lava shapes WORKSPACE --render ID --bind
  INPUT=SOURCE…` draws it with each region's labels fitted inside it (a centred column, "+N
  more" when they don't fit); with `--json` it prints `shown`. `--bind` takes a query as words
  (`type=volcano tag=eruption title="Erupted" where vei >= 6`), `note:PATH`, `items:A, B`,
  `text:…`, or JSON.

#### The region language

A region's `shows` is read when the shape loads (a mistake drops the shape with one line naming
it) and evaluated each time `shown` is asked, so a changed input shows at once. It is a small
language, not a programming language (`shows.rs`):

```text
shows   = sets [ "where" cond ] [ "sort" "by" FIELD [ "asc" | "desc" ] ] [ "limit" N ]
          [ "label" FIELD ]
sets    = input names with & | - and ( ), read as an area is over parts
cond    = all { "or" all }
all     = one { "and" one }
one     = "not" one | "(" cond ")" | FIELD [ op value ]
op      = "=" | "!=" | "<" | "<=" | ">" | ">="
value   = "quoted text" | a word: 6, 2.5, 2026-11-10, ring-of-fire, #strato
```

- **Sets.** `&` binds tighter than `|` and `-`, which go left to right, as in areas. Members
  are matched on the shape's `key`: `a & b` is each member of `a` whose key `b` has too, `a - b`
  each one it doesn't, `a | b` the members of `a`, then those of `b` it doesn't have. A member
  keeps its input's order, and comes from the left side.
- **`where`** keeps the members the condition holds for. `FIELD` alone holds when the field has
  a value. A comparison reads its value as a number when both sides are numbers, as a date when
  both are `YYYY-MM-DD`, else as text folded with `search_key` (and a tag's `#` aside), so
  `name = paricutin` finds Parícutin. On a list field (`tags`), `=` and the orderings hold when
  one of its values does, and `!=` when none is equal. A member with no value for the field
  never meets a comparison, `!=` included. `and` binds tighter than `or`; `not` and brackets
  as usual.
- **`sort by FIELD [desc]`** orders them, stably: numbers before text, a list by its first
  value, members without the field last either way.
- **`limit N`** keeps the first `N`.
- **`label FIELD`** is what the region calls each member; its default label without a value.
- The clauses come in that order, each at most once. The words `where`, `sort`, `limit`,
  `label`, `and`, `or`, `not`, `by`, `asc` and `desc` are the language's, so a field can't be
  called one.

The example workspace's `tallest` shape lists the five tallest of what it is bound to:
`shows = "volcanoes where elevation sort by elevation desc limit 5"`.

```console
$ cold-lava shapes example-workspace --render venn2 --size 80x24 \
    --bind a='type=volcano tag=eruption title=Erupted' \
    --bind b='type=volcano tag=volcano/strato title=Stratovolcanoes'
                               Erupted  Stratovolcanoes

                            ⡠⠤⠒⠊⠉⠉⠉⠉⠉⠉⢒⡢⢔⡒⠉⠉⠉⠉⠉⠉⠑⠒⠤⢄
                         ⡠⠔⠉       ⢀⠤⠊⠁  ⠈⠑⠤⡀       ⠉⠢⢄
                       ⢠⠊         ⡔⠁        ⠈⢢         ⠑⡄
                      ⡰⠁        ⢀⠎            ⠱⡀        ⠈⢆
                     ⢰⠁         ⡎              ⢱         ⠈⡆
                     ⡇         ⢸                ⡇         ⢸
                    ⢸          ⡜Eyjafjallajökull⢣          ⡇
                    ⢸Krakatoa  ⢣Mount St. Helens⡜ Mount Fu…⡇
                     ⡇         ⢸    Vesuvius    ⡇         ⢸
                     ⠸⡀         ⢇              ⡸         ⢀⠇
                      ⠱⡀        ⠈⢆            ⡰⠁        ⢀⠎
                       ⠘⢄         ⠣⡀        ⢀⠜         ⡠⠃
                         ⠑⠢⣀       ⠈⠒⢄⡀  ⢀⡠⠒⠁       ⣀⠔⠊
                            ⠑⠒⠤⢄⣀⣀⣀⣀⣀⣀⠬⠕⠪⠥⣀⣀⣀⣀⣀⣀⡠⠤⠒⠊
```

The built-in `card`'s region `note` has no `shows`: the card's text already shows its note, and
the region is there to be clicked.

#### Behaviour, as built

A placed shape does what its `on` says, through the workspace gateway: `act.rs` in the engine.
Placed shapes on screen are #136, so a gesture arrives as data, and `cold-lava shapes --do`
sends one from the command line.

- **Gestures.** `{"gesture": "drop", "from": REGION, "to": REGION, "item": ID}`,
  `{"gesture": "click", "on": REGION-OR-PART, "item": ID?}`, `{"gesture": "key", "key": K}`
  and `{"gesture": "action", "name": NAME}`; the item is a member's id, as `shown` gives it.
  `Binding::act` finds the `on` entry the gesture starts, refused (`refused`) when there is
  none, and runs its steps in order, stopping at the first refused one.
- **Placeholders in steps** are the inputs' (`${b.tag}`), the gesture's member's (`${dropped}`,
  its label; `${dropped.name}`, `${clicked.visited}`), and the note an earlier `create` made
  (`${created.path}`, `${created.title}`).
- **`edit`** names its item: `dropped` or `clicked` (the gesture's member), or an input with
  one member. `set-field` sends `items/edit set-field` at the member's `rev`, so an item changed
  since it was shown is refused (`changed`); `value` is text the field's type reads, and `add` or
  `remove` change a list field's values (once, compared folded), refused on a field that holds
  one value. `toggle = true` ticks a task as the Tasks plugin does. `recipe` names a recipe and
  its `options` (`{ recipe = "rename-tag", options = { from = "${a.tag}", to = "…" } }`),
  planned and applied at once; one that would change a file another owner reads is refused,
  and runs from its own screen instead. Every edit goes through the engine's checks, so a
  drawing can ask for a change but never corrupt a note.
- **`create`** makes a note with its `text` where Obsidian puts new notes (`.obsidian/app.json`),
  named `name` or the text's first line, numbered when the name is taken (`workspace/create`).
- **`rebind`** says what this placed shape becomes: `shape`, and its inputs, each `created`
  (the note just made), the name of one of this shape's inputs (bound the same way), or text.
  The frontend keeps the placement, so it is the same place and size, and calls
  `Binding::rebind`. This is "make it a note": the built-in `sticky` creates a note with its
  text and becomes a `card` bound to it.
- **Attribution.** The steps run through a `Gateway`: for the user's shapes (built in,
  config.toml, the workspace's), the core itself (`Direct`), edits attributed to `canvas`; for
  an extension's shape, that extension's `Host`, with its manifest's permissions (`items.edit`
  for an edit; `files.write` for `create` and a recipe) and its name on the edits
  (`ItemEdited`'s `by`). A step without its permission is refused (`forbidden`) and nothing
  after it runs.
- **The extension's half.** A region with `extension = true` shows what the extension that
  ships the shape answers to `shape/region` (asked after binding and whenever an input
  changes, `Binding::region_asks`); a gesture whose `on` entry says `extension = true` asks it
  `shape/gesture`, and the steps it answers run as above, through its own permissions. Only an
  extension's shapes may say so; the messages are in docs/extensions.md ("Shapes, as built").

```console
$ cold-lava shapes WORKSPACE --render sticky --size 40x10 --do action:make-a-note \
    --bind 'text=text:Ash masks
Two per person, for the Krakatoa trip.'
created Ash masks.md
now a card

 ⡖⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⢲
 ⡇ Ash masks                          ⢸
 ⡇                                    ⢸
 ⡇                                    ⢸
 ⡏⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⢹⠁
 ⡇ Two per person, for the Krakatoa   ⢸
 ⡇ trip.                              ⢸
 ⠧⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠼
```

The built-in `venn2`'s drop (`only-a` to `both` adds `${b.tag}` to the item's `tags`) needs a
`tags` field set-field can write. The example workspace's volcanoes read their tags from the
text, which can't be written through a field yet, so there the drop is refused with that reason
and nothing is written.

The example workspace has a three-circle Venn, a `tallest` list ("The region language"), a
timeline that nests a `tick` three times, and a glyph of its own (`:eruption:`) in
`.cold-lava/shapes.toml`. `venn2` is built in rather than the example's, as the docs' example and
the showcase of step 5: a placed Venn works in every workspace.

### Glyphs are shapes too

Letters, punctuation and symbols are shapes from a **glyph set**, the way a font is a set of
glyphs. A paragraph is one `text` shape that lays out and draws its glyphs, so a page of text is
not thousands of objects; but the glyph set can be extended with icons and symbols that draw
like letters anywhere text goes.

## Placed shapes and files

- **Text stays Markdown.** Characters in the flow of a note are the note, as now.
- **A floating drawing gets its own file,** and the note holds only a link to it
  (`![[Ring of Fire.canvas]]`), so any other tool sees a plain link, and the document view draws
  the drawing in place.
- **The file is JSON Canvas** (Obsidian's `.canvas`), each placed shape a node with extra fields
  (shape id, inputs, transform), so Obsidian opens it as a canvas. Obsidian 1.15 keeps fields
  it doesn't know when it saves a canvas, at node, edge and top level (#132), but sorts the
  nodes and rounds `x`, `y`, `width` and `height` to integers.
- **The drawing's file joins the workspace** through its owner: text on a shape is Markdown,
  so a task written on a sticky note is a `task`, and a link on a card is a backlink. This is
  built: "Drawings join the workspace, as built" below.

### Placed shapes, as built

Placed shapes live on the whiteboard (`board.rs`), in the `.canvas` file it already reads,
writes and merges. A placed shape is a node with a `cold-lava` field (`board/placed.rs`); the
file has `"cold-lava-version": 1` at its top level while it has one.

```json
{
  "id": "venn", "type": "text", "text": "Simple Venn diagram",
  "x": 0, "y": 0, "width": 495, "height": 489,
  "cold-lava": {
    "shape": "venn2",
    "inputs": {
      "a": {"query": {"kind": "volcano", "filter": {"tag": "eruption"}, "title": "Erupted"}},
      "b": {"query": {"kind": "volcano", "filter": {"tag": "volcano/strato"}}}
    },
    "rotation": 105.0,
    "exact": [0.0, 0.0, 495.4, 489.2]
  }
}
```

- **The field** holds what the user placed and nothing derived: `shape` (its id), `inputs`
  (each input's `Source`, as `--bind` takes it in JSON), `node` (the input the node itself holds,
  below), `rotation` (degrees clockwise about the centre) and `exact` (the box in canvas pixels,
  only when a side isn't a whole number). Members, region contents, outlines and cells are
  computed and never saved.
- **The node is the placement.** Its own `x`, `y`, `width` and `height` are the shape's box, so
  Obsidian shows the node where the shape is and moving it in Obsidian moves the shape. `exact`
  counts only while it rounds to the node's box: after Obsidian moves the node, the node wins.
  A canvas pixel is a tenth of the canvas engine's world unit (a terminal cell's width at zoom 1).
- **The node's type** is chosen so Obsidian shows something that means the same:

  | The shape has | Node | Obsidian shows | `node` |
  | --- | --- | --- | --- |
  | an input bound to a note (a card) | `file`, `file` = the note | the note | that input |
  | else an input bound to text (a sticky) | `text`, `text` = the text | the text, editable | that input |
  | neither (a Venn, an arrow) | `text`, `text` = the shape's title | the title | none |

  The input in `node` isn't repeated in `inputs`: the note is the node's `file` (so a rename
  moves it, here and in Obsidian) and a sticky's text is the node's `text` (so editing it in
  Obsidian edits the sticky). The rule looks only at the sources, not at the shape's id, so it
  holds for any shape.
- **Order doesn't matter.** Nodes are found by id, so Obsidian sorting them changes nothing;
  stacking follows the file's order, as for every card.
- **Edits on both sides merge** as the whiteboard's always have (`merge3`: nodes by id, field by
  field; the `cold-lava` field is one value). Unknown fields are kept.
- **The guard.** When the file changes outside and a node that had the field still exists
  without it, a program saved the canvas without cold-lava's fields. The whiteboard puts them
  back from its own copy, keeps the outside change to everything else, saves, and says so in the
  status line until a key is pressed, instead of the shape silently becoming a plain card.
  A node deleted outside is a deletion, as for any card.

**On the whiteboard.** Each placed shape is bound while the board is open (`board/live.rs`):
the board runs in the menu's loop, which hands it the core, query inputs are subscribed as
`canvas` and the menu passes their news to the bindings, core events reach `Binding::noticed`
through the menu too, and the extension that ships a shape is
asked `shape/region` for the regions it computes and answers through
`Core::take_shape_answers`. The canvas engine paints each shape into a grid (braille outlines,
region tints, text, labels, and what each region shows), cached per shape until its placement,
the zoom, the view's sub-cell offset or what it shows changes, so panning by whole cells
repaints nothing. A click selects a shape where its parts, regions or box are
(`cold_lava_canvas::hit`); a selected shape shows its four corner grips (`◆`, drag to stretch in
its own turned frame) and its turn grip (`↻`, drag to turn in 15° steps; `r` turns 15°).
Dragging it moves it. Kitty pixels aren't used for shapes yet; braille is.

**Placing and using them** (the whiteboard's keys and mouse):

- **`s` places a shape**: a palette of every loaded shape (built in, the workspace's, config's
  and extensions', each with its origin), then a form with a line per input, each a source in
  the short form `--bind` takes (`type=volcano tag=eruption title="Erupted" where vei >= 6`,
  `note:Volcanoes/Krakatoa`, `items:Fuji, Krakatoa`, `text:…`; empty leaves it unbound). It is
  bound before it is placed, so a mistake (`no item kind or type called vulcano`) shows in the
  form. The shape goes beside the selection, else right of everything, at about 1200 square
  world units in its own `aspect`. **`b`** opens the same form on the selected shape, filled in
  with its sources (`Source::short`).
- **A click** runs the `click` its shape declares for what is under it: the region member (its
  label, as the region painted it), else the region, else the part. The workspace's
  `checklist` ticks a task this way, through `items/edit toggle`, attributed to `canvas`.
- **Dragging a member** out of a region a `drop` starts from, into another region of the same
  shape, is that drop; dragging anything else moves the shape.
- **A key** that the selected shape declares in `on` is its gesture, before the whiteboard's
  own keys.
- **`x`** lists the selected shape's actions by title; Enter runs one. The sticky's "Make it a
  note" creates the note where Obsidian puts new notes and turns the node into a card on it,
  same place and size: the step's `rebind` becomes the node's new shape and sources, and the
  live binding follows the node.
- **Enter** (or a double-click) on a shape opens what it holds: a sticky's text is edited in
  place on the node (Esc saves, merging with outside changes as a card's text does); a card
  opens its note in the menu, and Back returns to the whiteboard; any other shape opens `b`'s
  form.
- Each step goes through the gateway as "Behaviour, as built" says: the core for the user's
  shapes, the shipping extension's `Host` (its permissions, its name on the edits) for an
  extension's, whose `extension = true` gestures are asked `shape/gesture` and run when it
  answers. A refused step says why in the status line and writes nothing; the built-in Venn's
  drop is refused this way in the example workspace.

### Drawings join the workspace, as built

A `.canvas` file is read by the engine's canvas owner (`crates/engine/src/owner/canvas.rs`;
docs/extensions.md, "The canvas owner, as built"), so what is drawn is data like what is
written in a note. The node mapping above is what makes it work:

- **A sticky's text is its node's `text`**, a Markdown piece at the node's id. A task on it is
  a `task`: the agenda, the type table and `items/query` list it beside the notes' tasks, and
  ticking it there rewrites that node's `text` in the `.canvas` and nothing else. Its
  `[[links]]` are backlinks and its tags are tags. Any text card counts the same way, and so
  does the title a Venn or an arrow keeps in its `text`.
- **A card's note is its node's `file`**, a link from the drawing at the node's id. The note's
  backlinks list the drawing (Enter on that row opens the whiteboard), the graph draws the
  canvas as a node with an edge to the note, and tidy doesn't call the note an orphan. "Make it
  a note" therefore gives the new note a backlink from the drawing at once.
- **Renaming the note** rewrites the card's `file` and the links in text nodes in place, in the
  style each was written.
- **Not links:** a web page card (`link` node), a group's label, and edges: an arrow joins two
  nodes of the drawing, not two files.

Why the whiteboard and not a second canvas view: the whiteboard is already the one place that
reads, writes, merges and watches `.canvas` files. Shapes on it are nodes among cards, edges and
groups, sharing its undo, selection, camera and Obsidian-safe saving. A separate scene view for
shapes would have been a second editor of the same files; with one, #92 moved one screen, cards
and shapes together, into the menu's loop.

### The shape editor, as built

The shape editor makes a shape by drawing it and writes the same `[[shapes]]` TOML people write
by hand into the workspace's `.cold-lava/shapes.toml`. There is no other format: what it shows
is the definition it will save, the file stays editable by hand, and an extension could ship the
result as it is. `S` on the whiteboard opens it:

- with nothing selected, on a new shape or one of the workspace's own (`edit venn3`; only those
  are in the file it writes);
- with a placed shape selected, on a copy of its definition (`venn2-copy`), to make your own;
- with a group selected, on one new shape of every placed shape in the group (`drawing1`), each
  part where it was: the group's box becomes the shape's box and `aspect`, curves stay curves
  (an affine map keeps a Bézier one), a turned rectangle becomes a closed path of its corners, a
  turned ellipse four curves, ends and labels at anchors go where the anchors were, and regions
  keep their areas. Inputs, what regions show and behaviour are each shape's own and are left
  out, with a line saying so. The whiteboard has no freehand strokes (JSON Canvas has none), so
  placed shapes are what is drawn freely there.

**Where it lives, and why.** It is a screen of the core, in the binary
(`crates/cold-lava/src/shape_editor.rs`), opened by the whiteboard, which lends it the core.
An extension can't be one: extensions never see keystrokes or draw, and the view gateway carries
lists and tables, not a canvas with handles under the mouse. Nothing in it knows one shape from
another, though, and it is split so the parts a second frontend needs carry no drawing:

- the shape being edited is its `Definition` and nothing else; `shape_editor/draft.rs` is the
  editing on it (where each part's grips are and what dragging one does, making parts, naming
  regions, anchors and parts, a selection made into one shape), with no ratatui;
- the guides are geometry, in the canvas crate (`snap.rs`);
- writing one block back into the file as text is the engine's (`shapes/file.rs`), beside the
  loader, and goes through its safe writes; `shapes::with_draft` loads the draft as the
  workspace's shape, so it is checked, drawn and bound exactly as a saved one is.

**Drawing.** The shape's box is drawn as large as fits in its `aspect` (a new shape is square),
its parts painted by the canvas engine and its grips on top: `●` a vertex, `◦` the middle of a
side, `+` a centre, `◆` a radius, a corner or an end.

| Key | Tool or command |
| --- | --- |
| `v` | grab: drag a grip, a side's middle to bend it, a part to move it; a click selects a part |
| `p` | vertices: a click places each; a click on the first closes the path, `Enter` leaves it open |
| `c` / `e` / `r` | a circle (drag from its centre out) / an ellipse (centre to a corner) / a rectangle |
| `t` | a label: click an anchor (a part's `top`, `left`, …, or a named point) or a point, then type it |
| `a` | a region: click inside an area, then name it |
| `n` | an anchor: click a vertex or a point, then name it |
| `N` / `d` | rename the selected part (and what names it) / delete it |
| `i` / `w` / `o` / `b` | the id, title, inputs, key and aspect / what each region shows / a behaviour / preview data |
| `g` / `u` / `Ctrl+s` / `q` | guides on or off / undo / save / leave (twice with unsaved changes) |

- **Vertices are the handles.** A path is written as a `start` and `segments` (one written as
  `points` becomes that when it is first changed). Moving a vertex takes the control points of a
  curve next to it along, so a bent side keeps its bend.
- **Sides bend.** Dragging a side's middle makes it a cubic `curve` whose middle passes through
  the pointer (the quadratic through it, raised to the cubic it equals); dragging it back within
  a cell of the straight middle makes it a `line` again. A closed path's closing side is written
  out as a segment ending on the start when it bends, and dropped again when it is straight.
- **Circles, ellipses and rectangles** are their own tools, not curves point by point. In a box
  that isn't square, the circle tool writes the `ellipse` that looks round.
- **Guides, never data.** Placing or dragging a point asks `snap::vertex`, in world units (a
  cell is 1×2, so angles are the ones the eye sees), in order: onto a point already there (the
  first vertex closes the path), the next corner of a regular polygon (the last side again,
  turned as the last corner was), the side's angle to a multiple of 15° (90°, 45°, 60°, 30°)
  and, within a cell, the previous side's length, then level or plumb with another point. A
  circle's radius snaps to another circle's (`snap::size`). The status line says which guide
  placed the point (`⊹ 60° · = side`, `⊹ regular`); a grip is dragged by where it was grabbed,
  so a straight drag stays straight. Only where points end up is saved (to four places).
- **Regions by clicking.** `draft::area_at` asks each area part whether it contains the clicked
  point (the canvas engine's exact tests): the ones that do are joined with `&`, and the ones
  that don't, where their boxes could overlap, are taken away with `-` (`left & right`,
  `right - left`, `left & right - low`). The form says which parts the point is in and asks
  only for the name. Naming an area that already has a region renames it.
- **Anchors by clicking.** A vertex a segment ends at gets the segment's `id` (the anchor
  `<path>.<id>`); a path's start, or any other point (snapped to a grip), goes in `anchors`.
- **Data by form.** `i`: the id, `title`, `inputs` (`a, b`), `key` and `aspect`. `w`: per
  region, its sets (`a & b`), `where`, `sort by` and `limit`, put together as `shows` and read
  by the region language before they are kept. `o`: a gesture (`click`, `drop`, `key`,
  `action`, picked with ← →), what it is on, and one step (`toggle`, `set-field` with `value`,
  `add` or `remove`, `create a note`, `rebind`); one the loader would refuse is not added, and
  the form says why. Every mistake the loader finds in the draft is listed in the panel, worded
  as `cold-lava shapes` words it, and saving waits until there are none.
- **The preview** on the right is the draft bound to data as a placed shape is (`Binding`, the
  core's queries followed live): `b` takes each input as `s` does on the whiteboard
  (`type=volcano tag=eruption`, `note:…`, `items:…`, `text:…`), and each region fits what it
  shows.
- **Saving** (`shapes::file::save`) writes the shape as a block in the file's usual form (`parts`
  and `regions` one to a line, a path's `segments` one to a line, `[[shapes.on]]` tables), in
  place of the block it was read from, or appended to the file (made with `version = 1` when
  there is none). A block is its `[[shapes]]` line up to the next table that isn't its own
  (`[[shapes.on]]` is), less the comments and blank lines just before that table, which are the
  next one's; table headers are found outside strings and values. Everything outside the block
  stays byte for byte, comments included; comments inside a rewritten block are not kept.
  Before writing, the new text is read back: every other shape, the glyphs and the version must
  read as before, and the shape as its definition, or nothing is written. The write is
  `write::replace` against the bytes the editor read, so a file changed elsewhere since (by hand,
  or by Obsidian syncing it) is refused, with the shape still in the editor. An id another shape
  in the file has is refused too. After a save, the whiteboard reads its shapes again, so `s`
  places the new one.

The drive scripts `tests/drive/shape-hexagon.drive` and `shape-venn.drive` make a regular
hexagon with a curved side and a two-circle Venn this way, without typing TOML;
`shape-edit.drive` edits `venn3` and checks that only its block changed, and
`shape-from-drawing.drive` makes a shape of a group.

## The canvas engine

A headless crate (like `cold-lava-editor`: nothing drawn), shared by every frontend. It is
`cold-lava-canvas` in `crates/canvas`, with no dependencies, built up in the order below.

- **Geometry** *(as built)*: parts in a shape's unit box (`part.rs`: `rect`, `circle`,
  `ellipse`, `line`, `arrow`, `path` as a polyline or cubic Béziers flattened once (24 lines a
  curve; its rows banded, up to 1024 bands, so testing a point looks at few edges), `text` as the
  box its text will fill, `label` as one line of text beside a point); a `Placement` (what is saved: top-left corner, size, rotation in
  degrees clockwise about the centre) composed into one affine `Transform`; anchors
  (`left.top`, a part's id for its centre, the box's own `center`, `top-left`, …, `start`
  and `end` of strokes, a path's named vertices, and named points of the shape's own; a part's id may hold dots, as a
  nested shape's parts do: `card.frame.top`). A part is an area (it has an inside) or a stroke (a line, an arrow, an
  open path); an arrow's head keeps its size in world units when the shape is stretched.
- **Regions** *(as built)*: boolean operations on areas (`&`, `|`, `-`). `area.rs` parses the
  expression (`&` binds tighter than `|` and `-`, brackets group, `only-a` is one name so the
  operator is written `a - b`) and keeps it as a tree that answers *is this point inside?*
  from the parts' exact implicit forms (a circle is `|p − c| ≤ r`; a closed path, even-odd).
  Nothing is clipped into a new outline: rasterizing samples the tree at the resolution it
  draws at, hit testing asks it about one point, and a rotated or stretched shape maps the
  point back into its unit box first. That has no slivers, tangent cases or precision drift;
  it gives up exact vector outlines of a region, which a GUI can still get by sampling finely.
  Shapes are made from their TOML definitions ("Shape definitions, as built").
- **Fitting** *(as built)*: text and region content laid out inside areas, overflow counted.
  `fit.rs` takes a region's `Room` (per row, the widest run of cells wholly inside it and clear
  of its outline) and lays out text (wrapped at spaces, "…" when cut short) or a list of labels
  (a column, or wrapped two spaces apart; "+N more", or "+N" where only that fits, with labels
  making way for it), starting from whichever row shows the most and centres best. A label wider
  than its row is cut with "…" in rows of four cells or more; narrower rows are skipped. Text is
  upright, one glyph per cell, at any rotation of the shape.
  **Labels** (`fit::anchored`) sit on their side of their anchor: the row above a `top`
  anchor, below a `bottom` one, beside `left` and `right`, on anything else (`Side::outside`).
  They never overlap. They are placed in the order of the shape's parts, so the first is the
  most important; a later label that would come closer than two blank cells to an earlier one on
  its row slides along the row to the nearest clear place within two cells of its anchor's
  column; failing that it is cut with "…" (not below four cells) and tried again; failing that
  it is left out and counted. At 40×12 the Venn's titles, five columns apart, come out
  `Fruit  Green` instead of touching (`tests/snapshots/labelled_venn_40x12.txt`). `draw.rs`
  paints a whole placed shape this way (regions tinted, outlines, text boxes, labels), which is
  what `cold-lava shapes --render` prints.
- **Rasterizing** *(as built)*: areas and strokes to terminal cells, braille dots (2×4 per cell)
  and, where the terminal has Kitty graphics, pixels; a GUI would draw the same geometry exactly.
  `raster.rs` does all three with one step, a `Mask`: the samples of a `Viewport`'s grid an area
  or a stroke covers, at 1×1, 2×4 or a cell's pixels per cell. An area's samples ask the region
  tree about their centres, only inside the area's box; strokes are walked in half-sample steps;
  an area's outline is its covered samples next to an uncovered one, so outlines sit just inside
  fills and follow rotation and stretching. A `Grid` holds a glyph and two style tokens (`fg`,
  `bg`) per cell; region tints go to `bg`, braille dots merge into `fg`, text is upright, one
  glyph per cell, and wins over dots. `Pixels` is a buffer of style tokens; the frontend picks
  colours and encodes the image (`kitty.rs`). Style tokens are numbers a frontend maps to its
  own colours.
- **Guides** *(as built)*: where a point placed by hand should go, so coarse clicks make a clean
  square, a regular hexagon or two circles of one size. `snap.rs`: onto a point, the next corner
  of a regular polygon, an angle in 15° steps and the previous side's length, level and plumb,
  and equal sizes, in world units. They place points and are never saved.
- **Hit testing** *(as built)*: what is under a point, for clicks, drags and handles. `hit.rs`
  answers with the topmost placed shape there and, within it, the handle (four corners and a
  rotate grip above the top edge, only on the selected shape), the topmost part (inside an area,
  or within a tolerance of a stroke) and the region. Blank space inside a shape's box still hits
  the shape.
- **Bindings** *(as built: "Bindings, as built" above, in the engine rather than this crate,
  which stays pure geometry)*: inputs run as gateway queries and stay live (`items/subscribe`);
  region expressions are evaluated when an input changes; gestures become gateway edits.
  `draw::paint_contents` fits each region's labels inside it.

The TUI only paints what the engine hands it. In a terminal, text stays upright, one glyph per
cell; strokes and areas rotate and stretch freely in braille or pixels. That is the
engine/frontend split again: the model is general, each frontend draws it as well as it can.

## Order of work

1. This design, and the Obsidian test for unknown canvas fields.
2. The canvas engine crate: geometry, regions, fitting, rasterizing, hit testing, all pure and
   tested without a terminal. *(as built: `crates/canvas`, #133; snapshots of the Venn with its
   regions labelled, at three sizes and turned, in `crates/canvas/tests/snapshots/`. Nothing
   draws with it yet: the TUI paints its grids once placed shapes reach the scene view.)*
3. Shape definitions: loaded like types, with built-ins, `cold-lava shapes WORKSPACE`. *(as
   built: `crates/engine/src/shapes.rs`, #134; "Shape definitions, as built" above.)*
4. Bindings: inputs as live queries, the region expression language, behaviour as edits.
   *(as built: `bind.rs`, `shows.rs` and `act.rs` in the engine, #135; "Bindings, as built" above.)*
5. Placed shapes in the scene view (after scenes, #91): place, move, stretch and rotate; the
   Venn diagram in the example workspace as the showcase; then the whiteboard (#92) built on
   shapes, and shapes in the document view through `![[…]]`. *(as built, on the whiteboard:
   "Placed shapes, as built" above, #136. The example workspace's `Volcano Venn.canvas`,
   embedded from `Guide/Whiteboard`, has a bound Venn, a card, a sticky with a task, an arrow
   and a checklist. Still to come: the document view drawing an embedded drawing in place
   (the reader shows its name, #138), and the drawing's file joining the workspace through an
   owner, so a sticky's task is a `task` and a card a backlink.)*
6. The shape editor: shapes made by drawing them, saved as the TOML they are written in. *(as
   built: "The shape editor, as built" above, #153.)*

## The notebook

Shapes are what make the notebook (docs/engine.md, "The notebook") possible: the pencil moment is
a drawing created at the cursor and edited in place, and "map this note" is a drawing whose shapes
are bound to the note's headings, links and tags, kept in sync both ways (an arrow drawn on the
map can add the link in the text, through the gateway). Writing never feels like operating a
drawing program: the shape machinery shows only when you draw or look at a drawing.
