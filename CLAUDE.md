# cold-lava — notes for Claude

cold-lava has a code editor's architecture, for human notes. It is written in Rust: an engine
that understands a workspace of plain files, and frontends on it (the terminal app today, built
with ratatui). Three things define it:

- **Shared types.** A task, a date or an address is the same thing wherever it is written (a
  note, a canvas card, a CSV row) and whoever reads it (the engine, an extension, the person
  writing). Owners translate file formats; types are the contract between them all.
- **Safe beside Obsidian.** It works on a workspace **without the Obsidian app, and safely
  alongside it**: plain Markdown and JSON Canvas, one safe write path, nothing overwritten.
  Everything else stands on this, and no feature may break it.
- **One door.** Everything outside the engine reaches it through the gateway. Extensions bring
  data, file formats, types and simple views (lists, tables, scenes). The surfaces you work in
  (the page with the reader and the editor, the whiteboard) are built into the frontend.

The order of work is the first section of roadmap issue #100: the engine-facing work first (one
path for each thing, one door, `cold-lava lsp`, types and identity), the notebook's UX last.

cold-lava's word for a folder of notes is **workspace**, in the UI, the CLI, the docs and the
code. "Vault" is only for Obsidian's own concept (an Obsidian vault, Obsidian's vault list,
`obsidian://open?vault=`).

