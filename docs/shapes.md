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
  part before it, like `frame.left`), `path` (`points`, `closed`), `text` (`value`, and either
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
  … ]`, or one `edit` alone. Steps are `edit` (`item`, and one of `set-field` with one of
  `value`, `add`, `remove`; `toggle`; `recipe`), `create` (a note: `text`, `name`), and `rebind`
  (`shape`, `inputs`). They are stored and checked now, and run in step 4.
- **Checked at load,** one line each, naming the file and the shape: a definition that doesn't
  read (an unknown part kind or field), no id, no parts, a bad area (an unknown or stroke part,
  a stray operator), an anchor no earlier part has, two parts or anchors with one id, a
  placeholder that isn't an input, a `shows` naming something not an input (its set expression,
  before `where`, `sort` or `limit`, goes through the area parser with the inputs as names), a
  gesture count other than one, a `drop` between regions that don't exist, an edit with no or
  two kinds, a `rebind` or a nested shape naming a shape that didn't load, and a cycle of nested
  shapes. The shape is dropped; its siblings, and shapes in other files, load.
- **`cold-lava shapes [WORKSPACE]`** lists every shape with its origin, inputs, parts, regions
  and behaviour, then the glyphs; notes and errors go to stderr, and errors exit 1. `--json` is
  the loaded set as data. `--render ID [--size WxH]` draws one, unbound, in a WxH terminal
  (default 60×18): braille outlines, text and labels, then its regions' tints by number.

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

### Bindings, as built

A placed shape's inputs are bound to data, and the engine says what it shows: `bind.rs` and
`shows.rs` in the engine, data and no UI. A frontend makes a `Binding` for a placed shape,
hands it the core's item news and events, and asks it what to draw.

- **Sources.** Each input is bound to one `Source`, saved with the placed shape as the user
  chose it:
  - a **query**: an `items/query` selection (`kind` and the gateway's `filter`: `tag`,
    `folder`, `note`, `status`, `property`, `text`, `date`), and an optional `title`;
  - a **note** (`Volcanoes/Krakatoa.md`): one member with its `title` and `name` (the file's
    stem), `path`, `tags` and `excerpt` (its first three lines of text, after the frontmatter
    and a first heading). This is a card's input;
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
  (`type=volcano tag=eruption title="Erupted"`), `note:PATH`, `items:A, B`, `text:…`, or JSON.

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

The example workspace has a three-circle Venn, a timeline that nests a `tick` three times, and
a glyph of its own (`:eruption:`) in `.cold-lava/shapes.toml`. `venn2` is built in rather than
the example's, as the docs' example and the showcase of step 5: a placed Venn works in every
workspace.

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
  (shape id, inputs, transform), so Obsidian opens it as a canvas. *Open question, to test in
  Obsidian first:* whether Obsidian keeps fields it doesn't know when it saves a canvas. If it
  drops them, placed shapes live in a file type of cold-lava's own, owned like any other
  (still embedded by link), so editing a canvas in Obsidian can never erase them.
- **The drawing's file joins the workspace** through its owner: text on a shape is Markdown,
  so a task written on a sticky note is a `task`, and a link on a card is a backlink.

## The canvas engine

A headless crate (like `cold-lava-editor`: nothing drawn), shared by every frontend. It is
`cold-lava-canvas` in `crates/canvas`, with no dependencies, built up in the order below.

- **Geometry** *(as built)*: parts in a shape's unit box (`part.rs`: `rect`, `circle`,
  `ellipse`, `line`, `arrow`, `path` as a polyline or cubic Béziers flattened once, `text` as the
  box its text will fill, `label` as one line of text beside a point); a `Placement` (what is saved: top-left corner, size, rotation in
  degrees clockwise about the centre) composed into one affine `Transform`; anchors
  (`left.top`, a part's id for its centre, the box's own `center`, `top-left`, …, `start`
  and `end` of strokes, and named points of the shape's own; a part's id may hold dots, as a
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
   *(as built: `bind.rs` and `shows.rs` in the engine, #135; "Bindings, as built" above.)*
5. Placed shapes in the scene view (after scenes, #91): place, move, stretch and rotate; the
   Venn diagram in the example workspace as the showcase; then the whiteboard (#92) built on
   shapes, and shapes in the document view through `![[…]]`.

## The notebook

Shapes are what make the notebook (docs/engine.md, "The notebook") possible: the pencil moment is
a drawing created at the cursor and edited in place, and "map this note" is a drawing whose shapes
are bound to the note's headings, links and tags, kept in sync both ways (an arrow drawn on the
map can add the link in the text, through the gateway). Writing never feels like operating a
drawing program: the shape machinery shows only when you draw or look at a drawing.
