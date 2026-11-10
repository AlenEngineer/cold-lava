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

Defined like types: in `.cold-lava/shapes.toml` in a workspace, in `~/.config/cold-lava/`, built
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
  `arrow`, `card` (a note), `table` (a type's rows, cells editable through `set-field`), and the
  glyph set.

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
  box its text will fill); a `Placement` (what is saved: top-left corner, size, rotation in
  degrees clockwise about the centre) composed into one affine `Transform`; anchors
  (`left.top`, a part's id for its centre, the box's own `center`, `top-left`, …, and `start`
  and `end` of strokes). A part is an area (it has an inside) or a stroke (a line, an arrow, an
  open path); an arrow's head keeps its size in world units when the shape is stretched.
- **Regions** *(as built)*: boolean operations on areas (`&`, `|`, `-`). `area.rs` parses the
  expression (`&` binds tighter than `|` and `-`, brackets group, `only-a` is one name so the
  operator is written `a - b`) and keeps it as a tree that answers *is this point inside?*
  from the parts' exact implicit forms (a circle is `|p − c| ≤ r`; a closed path, even-odd).
  Nothing is clipped into a new outline: rasterizing samples the tree at the resolution it
  draws at, hit testing asks it about one point, and a rotated or stretched shape maps the
  point back into its unit box first. That has no slivers, tangent cases or precision drift;
  it gives up exact vector outlines of a region, which a GUI can still get by sampling finely.
  Shapes are made in code for now; their TOML definitions are step 3.
- **Fitting:** text and region content laid out inside areas, overflow counted.
- **Rasterizing:** areas and strokes to terminal cells, braille dots (2×4 per cell) and, where the
  terminal has Kitty graphics, pixels; a GUI would draw the same geometry exactly.
- **Hit testing:** what is under a point, for clicks, drags and handles.
- **Bindings:** inputs run as gateway queries and stay live (`items/subscribe`); region
  expressions are evaluated when an input changes; gestures become gateway edits.

The TUI only paints what the engine hands it. In a terminal, text stays upright, one glyph per
cell; strokes and areas rotate and stretch freely in braille or pixels. That is the
engine/frontend split again: the model is general, each frontend draws it as well as it can.

## Order of work

1. This design, and the Obsidian test for unknown canvas fields.
2. The canvas engine crate: geometry, regions, fitting, rasterizing, hit testing, all pure and
   tested without a terminal.
3. Shape definitions: loaded like types, with built-ins, `cold-lava shapes WORKSPACE`.
4. Bindings: inputs as live queries, the region expression language, behaviour as edits.
5. Placed shapes in the scene view (after scenes, #91): place, move, stretch and rotate; the
   Venn diagram in the example workspace as the showcase; then the whiteboard (#92) built on
   shapes, and shapes in the document view through `![[…]]`.