## Commands

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # CI runs the latest stable clippy, which can be stricter than yours
cargo test --workspace
cargo run --release -- example-workspace                # main menu on the example workspace
scripts/release-check.sh                                # is this commit a release, and is it ready?
scripts/bench-items.py BINARY...                        # time and peak RSS of items/agenda/table, per binary, on a 3k-note workspace
```

All three checks must pass before pushing: CI runs them on Ubuntu and macOS, and they are
required checks on `main`.

## Architecture

A Cargo workspace of five crates, plus the GUI beside it. The version lives in the root `Cargo.toml` under
`[workspace.package]`; `example-workspace/`, `docs/` and `scripts/` stay at the root.

- `crates/engine`: the `cold-lava-engine` library (the core below). It has no UI and must not
  depend on ratatui, crossterm, portable-pty or vt100 (`cargo tree -p cold-lava-engine`).
- `crates/cold-lava`: the `cold-lava` binary (extensions and UI below, `tests/` with the drive
  scripts and goldens). `main.rs` imports the engine's modules by name, so `crate::workspace` etc.
  work in the binary.
- `crates/views`: `cold-lava-views`, the frontend framework, empty on purpose: pieces move into it
  from `crates/cold-lava` once a second frontend needs them (`docs/engine.md`, "The frontend").
- `crates/editor`: `cold-lava-editor`, cold-lava's own text editor (the main writing surface to come,
  docs/engine.md "The notebook"; other editors are extensions), headless (text, cursors, edits,
  undo; no drawing) so the TUI and the GUI render it alike. Empty until the document view edits
  in place. Saves go through the engine's safe writes.
- `crates/canvas`: `cold-lava-canvas`, the canvas engine of `docs/shapes.md`, headless and with
  no dependencies. Shapes are parts in their own unit box, placed by the user (move, stretch,
  rotate), with anchors; regions (`&`, `|`, `-` over parts' insides) stay expressions answered
  point by point (`area.rs`). Built on that: rasterizing to plain data (`raster.rs`: a `Mask` of
  covered samples at 1×1, braille 2×4 or pixels per cell; a `Grid` of glyph and style tokens;
  `Pixels`), fitting text and labels into a region's `Room` with "+N more" (`fit.rs`), and hit
  testing (`hit.rs`: the shape, handle, part and region under a point), and the shape editor's
  guides (`snap.rs`: angles, equal sides, regular polygons, level, equal sizes; never saved).
  `tests/snapshots/` holds
  rasterized Venns (`UPDATE_SNAPSHOTS=1` rewrites them); `cargo run --release -p
  cold-lava-canvas --example bench` times a full screen. Its own crate, not `views`: it is pure
  geometry every frontend computes the same way, not a frontend piece, and it must stay free of
  UI crates (`cargo tree -p cold-lava-canvas`). Labels (`label` parts, text at a point) are
  placed by `fit::anchored` so they never overlap; `draw.rs` paints a whole placed shape and
  previews it as text. Shape definitions are read by the engine (`shapes.rs`), not here, so the
  crate keeps no dependencies. Nothing places shapes yet; that comes after scenes (#91).
- `crates/gui`: `cold-lava-gui`, a Tauri window on the same engine. It is excluded from the Cargo
  workspace (its own `[workspace]` and `Cargo.lock`) so `cargo build`/`test` and CI never compile
  a webview; build it with `cargo build --manifest-path crates/gui/Cargo.toml` (Linux needs
  webkit2gtk-4.1). The terminal UI stays the main frontend. A new engine dependency must also
  go into `crates/gui/Cargo.lock`: `cargo metadata --format-version 1 --manifest-path
  crates/gui/Cargo.toml >/dev/null` resolves and writes it without building, and CI's "GUI lock
  up to date" step fails while it is stale.

**Core** (`crates/engine/src`; no UI; other code asks it instead of re-parsing files):

- `core.rs`: `Core` = one live workspace. Owns the `Index`, is the only file watcher, re-reads only
  changed notes and emits `Event`s (`poll()`, `subscribe()`). The menu owns it, and the graph
  runs in the menu's loop on it. Don't add another watcher or full rescan loop.
  `core/news.rs` (tests only) is the harness for what subscribers hear and in which order: it
  plays the watcher and an owner, applies steps (outside write, `items/edit`, rescan, rename,
  recipe, an owner's answers) and collects each delivery, with no waiting. Test a change to how
  news is queued there, not in a drive script. News goes into the queue through `Core::tell`
  only: while an `items/reset` waits to be taken, it drops that subscription's later item news
  (the query on the reset finds it), so a frontend never has to take news between two calls to
  keep a change from counting twice. News carries no order: `items/query`'s is the only one
  (notes by path, then owned files by path), and whoever shows rows queries again on any news
  (`Feed`, bound shapes).
- `write.rs`: the only way to write to a workspace: `write_atomic`, `replace` (refused if the file
  changed since it was read), `create`, `move_file`. Never call `std::fs::write` on workspace files.
- `trash.rs`: the only way to delete: `delete` moves a file to the trash the workspace's
  `trashOption` names (system trash, or `.trash`; never a permanent delete), refused if it
  changed since it was read; `restore` puts it back.
- `workspace.rs`: scans a workspace: notes with links (`refs` keep line and context), tags,
  frontmatter aliases, full text, each read by its owner. `search_key()` folds text for matching (lowercase, no
  accents, no ʻokina): use it for every user-facing search.
- `index.rs`: `Index` = workspace + the workspace's link `Resolver` + backlinks built from it.
- `resolver.rs`: the link resolver, both directions, with Obsidian's rules (path, then name,
  shallowest wins; attachments beside the note; `./` and `../`). Resolve: `Resolver` (the
  index's), `Files::resolve`, `split_link`, `attachment`, `file`. Write: `Files::write` in a
  `Style`, `Rewriter` (renames and merges), `new_note_path`/`new_file_path`. Don't write another
  link resolver: ask this one (docs/extensions.md, "Link resolver").
- `style.rs`: the workspace's style (how its notes are written). The Markdown owner counts link
  syntax as it reads (`Read::written`); `count_with()` counts the rest of a note the first time
  the style is asked for (`Note::style`); `Workspace::style()` adds them up into a `Style`, the majority,
  then Obsidian's settings, then `[style]` from config.toml (`style::set_configured`, from
  `config.rs`), then `.cold-lava/style.toml`. `cold-lava style` prints it. New text is spelled
  through it (`Style::link`, `tag`, `new_list`, `key_before`, `task_fields`; frontmatter's
  `new_key_at`): set-field, toggles, tag edits and merges ask it. Text already written keeps its
  own spelling; never format new text by hand. Extensions read it with `workspace/style`
  (`gateway::Request::Style`, answered by `Host::style`).
- `search.rs`: fuzzy matching (quick switcher) and full-text search with match positions.
- `rename.rs`: rename/move with every link updated;
  `plan()` then `apply()`. Its `FileEdit` (a file's planned new text, or what to ask its owner),
  `check_unchanged` and `Written` (files written, to put back) are shared by merges and recipes.
- `recipe.rs`: recipes, named whole-workspace changes (`BUILT_IN`: `rename-tag`). `plan()`
  (a `Draft` of every file's new text, nothing written, a diff per file), `apply()` (refused if a
  file changed; owned files get their text from their owners through `Core::relink` first) and
  `undo()`. A recipe finds what it changes through owners (tags: `owner::markdown::tags_in`),
  never by matching files itself.
- `json_canvas.rs`: JSON Canvas, Obsidian's `.canvas`: `Doc` (unknown fields kept), `parse`,
  `to_bytes`, and `fields`/`set` to change one node's `text` or `file` in place. The whiteboard
  and the canvas owner share it; don't parse `.canvas` anywhere else.
- `markdown.rs`: Markdown flavours (named sets of `Syntax`, built-in or from `[markdown]` in
  the config) and `parse()` into a `Doc` the reader draws. Obsidian extras pulldown-cmark lacks
  are rules in one section, each gated by its `Syntax`; the tag pattern is shared with `workspace.rs`.
- `cst.rs` (`cst/block.rs`, `cst/inline.rs`): the lossless syntax tree of a note on `rowan`
  (every byte a token; printing it gives the note back), blocks found line by line as
  CommonMark does and inlines per line, each gated by the flavour's `Syntax`. `Tree::edit`
  reparses only the top-level blocks an edit can change; `cst::read` gives a reader the steps
  of a walk through the tree without making it (the scan uses it). The Markdown owner's `read`
  and `tags_in` read links and tags this way, and `items::extract` reads items so (#140).
  Link definitions (`[label]: destination`) are blocks, kept as the note's `Definitions`
  (`Tree::definitions`, `cst::blocks`): a reference link (`[text][label]`, `[label][]`,
  `[label]`) is a link only where the note defines its label, and its reader asks the
  definitions where its `LinkLabel` points. An edit that changes the definitions parses the
  note whole. A tag may follow the markers that open emphasis, a strikethrough or a highlight
  (`**#tag**`, `markdown::MARKED_TAG`).
- `owner/`: owners translate a kind of file to items and links and back: read (text → links,
  items), write (an item edit → the file's new text, which the engine writes safely) and
  relink (links to moved files → new text). `owner/markdown.rs` is Markdown's, the first owner:
  the link parser, `items::extract`, `edit::plan`, `tasks::toggle_text` and rename's link
  rewriting, each reached through it. Read and change notes through the owner. An extension
  can own another kind: `owner/given.rs` reads its `file/read` answer (Markdown pieces, typed
  items, links) into items and refs, `owner/cache.rs` keeps answers by `rev` (on disk under
  `~/.cache/cold-lava/owners/`) and says what to ask, `owner/edits.rs` holds an edit while
  its owner answers `file/edit` and reads the new text back. `Core::set_owners`, `take_reads`,
  `take_file_edits` and `file_given` run it and never wait; an `items/edit` of an owned file
  is answered `Later` (`take_edited`). Owned files are `Workspace::owned`, beside the notes,
  and the index, the gateway, the graph and tidy read them too. `owner/canvas.rs` is the
  engine's `.canvas` owner (`owner::BUILT_IN`): text nodes are Markdown pieces and file nodes
  links, both at the node's id. No extension may claim a built-in owner's kind.
  **One path for owned files** (#183): a file that isn't a note is changed the same way
  whoever owns it. An item edit (`Core::edit_item`) and a rename, merge or recipe
  (`FileEdit::owned`, then `Core::relink`) ask the owner `file/edit`, read its new text back
  (`file/read`) and only then write. The canvas owner answers the same two requests through
  `owner::give`/`owner::edit`, inside the call, so a canvas's edit is done when `edit_item`
  returns (still `Later`, the answer already in `take_edited`) and `relink` returns `None` with
  the new texts in the plan when no extension has to answer. Don't give a built-in owner a
  shortcut around that flow, and don't branch on who owns a file outside `owner/mod.rs`.
  What stays direct: Markdown (the scan and a note's edits never ask anyone), the scan reading
  canvases into `owned` (it has no core to ask through), and planning a canvas's rename
  (`canvas::relink` plans its own changes, since a card holds a path, not a link).
- `items.rs`: notes as data. `extract()` reads a note into `Item`s (task, list, heading, block)
  in one walk through its tree (`cst::read`) in the workspace's flavour; `Workspace::items()`
  caches them per note on first use. Ids are stable (block id, heading path, or text hash under
  the parent), `rev` hashes the item's lines. The walk reads blocks as pulldown-cmark, which
  read them before, did, so ids didn't move; `items/pulldown.rs` keeps that pass for a test
  that compares the two on every note of the corpus. Read tasks through items, not by scanning
  lines.
- `gateway.rs`: the item messages of the workspace gateway (`items/query`, `items/subscribe`,
  `items/changed`/`items/reset`, `items/edit` with `set-field` and `toggle`, `types/list`) and
  their pinned JSON. `Core` answers them (`query_items`, `subscribe_items`, `edit_item`); an
  edit goes through `edit::set_field` or `tasks::toggle` (in a file another owner reads,
  through that owner: `owner/edits.rs`), is read again and emits
  `Event::ItemEdited` naming who made it. Extensions and screens change items only here.
- `links.rs`: the link graph for `links/query` (every file links come from, where each leads,
  Obsidian's graph colour groups); `links/subscribe` news is queued with the items' news.
- `files.rs`: the file messages of the workspace gateway (`workspace/read` with a `rev`,
  `workspace/write` refused unless the file is still that version, `workspace/create` where
  Obsidian puts a new file, `workspace/files`) and their pinned JSON; `resolve` keeps a path
  inside the workspace and out of hidden folders. `Core` answers them, and its watcher reports
  other files as `Event::FileChanged`.
- `tasks.rs`, `daily.rs`: Obsidian Tasks settings (statuses, global filter); `agenda` and
  `toggle` on items of the built-in `task` type (`types/task.toml`, read by `Task::of`); Daily
  notes settings, moment.js date formats.
- `shapes.rs`: shape definitions (docs/shapes.md): `[[shapes]]` and `[glyphs]` from the
  built-in file (`shapes/built-in.toml`: text, rect, circle, line, arrow, card, sticky, table,
  venn2, the glyphs), extensions' manifests, config.toml and `.cold-lava/shapes.toml`, in the
  order types load (`Origin` is types'). Checked at load, a broken one dropped with its error;
  each is built into a `cold_lava_canvas::Shape` (nested shapes flattened, anchors resolved,
  glyphs looked up). Built-ins are TOML: add a shape by writing it there, never a match arm;
  extend the format when it can't say something. `cold-lava shapes` lists and `--render`s them.
  `shapes/file.rs` writes one shape back as its `[[shapes]]` block, keeping the rest of the file
  as text and refusing a file changed since it was read; `with_draft` loads an unsaved one.
- `bind.rs`, `shows.rs`: bindings (docs/shapes.md, "Bindings, as built"). A `Binding` is a
  placed shape's inputs bound to `Source`s (a gateway query, a note, labels, text), followed
  with `items/subscribe` as `canvas` (`heard`, `noticed`), and answers what it shows now
  (`shown`: each region's labels, each text filled; `canvas`: the shape to draw). `shows.rs`
  reads and evaluates a region's `shows` over the inputs, matched on the shape's `key`.
- `act.rs`: behaviour. A `Gesture` (drop, click, key, action) runs the shape's `on` steps
  (`Binding::act`/`run`) through a `Gateway`: `Direct` (the core, as `canvas`) for the user's
  shapes, the extension's `Host` (its permissions, its name) for an extension's. `edit` goes
  through `items/edit`, `create` through `workspace/create`, a recipe through
  `Core::run_recipe`; `rebind` returns what the placed shape becomes (`Binding::rebind`). The
  extension's half is `shape/region` and `shape/gesture` (`Ask`, `Answered`), answered into
  `Core::take_shape_answers`.
- `paths.rs`: the user's home and config folders.

The engine never reads `config.toml`: `config.rs` in the binary parses it and hands the engine
what it needs (`markdown::set_configured` for the chosen flavour).

**Extensions** (`crates/cold-lava/src/ext/`, step 3 of `docs/extensions.md`):

- `ext/mod.rs`: `Extension` (one method per host→extension JSON-RPC method), `Host` (one method
  per extension→host method; it queues `Effect`s), `Registry` (loaded extensions, their
  manifests, the editor, and the menu's command table: the core's commands plus the extensions',
  built by `commands::menu`; keys, palette, help, `cold-lava keys` and config all read it).
  Three rules:
  - An extension can only do what a message can say: every type that crosses derives serde.
  - Extensions never see keystrokes and never draw: the core owns, draws, resizes and types
    into panes, and draws the lists and tables they send (`view/open`), telling them what the
    user did as `view/event`.
  - Side effects are queued: `Host` collects `Effect`s that `Home` applies after the call
    returns, so extensions never hold `&mut Home`. The gateway's `items/*` and `types/list` are
    answered during the call by the core, after `Host` checks the manifest's `permissions`
    (`items.read` by default, `items.edit`). The reader's `x` uses the same `Host`, as
    `Registry::screen`; the agenda and the table are built-in `Extension`s.
- `ext/nvim.rs`: Neovim as the editor extension: `VIM_SCRIPT`, what each OSC message means, the
  commands it types into its pane.
- `ext/process.rs`, `transport.rs`, `rpc.rs`, `frame.rs`, `installed.rs`: extensions in their own
  process. `installed` reads `~/.config/cold-lava/extensions/*/extension.toml`; `Process` is the
  `Extension` for one, starting it on `Extension::workspace` and answering it in
  `Extension::poll` (once a frame) through `rpc::serve` and `Host`, so the permission gate holds;
  `Transport`/`Connector` (stdio, threads behind channels, so the UI never waits) carry
  LSP-framed JSON-RPC (`frame`). The examples are `examples/extensions/open-tasks` and
  `csv-table` (Python), each with its own copy of `cold_lava.py` (a test keeps them alike).
- File types: a manifest's `[[file-types]]` (`FileType`) claims a kind of file; `Registry` gives
  each kind to the first claimant (`owner_of`; never `md` or `canvas`). The tree, the switcher,
  links and bookmarks hand an owned file to its owner as `file/open`, and the core's
  `FileChanged` reaches it as `file/changed`. `Host` lets an owner read and write its own kinds;
  other files need `files.read`/`files.write`. The example is `examples/extensions/csv-table`.
- `ext/view.rs`: the view gateway's messages (`Content` is a list or a table with a `kind`,
  `Event`, `ViewNews`) and its bounds. `views.rs` in the binary keeps the views extensions
  opened: drawn with `ListView`/`TableView`, updated once a frame, places in history while open.

**Features / UI** (`crates/cold-lava/src`):

- `home.rs`: the main menu (workspaces, note tree, the right pane with the reader or an extension's
  program, backlinks, switcher, search). It applies the extensions' `Effect`s.
- `recipe_view.rs`: the palette's `recipe.run` dialog (pick, options, plan with diffs); `home.rs`
  plans, applies and undoes through `recipe.rs`, asking owners as the merge dialog does.
- `embed.rs`: terminal-in-a-pane (portable-pty + vt100). Messages from the program arrive as a
  private OSC sequence `ESC ] 5379 ; kind ; payload BEL` (Neovim's come from `VIM_SCRIPT` in
  `ext/nvim.rs`); the payload encodes non-ASCII as `%u<hex>;`. The core passes them to the
  extension that owns the pane.
- `scene_view.rs`: the scene view, a scene of nodes and links (`Scene`, the view gateway's
  `scene` content) that the core lays out (`graph.rs`'s force layout, its repulsion through a
  Barnes–Hut quadtree, positions kept by node id), draws (`scene.rs`'s `Frame`, in braille or Kitty-graphics pixels under the text with
  `z=-1`, `kitty.rs`) and moves around in. It is one of `views.rs`'s views, ticked in the
  menu's loop while in front, so extensions keep being answered; keys whose meaning is the
  sender's go to it as `view/event`.
- `graph_ext.rs`: the graph as a built-in extension (`g`, `graph.open`): `links/query` made
  into a scene with its filters and colour groups (`graph::build`, `groups.rs`), sent with
  `view/open`, followed with `links/subscribe`. `app.rs` is the palette.
- `board.rs`: whiteboard on Obsidian Canvas (JSON Canvas) files (read and saved with the
  engine's `json_canvas`; their items and links are the canvas owner's), and placed shapes on it:
  `board/placed.rs` is how a placed shape is stored on a node (the `cold-lava` field, which node
  type it is, the guard against a save that drops the field), `board/live.rs` binds each one
  through the core and paints it with the canvas engine. The board is a mode of the menu
  (`Mode::Board`, `Home::board`, `Place::Board`): the menu keeps the core and the registry,
  hands them to each `Board` call, ticks and draws it in its loop and gives it the core's
  events and the `canvas` item news. `cold-lava board` runs it in a small loop of its own.
- `table.rs`: the declarative table view (columns, rows by id, events back; serde, so it can
  cross the view gateway). `type_table.rs` is the table as a built-in extension (`Tables`):
  `i` (`table.open`, a core command the menu hands to it) sends the types as a table to pick
  from, then the picked type's table with `view/open`, kept by `items/query` and
  `items/subscribe` (`Feed`, which the agenda uses too); a cell's edit is
  `items/edit set-field`. `build` is also what `cold-lava table` prints.
- `list.rs`: the declarative list / tree view (groups of rows by id with a checkbox, pieces of
  text with looks, a right side; selection, folding, filtering; events back; serde like the
  table). `agenda.rs` is the agenda as a built-in extension: it follows tasks with
  `items/query` and `items/subscribe`, sends its list with `view/open` and ticks with
  `items/edit toggle` (`A`, `agenda.open`, a core command the menu hands to it).
- `shape_editor.rs`: the shape editor (`S` on the whiteboard, a screen with its own loop that
  the whiteboard lends the core, so extensions wait while it is open): draws a
  shape and saves the same `[[shapes]]` TOML into the workspace's `.cold-lava/shapes.toml`.
  `shape_editor/draft.rs` is the editing on the `Definition` itself (grips, bending sides into
  `curve`s, regions read from what contains a click, anchors, a selection made into one shape),
  with no drawing; the guides are `snap.rs` in the canvas crate; the block written back as text,
  everything else in the file kept byte for byte, is `shapes/file.rs` in the engine.
- `main.rs`: CLI. `term.rs`: suspend/editor/Obsidian-URI helpers. `config.rs`: `config.toml`.
- `drive.rs`: `cold-lava drive`, scripted UI runs: the real binary in a pty, screen read back
  with vt100, cursor-position queries answered, the mouse clicked, dragged and scrolled.
  `graph.drive` and `board.drive` pin the graph and the whiteboard.

## Rules that keep it compatible with Obsidian

- Never overwrite edits made elsewhere: canvases three-way merge (`board.rs::merge3`), renames
  and task toggles check the file didn't change since it was read.
- Keep fields and formatting you don't understand (canvas `extra` maps; item edits and renames
  change a canvas node's `text` or `file` in place, `json_canvas::set`, instead of re-serialising).
- Follow the workspace's own settings where Obsidian has one: `.obsidian/app.json` (new-note
  location), `daily-notes.json`, `graph.json` colour groups, Tasks plugin `data.json`.
- Links keep the style they were written in (`[[Name]]` short, paths stay paths, relative
  Markdown links stay relative). Code blocks and inline code are never treated as links.
- Write files atomically (write a temp file, then rename).

## Workflow

- **No direct pushes to `main`**: a ruleset requires a PR with `ubuntu-latest`, `macos-latest`
  and `release check` passing. Tags `v*` can't be deleted or moved.
- One branch per feature. Work spanning several features goes in a **GitHub native stacked PR**
  (https://docs.github.com/en/pull-requests/how-tos/stacked-pull-requests), using the
  `gh stack` extension: `gh stack init`, `gh stack add BRANCH`, `gh stack submit` (or
  `gh stack link branch1 branch2 …` for branches made with plain git, bottom first). Merging the
  top PR merges the whole stack; `gh stack rebase` / **Rebase stack** restores linear history.
- Every user-visible change adds an entry under `## [Unreleased]` in `CHANGELOG.md`, and the
  README is updated with it.
- **Releases are automatic**: a PR that sets a new `version` in `Cargo.toml` (`[workspace.package]`) and moves the
  Unreleased entries into `## [x.y.z] - date` makes `.github/workflows/release.yml` tag the merge
  commit, build Linux/macOS binaries and publish the release. Before 1.0, minor = may break,
  patch = fixes only.
- Commit messages: a short summary line, then what changed and why.

## Testing the UI

- `example-workspace/` is the showcase workspace and is used by tests (they count its notes and groups).
  Copy it to a temp folder before trying anything that writes.
- Drive the TUI headless with `cold-lava drive SCRIPT -- WORKSPACE` (`cold-lava --help` lists the
  steps; `crates/cold-lava/tests/drive/*.drive` are examples). Wait with `wait-for TEXT`, not `wait`;
  `write FILE TEXT` changes a workspace file as another program would. Runs get an
  empty home (so they don't read your config or add workspaces to your list; `--real-home` to
  use yours), and `env COLD_LAVA_EDITOR=none` for the reader instead of Neovim. The embedded editor needs `nvim`
  (or `vim`) installed.
- To try a pushed branch without compiling it here (this laptop is short on memory), CI uploads
  its Linux release binary for 3 days: `gh run download RUN_ID -n cold-lava-linux -D DIR`, then
  `chmod +x DIR/cold-lava` (`gh pr checks PR` links the run). `DIR/cold-lava --version` names the
  commit it was built from: for a PR, GitHub's merge commit, whose second parent is the PR's head
  (`git log -1 --format=%P COMMIT`). Check it before trusting the binary.
- Pixel mode only shows in terminals with the Kitty graphics protocol (Ghostty, Kitty, WezTerm),
  not in a pseudo-terminal or tmux; check it in the real terminal.

## Where things stand

- The whole picture (engine layers, engine vs. frontends) is `docs/engine.md`; the crate split
  keeps the engine free of ratatui/crossterm. The extension API design is in `docs/extensions.md`. The connection to extensions must stay
  behind the `Transport`/`Connector` abstraction (stdio first; Unix sockets etc. later).
- The direction and the order of work were decided on 2026-11-11 (`docs/engine.md`, "Direction
  and order"; roadmap issue #100): finish the half-done migrations, one door into the engine
  (#179, #180, #193), `cold-lava lsp` (#195), types and identity (#190, #191, #186), the
  terminal views, drawings, then the notebook.
- The `Extension` interface exists and Neovim runs on it, and so do the agenda, the table and
  the graph (over the view gateway). The menu, the page and the whiteboard stay built into the
  frontend: they are not meant to become extensions.
- The project's issues are tracked on GitHub (#2 agenda and #3 daily/toggle are done).
