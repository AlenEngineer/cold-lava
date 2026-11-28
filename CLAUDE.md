# cold-lava — notes for Claude

cold-lava has a code editor's architecture, for human notes. It is written in Rust: an engine
that understands a workspace of plain files, and frontends on it (the terminal app today, built
with ratatui). Three things define it:

- **Shared types.** A task, a date or an address is the same thing wherever it is written (a
  note, a canvas card, a CSV row) and whoever reads it (the engine, an extension, the person
  writing). Owners translate file formats; types are the contract between them all.
- **Safe on shared files.** It works on plain Markdown and JSON Canvas that other programs
  (Obsidian, an editor, git) edit at the same time: one safe write path, nothing overwritten,
  nothing it doesn't understand dropped. Everything else stands on this, and no feature may
  break it. How another product reads those files is a profile a workspace can choose, never
  the core's definition.
- **One door.** Everything outside the engine reaches it through the gateway. Extensions bring
  data, file formats, types and simple views (lists, tables, scenes). The surfaces you work in
  (the page with the reader and the editor, the whiteboard) are built into the frontend.

The picture, told as a library with translators and a librarian, and a map of the crates, is
`ARCHITECTURE.md`. The ideas the project is built on are `docs/foundations.md`: read it before
designing anything, and where it and another document disagree about an idea, it is right. The order of work is
the first section of roadmap issue #100: the engine-facing work first (one
path for each thing, one door, types and identity), the notebook's UX last. cold-lava's own
editor comes before any language server: `cold-lava lsp` was dropped from the order on
2026-11-20, and stays a reader anyone may build later through the same door.

**How code is built** is the last section of `docs/foundations.md` ("Tools, called by a
simple loop, all the way down"): every unit is a caller (it only calls its tools in order) or
a leaf (it needs nothing but a dependency the project doesn't control); a tool owns its
state; complexity for speed is a stated decision, written where it is. Write new code that
way. When you meet a unit that is neither, split it; don't add to it. Much of the existing
code doesn't meet this yet (`Core`, `home.rs`, `board.rs`, `owner/given.rs`): don't copy
their shape, and don't refactor them inside a feature PR.

**Refactor before adding a feature somewhere** (decided 2026-11-12). The whole codebase was
read against that standard, and the work to meet it is seven tracks of small,
behaviour-identical steps (issues #265 A, #270 B, #271 C, #272 D, #264 E, #273 F, #274 G).
A track goes before the feature that lands on its code: the roadmap (#100) says which track
each feature waits for. Before starting a feature, check its line there; if it says "after
track X" and that step isn't merged, do the step first or stop and ask.

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

All three checks must pass before pushing: CI runs them on Ubuntu (Linux only, decided
2026-11-20; macOS comes back when the owner says), and they are required checks on `main`.
Until then the macOS branches (the `~/.Trash` trash, `open`, symlinked temp folders in the
drive tests) run nowhere.

## Architecture

A Cargo workspace of six crates. The version lives in the root `Cargo.toml` under
`[workspace.package]`; `example-workspace/`, `docs/` and `scripts/` stay at the root.

- `crates/engine`: the `cold-lava-engine` library (the core below). It has no UI and must not
  depend on ratatui, crossterm, portable-pty or vt100 (`cargo tree -p cold-lava-engine`).
- `crates/cold-lava`: the `cold-lava` binary (extensions and UI below, `tests/` with the drive
  scripts and goldens). `main.rs` imports the engine's modules by name, so `crate::workspace` etc.
  work in the binary.
- `crates/host`: `cold-lava-host`, the extension host every frontend shares (the host below).
  It has no UI and must not depend on ratatui, crossterm, portable-pty or vt100; it depends on
  the engine and on `cold-lava-views`, and the engine never depends on it. The direction is
  frontends → host → views → engine (`crates/host/tests/headless.rs` walks `Cargo.lock`, as
  `cargo tree -p cold-lava-host` would).
- `crates/views`: `cold-lava-views`, the frontend framework. Today it holds the view vocabulary
  as serde data, because the host names it: `list.rs`, `table.rs`, `image.rs`, `scene.rs`
  (`List`, `Table`, `Image`, `Scene` and their events) and `view.rs`, the view gateway's
  messages (`Content`, `Event`, `ViewNews`, the bounds), and `shown.rs`: `escape(text,
  Level)`, the one function that makes text a file or an extension chose safe to show
  (control characters but the tab, and U+2028/2029, become `\u{1b}`, `\n`, `\r`; at
  `Level::Message` the nine characters that turn the direction of text too, at
  `Level::Content`, the user's own content, they stay). Frontends call it where text
  leaves for the screen; never escape for showing with anything else. No UI dependency
  either. More pieces
  move into it from `crates/cold-lava` once a second frontend needs them (`docs/engine.md`,
  "The frontend").
- `crates/editor`: `cold-lava-editor`, cold-lava's own text editor (the main writing surface to come,
  docs/engine.md "The notebook"; other editors are extensions), headless (text, cursors, edits,
  undo; no drawing) so every frontend renders it alike. Empty until the document view edits
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
  crate keeps no dependencies. **Paused 2026-11-20, no new work** on shapes, bindings and the
  shape editor (this crate, `shapes.rs`, `bind.rs`, `shows.rs`, `act.rs`, `shape_editor.rs`).
  Foundations 11 and 12 still describe the design; what is missing is an owner for a placed
  shape's data, a new kind of book (ARCHITECTURE.md, "Where the story is ahead of the code";
  #375 says what must be decided to unpause). The whiteboard's own refactors (track F) go
  ahead.

**Core** (`crates/engine/src`; no UI; other code asks it instead of re-parsing files):

- `core.rs`: `Core` = one live workspace. Owns the `Index`, is the only file watcher, re-reads only
  changed notes and emits `Event`s (`poll()`, `subscribe()`). The menu owns it, and the graph
  runs in the menu's loop on it. Don't add another watcher or full rescan loop.
  `core/watch.rs` is that watcher, a tool: `Watch` owns the notify watcher, what it hears and
  the folder a linked `.obsidian` leads to, and gives back each path heard by its names in the
  workspace (`Watch::heard`); `classify` says what a path is (a note, a board, another file,
  a folder, the graph settings), for what was heard and for what cold-lava itself moved.
  `Core` only calls it.
  `core/whole.rs` is `Core::apply`, the one call that makes a rename, a merge or a recipe once
  it is planned and shown: the owners of the files other owners read give their new texts
  (`relink`), then it is written, then what they read is kept (`relinked`); when an extension
  has to answer, `take_relinked` says its ticket is ready and `apply_answered` makes the plan
  with the answer the edit desk kept. A ticket is good until the next `take_relinked`, which
  drops an answer not taken. The owners' answers never go out to a frontend, which holds only
  the plan and the ticket, and a file other owners read that has no answer is refused.
  `rename::apply`, `merge::apply` and `recipe::apply` are the engine's own, so no frontend
  writes a plan around it. One hole is left: a frontend that sets an edit's `owned` to `None`
  has the core write that file without asking its owner, until plans are kept by ticket
  (#193). Reading the workspace again afterwards is still the caller's.
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
  changed since it was read; the engine's only, `Seen::replace` outside it), `create`,
  `move_file`. `replace` never follows a symlink: a link at the path is refused (`Link`), and
  one swapped in after that check is compared by what it leads to, then swapped out as a link
  and removed, never written through; a folder swapped in on the way
  is still followed (#366). It is every write over a file the engine found by its scan or
  the gateway let through. `replace_through_link` (`Seen::replace_through_link`) is the
  exception, by name: the user's own save of a file they opened by its path (the board's
  canvas, the menu's tag edit of a note its tree lists, `shapes/file.rs`) lands in the file a
  link leads to and the link stays (`landing`, found once per write), as `write_atomic` does.
  A link to nothing or to a folder is `NotAFile`. The gateway never lets a link through
  (`files::resolve`).
  `taken` is the one test of whether something is at a path, a link to nothing included:
  `create`, `create_dir`, `files::free_name` and `trash::free_path` ask it, and so does a
  merge's undo. `may_move` is the one test of whether a move may go ahead: nothing is at
  the new path, or it differs only in case and is the very entry that moves (#388);
  `move_file` and a rename's checks ask it. No other program's write that lands while one of
  these is under way is replaced (#395, #230): `replace` (`write/replace.rs`) swaps its new
  file in (`write/os_rename.rs`, `exchange`: renameat2 or renameatx_np) and compares what
  came out, putting another write's text back and refusing (`Changed`). The swap back checks
  what comes out too: a newer text that landed in between is swapped in again, so the newest
  stays at the path and the older is kept beside the file as
  `.Name.cold-lava-kept-SECONDS-N.ext` and named (`Kept`, the gateway's `kept`).
  `find_kept` (`write/find.rs`) is the one finder of kept texts and of killed saves'
  leftovers, and the core lists them (`files::KeptList`, `Core::kept_texts`,
  `workspace/kept`, `Event::KeptChanged`, #427). After 16 rounds (`ROUNDS`, a writer that doesn't stop) or a
  swap that fails, the path may hold an older text: the newer one is then kept beside it and
  named, never removed. Where the file system can't swap, the file is moved aside
  by one rename onto a name it reserved and compared there. A text the engine took out is
  removed only when it is the caller's old text, the write's own, or what the path then
  holds, and after a swap back only when its rounds settled. cold-lava's own replaces take
  turns (`write/lock.rs`): each holds `flock` on its file's folder (the link's target's, for
  `replace_through_link`) from the compare to the last swap (the new text is staged before),
  so no two replaces that hold their turn race the swap; other programs, and cold-lava's own moves and deletes
  (`move_file`, `trash::delete`, which take no turn), still can. The lock leaves no file, a
  write that waits 5 s goes ahead without it (each later save in that folder waits the 5 s
  again while the holder is stopped), and on NFS it is never held. A swap back
  can't tell a later text from an earlier one another swapper put back, so with a writer that
  went ahead unlocked a done save may end up kept beside the path (never removed). The words
  a person sees name the kept file first, by its workspace path (`kept_words`, from the root:
  `WriteError::named_from`, or `named_in` for an error passed on; `kept_beside` only where no
  root is known, the edit desk). Every caller classifies a `WriteError` through `Reason::of`
  (`EditError::from(gateway::Error)` for an edit), never by matching its variants. `create`
  and `move_file` rename with `no_replace`, refused when something is there by then. Left
  open: a program that opened the file before the swap (an open under way counts: an
  in-place writer under load, #443) and writes into it after the compare; and, on a file
  system with neither rename flags nor hard links, `no_replace`'s check-then-rename
  (`os_rename::by_link`), which `create`, `move_file` and `replace`'s aside all go through.
  A save killed halfway leaves its staged `.Name.cold-lava-PID-N.tmp`: the first write a
  process stages in a folder clears those of dead PIDs (`write/stale.rs`: a file `aside` moved out,
  under its own `.aside` name, put back at its path when nothing is there, removed when a copy of the file or empty, kept under a kept
  name otherwise; a kept text never). It holds the
  folder's turn while it does (`lock.rs`), so a live `replace` it can't see by PID (another
  container's, another machine's) keeps its staged file; where the turn can't be had (NFS, a
  wait run out) it clears nothing there. A staged file changed or renamed within a minute
  (`AGE`, by the later of mtime and ctime) stays too. The staged name is made and read in
  one place (`write/name.rs`); `keep` is `write/kept.rs`. Never call
  `std::fs::write` on workspace files.
- `trash.rs`: the only way to delete: `delete` moves a file to the trash the workspace's
  `trashOption` names (system trash, or `.trash`; never a permanent delete), refused if it
  changed since it was read; `restore` puts it back. Frontends and extensions don't call
  these: they ask the core (`Core::delete_file`, `restore_file`: `workspace/delete`,
  `workspace/restore`). `Core::set_system_trash` names the system trash; a test that
  deletes sets one under its own folder. `delete` takes the `Seen` the user was shown.
- `read.rs` holds `Seen`, what was at a path when it was read, so a later write or delete can
  be refused if it changed since. Every write or delete that checks for a change goes through
  it (`files.rs`, `trash.rs`, `merge.rs`, `rename.rs`'s `Written` and `apply`, set-field, the
  tick, the shapes file, the board's save, the menu's tag edit, and the owned
  file's edit and the relink check the core makes for the edit desk), except `gateway.rs`'s
  item edit, `rename::check_unchanged` and `rename::owned_edit`, which move next (track A step
  A5, #265). `Seen::file` reads a file's bytes through a
  symlink, as a write does; `Seen::kept` holds bytes read earlier or just written, checked as
  `file` is; `Seen::entry` reads a file, a folder by its listing (`inside`) or a symlink by
  where it points, never following one. `unchanged` reads it again the way it was seen;
  `unchanged_entry` reads the entry itself, whatever the way, as a delete must (a file seen
  through a link is not the link). `replace` writes through `write::replace` (no link
  followed) and `replace_through_link` through `write::replace_through_link`, only over a file
  seen as a file. `rev` is the gateway's version (a folder's is its listing's); `text` refuses
  bytes that aren't UTF-8 with `NotText`, whose `about` is the one wording of "isn't UTF-8
  text". The gateway's `Reason::of` names a `WriteError`'s reason. It is also the one home of
  a version: `read::rev` (a file's bytes), `rev_at` (the file at a path now) and `Seen::rev`
  (what was seen, the way it was seen: through a link by `file`, the link itself by `entry`,
  as `workspace/stat` and a delete read it). It needs only `write`, `identity`, std and
  walkdir; `files.rs` and `trash.rs` depend on it, never the other way.
- `workspace.rs`: scans a workspace: notes with links (`refs` keep line and context), tags,
  frontmatter aliases, full text, each read by its owner. `search_key()` folds text for matching (lowercase, no
  accents, no ʻokina): use it for every user-facing search.
- `index.rs`: `Index` = workspace + the workspace's link `Resolver` + backlinks built from it.
- `resolver.rs`: the link resolver, both directions, with Obsidian's rules (path, then name,
  shallowest wins; attachments beside the note; `./` and `../`). Resolve: `Resolver` (the
  index's), `Files::resolve`, `split_link`, `attachment`, `file`. Write: `Files::write` in a
  `Style`, `Rewriter` (renames and merges), `new_file_path` (the engine's own: a frontend
  asks `files::new_note` or `files::new_file`). Don't write another
  link resolver: ask this one (docs/extensions.md, "Link resolver").
- `style.rs`: the workspace's style (how its notes are written). The Markdown owner counts link
  syntax as it reads (`Read::written`); `count_with()` counts the rest of a note the first time
  the style is asked for (`Note::style`); `Workspace::style()` adds them up into a `Style`, the majority,
  then Obsidian's settings, then `[style]` from config.toml (`Configured::style`, from
  `config.rs`), then `.cold-lava/style.toml`. `cold-lava style` prints it. New text is spelled
  through it (`Style::link`, `tag`, `new_list`, `key_before`, `task_fields`; frontmatter's
  `new_key_at`): set-field, toggles, tag edits and merges ask it. Text already written keeps its
  own spelling; never format new text by hand. Extensions read it with `workspace/style`
  (`gateway::Request::Style`, answered by `Host::style`).
- `search.rs`: fuzzy matching (quick switcher) and full-text search with match positions.
- `rename.rs`: rename/move with every link updated;
  `plan()`, then `Core::apply`. Its `FileEdit` (a file's planned new text, or what to ask its
  owner), `check_unchanged` and `Written` (files written, to put back) are shared by merges and
  recipes.
- `recipe.rs`: recipes, named whole-workspace changes (`BUILT_IN`: `rename-tag`). `plan()`
  (a `Draft` of every file's new text, nothing written, a diff per file), `apply()` (refused if a
  file changed; owned files get their text from their owners first, through `Core::apply`) and
  `undo()`. A recipe finds what it changes through owners (tags: `owner::markdown::tags_in`),
  never by matching files itself.
- `json_canvas.rs`: JSON Canvas, Obsidian's `.canvas`: `Doc` (unknown fields kept), `parse`,
  `to_bytes`, and `fields`/`set` to change one node's `text` or `file` in place. The whiteboard
  and the canvas owner share it; don't parse `.canvas` anywhere else.
- `markdown.rs`: Markdown flavours (named sets of `Syntax`, built-in or from `[markdown]` in
  the config) and `parse()` into a `Doc` the reader draws, built from the note's tree (`cst`),
  the one the index and the items read, so the reader shows as a link, a tag or a task what
  they count (#212). `markdown/pulldown.rs` keeps pulldown-cmark's parse for a test that
  compares the two `Doc`s on every note of the corpus; where they differ is listed there with
  the reason (`DIFFERENT`: the tree reads what the index counts, or pulldown-cmark wasn't
  Obsidian; `LESS`: the reader shows less than Obsidian, which is the tree's to change). Don't
  parse Markdown for the reader anywhere else. `markdown/entities.rs` is HTML5's named
  character references. `markdown/tag_name.rs` is what a tag's name is (the patterns that
  find `#tag`, `is_number`, and `is_name` for a name given on its own), the one rule the
  tree, the reader, the frontmatter's `tags`, the tag dialog and rename-tag ask (#272, D-4).
  Two places don't ask it yet, recorded on #272: the matcher's frontmatter gate and its
  `Value::Tag` (which a `set-field` on a tag field uses). Don't add a third.
- `cst.rs` (`cst/block.rs`, `cst/inline.rs`): the lossless syntax tree of a note on `rowan`
  (every byte a token; printing it gives the note back), blocks found line by line as
  CommonMark does and inlines per line, each gated by the flavour's `Syntax`. `Tree::edit`
  reparses only the top-level blocks an edit can change; `cst::read` gives a reader the steps
  of a walk through the tree without making it (the scan uses it). The Markdown owner reads
  every link and tag this way (`links`, `tags_in`), and `items::extract` reads items so (#140).
  Link definitions (`[label]: destination`) are blocks, kept as the note's `Definitions`
  (`Tree::definitions`, `cst::blocks`): a reference link (`[text][label]`, `[label][]`,
  `[label]`) is a link only where the note defines its label, and its reader asks the
  definitions where its `LinkLabel` points. An edit that changes the definitions parses the
  note whole. What text itself holds (escapes, character references, autolinks, inline HTML,
  footnotes, hard breaks, emphasis, strong, strikethrough, highlights; #212) is read after the
  lines, a block at a time, and only cuts `Text` tokens and wraps them in nodes: `Tree::parse`
  and `Gates::of` read it, the index and the items don't (`Gates::INDEX`, `Gates::plain`) and
  read the same links, tags and blocks either way (a test checks it on the corpus). Its kinds
  come last in `Kind`, so the others keep their numbers. A tag may follow the markers that
  open emphasis, a strikethrough or a highlight
  (`**#tag**`, `markdown::tag_name::marked_pattern`).
- `owner/`: owners translate a kind of file to items and links and back: read (text → links,
  items), write (an item edit → the file's new text, which the engine writes safely) and
  relink (links to moved files → new text). `owner/markdown.rs` is Markdown's, the first owner:
  the link parser, `items::extract`, `edit::plan`, `tasks::toggle_text` and rename's link
  rewriting, each reached through it. Its `links` is the one reading of a note's links, from
  the tree: `read` (backlinks, the graph), `link_targets` (tidy), `relink` (renames, which
  rewrite a reference link in its definition and no byte outside a target) and an item's own
  `links` all take them from it. Don't match links or tags with a pattern of your own. Read
  and change notes through the owner. An extension
  can own another kind: `owner/given.rs` reads its `file/read` answer (pieces, Markdown
  pieces, typed items, links) into items and refs, `owner/cache.rs` keeps answers by `rev` (on disk under
  `~/.cache/cold-lava/owners/`) and says what to ask, `owner/edits.rs` (the edit desk, a tool
  with private state) holds an edit while its owner answers `file/edit` and reads the new text
  back, then hands the core what to write over what (`ToWrite`: the file as the core read it,
  a `read::Seen`), and answers whoever asked once the core says how the write went
  (`Edits::wrote`). The owners' answers for a rename, merge or recipe wait there under its
  ticket (`Edits::relinked`). The desk reads and writes no file and tells no subscriber: the
  core does. `Core::set_owners`, `take_reads`, `take_file_edits` and `file_given` run it and
  never wait; an `items/edit` of an owned file
  is answered `Later` (`take_edited`). Owned files are `Workspace::owned`, beside the notes,
  and the index, the gateway, the graph and tidy read them too. `owner/canvas.rs` is the
  engine's `.canvas` owner (`owner::BUILT_IN`). It speaks the alphabet: each card is a piece
  at the node's place (`text`; `embed` for a file card or a web page; `opaque` for a type it
  doesn't know), placed by the attributes `x`, `y`, `width`, `height`, and each arrow a link
  between two pieces (`given::Between`, `{from, to, label}` in `links` on the wire and
  `Given::between` in the engine), carried and never
  interpreted. What a board gave before is still given beside them, unchanged: a text
  card's text as a Markdown piece and a file card's link, both at the same place. **A
  board's items, links, ids and revs must not move**: a card's piece is an item only when a
  type tests for one of its attributes (`matcher/piece.rs`, the one place for it: a piece
  with no words matches only a shape whose `where.attribute` it carries, for every owner),
  the owner asks `given::Places` which cards and arrows the engine takes instead of
  restating its bounds, a group is not given (a `section` piece would be a new heading on every
  board), and the owner gives no piece the engine would report. A card's Markdown becomes a
  fragment with #244, not before. No extension may claim a built-in owner's kind.
  **One path for owned files** (#183): a file that isn't a note is changed the same way
  whoever owns it. An item edit (`Core::edit_item`) and a rename, merge or recipe
  (`FileEdit::owned`, then `Core::apply`) ask the owner `file/edit`, read its new text back
  (`file/read`) and only then write. The canvas owner answers the same two requests through
  `owner::give`/`owner::edit`, inside the call, so a canvas's edit is done when `edit_item`
  returns (still `Later`, the answer already in `take_edited`) and `apply` makes a rename at
  once when no extension has to answer. Don't give a built-in owner a shortcut around that
  flow, and don't branch on who owns a file outside `owner/mod.rs`.
  What stays direct: Markdown (the scan and a note's edits never ask anyone), the scan reading
  canvases into `owned` (it has no core to ask through; it reads them as the core does,
  `owner::give_bytes`, and a rescan takes the scan's reading of a version as the owner's),
  and planning a canvas's rename
  (`canvas::relink` plans its own changes, since a card holds a path, not a link).
  **Pieces** (#243, docs/extensions.md "Pieces, as built"): an extension's owner can say what
  its file holds in the alphabet's terms, `given::Piece` (`place`, `role`, `attributes`,
  `text` as `Run`s: plain, marked, link, label, field; `in` for what it is inside, `cells`
  on a row). **The engine never reads a piece's text as Markdown**: `#42`, `[[x]]` and
  `- [ ]` in a plain run are characters, and a tag or a link is only what the owner said is
  one. Types match a piece through `matcher/piece.rs` (`Matcher::piece`), which reuses the
  matcher's conditions, precedence and value parsing but reads each source as written:
  `attribute = "due"` is the value the owner gave, never Markdown's reading
  (`Readings::flat` is for notes only). Its shapes are compiled the first time an owner gives
  a piece, so a note's path runs none of it: keep `Matcher::block`, `read` and `read_source`
  free of pieces. `checkbox` on a piece is a word (`given::CHECKBOX`: `open`, `done`,
  `in-progress`, `cancelled`), read as the workspace's status symbol for it. A piece's item
  id is `given::piece_id` (file, kind, place, then `piece`), apart from a typed item's and a
  Markdown piece's: none of the three may take another's id. An answer is untrusted:
  every field of its types names its bound (`bounded.rs`), which applies as the answer is
  read in the host; an item is read on its own by its keys (`owner/answer.rs`: `Kept`,
  `GivenItem::read`, a run's reader), so one that doesn't read is left out alone with the
  reader's words (`Given::left_out`, its one home in memory, which the cache saves and reads
  back beside the answer, and the read-back compares). Every answer, the canvas owner's
  and what `given::apply` makes included, goes through `given::Places` as it is read into
  items, which asks each item's bounds again (`GivenItem::check`) and holds **one rule for
  a place**: 1 to 1024 bytes, and one item of each form at it (the first within its bounds
  claims it; a second of that form is left out). A piece, a Markdown piece and a typed item
  may share a place (a board's text card is both a piece and a Markdown piece), since their
  ids and changes never cross. A change at a place is for the item that holds it
  (`apply`), and a rename, a recipe and tidy read only those (`Given::placed`,
  `placed_links`). A link to a file is checked there too (`Places::link`), alone, since a
  link that doesn't read as it arrives would refuse the whole answer (`WithAnswer`).
  `read_piece` applies what compares a piece with the rest of its answer (depth, `in`), and
  a place is never written into a file. **One rule for a change to an owned file**
  (`given::check_change`): an item edit and a relink alike are refused before the owner is
  asked when the change would leave out something the file as given doesn't
  (`left_out_by`, told apart by `Subject`, never by words), and go ahead beside what is
  already left out (#257). The read-back is its other half: everything the engine leaves out
  (left out as it arrived, by the JSON value it was sent as, once parsed; an item, a link
  to a file or a link between pieces left out as it is read, by what it reads as) must
  come back where it was, holding what it held, or the edit is refused naming it and
  nothing is written. The refusal names what changed, by the fewest moves that explain
  each list's new order (`owner/order.rs`, whose bound on repeats is `MORE_PAIRS`); when as
  few moves explain either, the left-out one is named. An item that reads is told apart by
  its form and place (two forms may share a place), a link given twice is two of one key,
  and one that moved past a left-out one is named, not the left-out one. A place names the
  item that holds it (`Places`, `Given::placed`): find an edit's item among those, never by
  order. Don't add a second rule for edits, and don't compare what is left out by its words.
  Markdown pieces and typed items read as before.
  **Changes in the model's terms** (`owner/change.rs`, docs/extensions.md "Changes in the
  model's terms, as built"): `items/edit` on a piece becomes `given::Change::Attribute`
  (`{place, attribute, value}`; a tick is `checkbox = done`, one change, no done date) or
  `Change::Runs` (`{place, text}` as runs, `cell` for a row's cell), written where the type
  reads the field. Rewording goes through `given::reword`, which keeps the links, labels and
  marked runs whose words are still there. A rename repoints a piece's links with
  `Change::Link`. Never make a Markdown line for a piece, and never read one back: the
  Markdown owner's `edit` is for Markdown pieces only. The read-back compares pieces too
  (`given::same`, `piece_key`), in the order given: what each says (role, attributes, text,
  cells) and where it stands (its position, and the position of the piece it is `in`), not
  its place; then every place both answers have must hold the piece expected there. Order
  and the place check are what catch a change made to an identical twin: don't sort them.
  New words (a piece's, a cell's) are refused unless every word they replace shows in the
  field being set: `change.rs` compares the field with the piece's full plain words, so
  whatever took a word out (a `marker`, a `without`, a source added later) is covered.
  Don't turn that back into a list of source kinds. A piece's tick follows the workspace's
  next statuses to the first that is another of the four words (`change::next_for_a_piece`)
  and never answers "unchanged". A `link` field on a piece is the owner's value as written. A typed item
  an owner gave is ticked as `Change::Field` on `status`. The protocol has no change that
  adds, moves or removes a piece yet, and `items/edit` takes no runs.
- `items.rs`: notes as data. `extract()` reads a note into `Item`s (task, list, heading, block)
  in one walk through its tree (`cst::read`) in the workspace's flavour; `Workspace::items()`
  caches them per note on first use. Ids are stable (block id, heading path, or text hash under
  the parent, then `.n` for the nth repeat), `rev` hashes the item's lines, and a twin's (a
  repeat whose lines another has too, `items/twins.rs`) also how many twins there are and,
  under a twin, that twin's rev (`identity::twin_rev`), so an edit held by an address another
  twin took is refused as changed (#327). The walk reads blocks as pulldown-cmark, which
  read them before, did, so ids didn't move; `items/pulldown.rs` keeps that pass for a test
  that compares the two on every note of the corpus. pulldown-cmark is a dev-dependency of
  the engine: those two test modules (`items/pulldown.rs`, `markdown/pulldown.rs`) are all
  that use it, and nothing shipped may. Read tasks through items, not by scanning lines.
  `items/anchors.rs` is the anchor rule: an edit loses no block id, moves none to another
  item and makes none, but on the edited item: one already written in its own text (an
  untick of a line 0.6.0 ticked after `^rent`), and the one `items/edit`'s `anchor` writes
  for it (#190), which then counts as its own. `set-field`'s, the tick's and the anchor's
  read-backs all ask it (`anchors::change`); don't check block ids anywhere else. (The
  anchor's tree check, `owner/markdown/anchor.rs`, asks Markdown's syntax only whether it
  reads the name it wrote as a block id where it wrote it.)
- `alphabet.rs`, `types.rs`, `matcher.rs`: the alphabet and the type language.
  `alphabet::Role` is the nine roles a piece of writing can have (`document`, `section`,
  `text`, `item`, `row`, `aside`, `verbatim`, `embed`, `opaque`), `alphabet::Mark` the six
  marks a run of text can have, and `alphabet::VERSION` its version (2: owners can give
  pieces; 3: a link between two pieces has a written form), sent in `initialize`; the whole alphabet (text, the five relations, open
  attributes) and what the engine does with each letter today is docs/extensions.md, "The
  alphabet, as built", whose table of roles a test checks against `Role::ALL`. Only the
  engine adds a letter, with a version bump: a new kind of thing is a type, never a role.
  `types.rs` reads and checks type definitions (`[types.NAME]`: a schema and `from` shapes)
  from every origin; `matcher.rs` matches them. The words a definition and an attribute
  reading are both written in (`On`, `Source`, `Sources`, `Provided`, `fill`, `compile`,
  `is_name`) are `sources.rs`, a leaf but for `alphabet::Role`, and where a layer of definitions came from (`Origin`:
  types, attribute readings and shapes are loaded in layers) is `origin.rs`, another.
  `types.rs` and `owner/markdown/attributes.rs` both read them there, so neither depends on
  the other for them (`types.rs` re-exports the ones `matcher.rs` and the table's
  `Source` still name through it, #343). A shape's `on` (`sources::On`) is a role or
  `inline` (a reading of a piece's text, not a role). The names `on` had before (`note`,
  `heading`, `paragraph`, `list-item`, `table-row`, `quote`, `code`) are aliases
  (`On::alias`): they always read, a definition's JSON still says `on` in them with `role`
  beside it, and a block's `inside` holds them, since that list also holds kinds and type
  names. Never drop an alias or print a role where a client reads the older name. An
  item's kinds (`task`, `list`, `heading`, `block`) are not roles and keep their names,
  because ids hash them; `items::KINDS` says how each maps to a role.
  **Attributes** (#242) are what a piece carries beyond its text, a name and a value, given
  by its owner. A shape tests one with `where.attribute` and reads one with the source
  `{ attribute = "name" }`, beside the older sources, which all still read. The built-in
  `task` (`types/task.toml`) is defined over them and names no character of Markdown's.
  **How Markdown writes each attribute is the Markdown owner's data, not the type's:**
  `crates/engine/markdown/attributes.toml`, read by `owner/markdown/attributes.rs`
  (`Readings`; then `[attributes]` in config.toml and `.cold-lava/attributes.toml`, a later
  entry replacing an earlier one; `cold-lava attributes` lists them). An entry is a list of
  the text sources (`inline`, `marker`, `flags`, `pattern`, `checkbox`), and
  `Readings::flat` spells a field's `attribute = …` out into them, in place. The matcher
  reads through that list and `edit.rs` writes through the same one, so
  `matcher::Place::sources` means the same to both: never index a shape's sources as
  written. The Tasks plugin's emoji, Dataview's fields (the `"*"` entry, for every
  attribute with none of its own) and the global filter live there, so the profile work
  (#235, #236) moves that file and no type. `checkbox` (the item has one) and `task` (its
  owner counts it as a task: in Markdown the checkbox `with` the global filter) are two
  attributes: the task type asks for `task` and reads its status from `checkbox`. Keep them
  apart, so a type on "an item with a checkbox" sees every checkbox.
  Don't put a glyph or a `${tasks.…}` reading rule back into `task.toml`, and don't turn a
  reading into a Rust match arm. `level`, `callout` and `fold` are the tree's
  (`matcher::Attr`, from `matcher::Given`, which holds what the walk knows that the text
  doesn't). Nothing keeps an
  attribute while a note is read: one is read off the block only when a shape names it.
  Keep it that way: don't store attributes on `items::Item`. Items carry none over the
  gateway yet, an owner's pieces included: what an owner gave stays in `Owned::given`, and
  a client sees it through a type's fields. When they do go on the wire, it is with the
  owner's values, not a type's reading of them, and without growing a note's items. A type on `aside` matches a quote or a callout; a typed aside's
  key is set apart (`items::finish`), so it can never take an id from one of its type's
  older items. `types/task.before-242.toml` is the task as it was, kept only for
  `items::tests::the_task_reads_from_attributes_as_it_did_from_characters`, which reads
  every note both ways: a change to how a task is read must keep that test passing, or it
  changes ids. Reading and `set-field` go through the table; ticking and recurrence
  (`tasks.rs`: `toggle_text`, `next_occurrence`, `strip_field`, `add_field`, `replace_date`,
  the task-line pattern, the default statuses), `style::TASK_GLYPHS` and the agenda's
  glyphs (`crates/cold-lava/src/agenda.rs`) still hold the plugin's markers as code
  (docs/extensions.md, "Attributes in Markdown, as built", lists them). A changed entry for
  one of `attributes::TICKED` is warned about. An entry emptied (`due = []`) stays in the
  table with no readings, so the attribute is on no piece and `"*"` doesn't stand in.
- `gateway.rs`: the item messages of the workspace gateway (`items/query`, `items/subscribe`,
  `items/changed`/`items/reset`, `items/edit` with `set-field` and `toggle`, `types/list`) and
  their pinned JSON. `Core` answers them (`query_items`, `subscribe_items`, `edit_item`); an
  edit goes through `edit::plan` or `tasks::toggle_text` (in a file another owner reads,
  through that owner: `owner/edits.rs`), is read again and emits
  `Event::ItemEdited` naming who made it. Extensions and screens change items only here.
- `links.rs`: the link graph for `links/query` (every file links come from, where each leads,
  Obsidian's graph colour groups); `links/subscribe` news is queued with the items' news.
- `files.rs`: the file messages of the workspace gateway (`workspace/read` with a `rev`, which
  `read.rs` makes, `workspace/write` refused unless the file is still that version,
  `workspace/create` where Obsidian puts a new file, `workspace/files`) and their pinned JSON;
  `resolve` keeps a path out of hidden folders and never follows a symlink, at the path or
  on the way to it (#349): a permission is judged by the path's name, and a link leads to a
  file of another name. With no link and no `..`, a path stays inside the workspace. The
  scan and the watcher read no link either (`workspace::first_link`, the one rule, which
  `resolve` asks), so no item is in one and `items/edit` never reaches one. The user's own
  saves go through the engine's writes, which follow a link. `Core` answers them,
  and its watcher reports other files as `Event::FileChanged`. Also here (#193,
  docs/extensions.md "Deleting, restoring and making folders"): `workspace/stat` (the `rev` of
  any file, or of a folder's listing, which leaves out hidden entries and is bounded),
  `workspace/delete` (to the trash, refused unless still that `rev`; answers a random token that
  only the client it was given to can use, kept in the core's `Trash` and gone with the core),
  `workspace/restore` (never over something; `to` is a new name) and `workspace/create-folder`.
  `workspace/kept` lists the texts kept outside their files (docs/extensions.md, "Kept texts,
  as built"); `read`, `stat`, `delete` and `restore` take such a hidden path
  (`normal_or_kept`), nothing else does, and the host asks `files.read` for one
  (`names_kept`), never its suffix's owner.
  `new_path` is the one check of where a new file or folder goes (`normal`, then
  `rename::check_path`, then `resolve`: inside the workspace, no symlink on the way), and a
  `NewPath` only it makes: `workspace/create`, `create-folder`, the menu's new note, the
  board picker's new board, `note.edit` of a note not there yet, and the switcher's create, a
  link followed to nowhere and tidy's create (one `Home::create_named_note`, through
  `files::new_note`), write only at one (#411, #421, #422). `files::new_file` names a new
  `name.extension` (`resolver::new_file_path`), checking each candidate before it asks
  whether one is there, and refuses Obsidian's `newFileFolderPath` naming it.
  `write::create` and `create_if_missing` themselves still take any path, until #448. `stat` and `delete` take a symlink as the entry itself, and the link is what goes; no
  folder on the way to it may be one (`resolve_entry`). A
  frontend that drops a core carries its own deletes to the next one with `Core::take_trashed` /
  `keep_trashed`. The core reads what moved at once and emits `Event::Deleted`, `Restored` or
  `FolderCreated` naming the client. The next groups of #193 (rename, merge, recipes, undo)
  follow the pattern that section ends with. cold-lava doesn't read or write Obsidian's saved
  list (`.obsidian/bookmarks.json`, removed in #373): a bookmark in cold-lava's story is the
  anchor on a passage (#190), and a saved list is for an extension to offer.
- `tasks_settings.rs`: Obsidian Tasks settings (statuses, global filter), a leaf that `items`,
  `tasks`, `style` and the owners read (`tasks.rs` re-exports it for `matcher.rs`, #343).
- `tasks.rs`, `daily.rs`: `agenda` and `toggle` on items of the built-in `task` type
  (`types/task.toml`, read by `Task::of`); Daily notes settings, moment.js date formats.
- `shapes.rs` (paused, see `crates/canvas`). Shape definitions (docs/shapes.md): `[[shapes]]`
  and `[glyphs]` from the built-in file (`shapes/built-in.toml`: text, rect, circle, line,
  arrow, card, sticky, table, venn2, the glyphs), extensions' manifests, config.toml and
  `.cold-lava/shapes.toml`, in the order types load (`Origin`, `origin.rs`). Checked at load, a
  broken one dropped with its error; each is built into a `cold_lava_canvas::Shape` (nested
  shapes flattened, anchors resolved, glyphs looked up). Built-ins are TOML: add a shape by
  writing it there, never a match arm; extend the format when it can't say something.
  `cold-lava shapes` lists and `--render`s them. `shapes/file.rs` writes one shape back as its `[[shapes]]`
  block, keeping the rest of the file as text and refusing a file changed since it was read;
  `with_draft` loads an unsaved one.
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
- `configured.rs`: `Configured`, what a frontend configured for the workspaces it opens (the
  flavour, `[style]`, the layers of types, attribute readings and shapes before a workspace's
  own). Data only, handed to `Core::open` and `workspace::scan`; each part's reader takes it
  (`markdown::flavour_for`, `Style::of`, `types::for_workspace`, `attributes::for_workspace`,
  `shapes::for_workspace`).
- `paths.rs`: the user's home and config folders.
- `rel_path.rs`: a path as the workspace names it (relative to its root, `/` between
  folders). A leaf. The menu asks it; the engine's and the binary's other copies of the rule
  are listed on #365.
- `identity.rs`: the hash every id and rev is made with (`fnv`, FNV-1a; `hash`, as 16 hex
  digits), the last step of every id (`id`: the file, the kind and the key with a NUL between
  each two, hashed), a rev of text (`line_id`) and a twin's (`twin_rev`, `items/twins.rs`: of
  its lines, how many twins there are and, under a twin, that twin's rev). A leaf. Each owner
  makes its own key: a note's items in `items::identify` (a typed item's in `matcher.rs`), an
  owner's pieces and typed items from the place it gave. Changing a key moves ids as surely as
  changing `identity.rs`, and either is the owner's decision; its tests pin ids given before. A
  note's key moves with the Markdown walk into its owner in step D-3 (#272).
- `bounded.rs`: the types another program's answer is held in (`file/read`, `file/edit`,
  `shape/region`, `shape/gesture`): `Text`, `List`, `Fields` and `Pairs`, each naming its
  bound, and the table of bounds, one type each (the trait `Limit`; the owner's limits of
  2026-11-13: `Place`, `Target`, the names, `PieceText`, `ValueText`, `LinkText`, `Cells`,
  `RegionItems`, `GestureSteps`, `StepName`, `StepValue`, …; `Message` where what a field
  comes from bounds it, 64 MiB for a message, and no other bound may have its number, a
  test says). The type names the bound and doesn't hold it: a value read from an answer
  is refused past its bound as it is read, in the bound's words (`Limit::past`;
  `ON_ARRIVAL` false for the ones the answer's reader applies, and `WithAnswer<B>` for a
  link to a file's), and nothing is checked when
  a value is built; a reader applies a bound to a value it didn't read by asking the field
  (`fits`, `most`, `past`), as `Piece::check` does. They serialize as what they hold, so
  revs don't move. A leaf: the reader's words are `owner/answer.rs` (`said`, the one place
  serde's words are judged safe to show, cut to 200 characters, which the host's `unread`
  asks; `LeftOut`), `serde_words.rs` (the leaf rule for serde's forms) and `problem.rs`
  (`capped`, the first 100 problems; `cut`). A field of an answer's
  type that holds a bare collection or JSON value, a slice, or a type the scan can't follow
  fails `tests/one_door.rs`, which follows every type an answer is read into; a new field
  names its bound.
- `problem.rs`: `Problem`, a refusal or failure as the lines it is said on (one at least),
  split only where the app's own text or the TOML parser's layout breaks it. Pick a
  problem out by `about`, never by its words; `joined`, `one_line` (and `--json`) say it
  on one line, `headline` on a status line. Words as written, no terminal: `say`
  (`say::problem_lines`) and `Notice` show them.

The engine never reads `config.toml`: `config.rs` in the binary parses it and builds the
engine's `configured::Configured` (the flavour, `[style]`, and the extensions' and the file's
types, attribute readings and shapes), handed to every `Core::open` and `workspace::scan` the app
makes. A workspace keeps it (`Workspace::configured`, `Core::configured`) and its
item settings and style are read with it, so two cores in one process read their own (#180).
No engine setting is the process's.

**The extension host** (`crates/host/src`, `cold-lava-host`; step 3 of `docs/extensions.md`).
Every frontend runs extensions through it, so the permission check exists once. Don't write
another host or check in a frontend:

- `lib.rs` names the five modules' items at the crate's root (`cold_lava_host::Host`, …).
  `extension.rs`: `Extension` (one method per host→extension JSON-RPC method). `registry.rs`:
  `Registry` (loaded extensions and their manifests, who owns which kind of file, who shows
  which notes, what was left out; `call`, `ask_shape`, `screen`; `load` takes the frontend's
  built-in extensions and the folder of installed ones). `host.rs`: `Host` (one method per
  extension→host method; it queues `Effect`s and checks `Permission`s). `manifest.rs`: the
  manifest types (`Manifest`, `FileType`, `Shows`, `Spec`, `CommandSpec`, `Permission`) and
  their rules (`namespaced`, `claimable`, `leave_out_definitions`). `protocol.rs`: `Effect`s
  and the other messages as data. Three rules:
  - An extension can only do what a message can say: every type that crosses derives serde.
  - Extensions never see keystrokes and never draw: the core owns, draws, resizes and types
    into panes, and draws the lists and tables they send (`view/open`), telling them what the
    user did as `view/event`.
  - Side effects are queued: `Host` collects `Effect`s that `Home` applies after the call
    returns, so extensions never hold `&mut Home`. The gateway's `items/*` and `types/list` are
    answered during the call by the core, after `Host` checks the manifest's `permissions`
    (`items.read` by default, `items.edit`; `files.read` and `files.write` for whole files,
    though `files.write` never reaches a kind another owner reads, `.canvas` included (#323);
    `files.delete` for `workspace/delete` and `workspace/restore`, which `files.write` and
    owning a kind don't include). The reader's `x` uses the same `Host`, as
    `Registry::screen`; the agenda and the table are built-in `Extension`s.
- `process.rs`, `transport.rs`, `rpc.rs`, `frame.rs`, `installed.rs`: extensions in their own
  process. `installed` reads `~/.config/cold-lava/extensions/*/extension.toml`; `Process` is the
  `Extension` for one, starting it on `Extension::workspace` and answering it in
  `Extension::poll` (once a frame) through `rpc::serve` and `Host`, so the permission gate holds;
  `Transport`/`Connector` (stdio, threads behind channels, so the UI never waits) carry
  LSP-framed JSON-RPC (`frame`). The examples are `examples/extensions/open-tasks` and
  `csv-table` (Python), each with its own copy of `cold_lava.py` (a test keeps them alike).
- File types: a manifest's `[[file-types]]` (`FileType`) claims a kind of file; `Registry` gives
  each kind to the first claimant (`owner_of`; never `md` or `canvas`). The tree, the switcher
  and links hand an owned file to its owner as `file/open`, and the core's
  `FileChanged` reaches it as `file/changed`. `Host` lets an owner read and write its own kinds;
  other files need `files.read`/`files.write`. The example is `examples/extensions/csv-table`:
  it gives each row as a `row` piece with plain cells, a `Name (link)` column as link runs
  and a `Name (markdown)` column as Markdown pieces, and writes a change into the one cell
  it names (never the whole file again). `docs/extensions-guide.md` uses it and the chores
  owner (`crates/cold-lava/tests/chores-owner`) as its two worked examples.
- The view gateway's messages (`Content` is a list or a table with a `kind`, `Event`,
  `ViewNews`) and its bounds are `cold-lava-views`' `view.rs`, named in the host as `view`.
- `tests/permissions.rs` runs the `open-tasks` example with and without `items.edit` and the
  file permissions in-process, with no terminal. `examples/headless.rs` is the least a
  frontend writes to host extensions (open a `Core`, `Registry::load`, tell them the
  workspace, each frame hand them the core's news and apply or ignore their `Effect`s): a
  GUI, or any other frontend, would start from it.

**Extensions in the terminal UI** (`crates/cold-lava/src/ext.rs`, `ext/`): what only this
frontend has. `crate::ext::…` names the host's items as before (`ext::Host`, `ext::view`).

- `ext.rs`: the menu's `Registry`, the host's (it derefs to it) with the editor and the menu's
  command table: the core's commands plus the extensions', built by `commands::menu`; keys,
  palette, help, `cold-lava keys` and config all read it. `Registry::load` names the built-in
  extensions. `home.rs` applies the `Effect`s.
- `ext/nvim.rs`: Neovim as the editor extension: `VIM_SCRIPT`, what each OSC message means, the
  commands it types into its pane.
- `kept_ext.rs`: the kept texts as a built-in extension (`kept`): their count on the status
  line while there are any, and `kept.open` to list, open and put them away.
- `views.rs` keeps the views extensions opened: drawn with `ListView`/`TableView`, updated
  once a frame, places in history while open.

**Features / UI** (`crates/cold-lava/src`):

- `home.rs`: the main menu (workspaces, note tree, the right pane with the reader or an extension's
  program, backlinks, switcher, search). It applies the extensions' `Effect`s. It deletes,
  restores and makes folders by asking the core as `MENU` (`Core::delete_file`,
  `restore_file`, `create_folder`), never `trash::delete`, `trash::restore` or
  `write::create_dir`, and makes new notes through `Core::create_file` (the new-note
  prompt, the switcher's create, a link followed to nowhere, tidy's create, `note.edit`),
  never `write::create`. It doesn't rescan afterwards (`after_core_change`): the core read
  what moved. `tests/one_door.rs` reads every file under `src/` and lists the engine
  writes, the whole-workspace operations and the `std::fs` changes the app still makes
  itself (#193), and what else still goes around the door (#270): reads of `core.index` (the
  host's too), a workspace scanned or indexed beside the core, item and tag edits around
  `Core::edit_item`, links read by a pattern, and what the built-in extensions use beyond
  the gateway. The lists only shrink: a new use fails, and so does an entry that is gone
  and still listed.
- `palette.rs`, `outline.rs`: the menu's first overlays, each a module that owns what is typed
  and which row is chosen, with `key` (giving its own `Action`) and `draw`. `Palette` runs a
  menu command by name; `Home` hands it the commands it may run now and runs the one picked.
  `Outline` jumps to one of the reader's headings (`Outline::open` gives none for a note
  without). `Home::palette_key` is how the menu keys an overlay: what it needs from the menu
  first, then the key in place in `Mode`, no clone.
- `prompt.rs`, `new_note.rs`, `tag_prompt.rs`: the menu's prompts, overlays of the same shape.
  `Prompt` is the one-line prompt (a new folder, a workspace to add, a rename, a move, a marked
  move; `Asks` says which). `NewNote` adds the templates and `TagPrompt` the tags to complete
  from. Each holds the target it opened on (a `prompt::Folder`, `tag_prompt::Whom` for the note
  or the marks, the path) with the workspace's root, and its title names it, so nothing that
  moves the tree's selection, the marks or the workspace under it changes what Enter does: the
  menu refuses when that workspace is no longer open (`Home::is_open`) or a folder is gone
  (`can_create_in`), and never makes a target again. Each says what closing it would lose
  (`unsaved`).
- `line_input.rs`, `pick_list.rs`, `popup.rs`, `words.rs`: leaves the menu and its overlays
  call: what a key does to a typed line, to a picked list's query and cursor, and how the
  list's rows are drawn (`pick_list::draw`, from the pieces each caller writes: the rows
  shown around the chosen one, its colours, the characters a match hit), where a popup goes
  and its frame, and `plural` (the app's one "1 note" / "2 notes").
- `recipe_view.rs`: the palette's `recipe.run` dialog (pick, options, plan with diffs); `home.rs`
  plans and undoes through `recipe.rs`, and the core makes it (`Core::apply`), as a merge.
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
  `view/open`, followed with `links/subscribe`. `app.rs` holds the colours.
- `board.rs`: whiteboard on Obsidian Canvas (JSON Canvas) files (read and saved with the
  engine's `json_canvas`; their items and links are the canvas owner's), and placed shapes on it:
  `board/placed.rs` is how a placed shape is stored on a node (the `cold-lava` field, which node
  type it is, the guard against a save that drops the field), `board/live.rs` binds each one
  through the core and paints it with the canvas engine. The board is a mode of the menu
  (`Mode::Board`, `Home::board`, `Place::Board`): the menu keeps the core and the registry,
  hands them to each `Board` call, ticks and draws it in its loop and gives it the core's
  events and the `canvas` item news. It has no file watcher: the core's event for its canvas
  makes it read the file and merge (`merge3`). What extensions ask for in its calls to them
  about their shapes it keeps for the menu to do (`Board::take_asked`). The board picker (`w`,
  `board::Picker`) is a layer too, `Home::picking`, over the menu or the graph. Neither it nor
  the shape editor may get a loop of its own: extensions are answered only in the menu's.
  `cold-lava board` runs the board in a small loop of its own, where what needs the menu is
  refused (`Board::asked_alone`). In memory a node's id is the canvas owner's place for it
  (`parse`, from `canvas::places`), so cards that repeat an id stay apart. A place must never
  be written into the canvas: `as_written` gives every node, and every arrow's ends, the id
  the file had (`Board::twins`), and is the only way a board's `Doc` becomes bytes. A notice that a request was
  refused to keep unsaved work goes through `Board::refused`, not `flash`.
- `table.rs`: how the terminal draws the declarative table view (its data, columns, rows by
  id and events back, is `cold-lava-views`' `table.rs`, serde, so it can cross the view
  gateway). `type_table.rs` is the table as a built-in extension (`Tables`):
  `i` (`table.open`, a core command the menu hands to it) sends the types as a table to pick
  from, then the picked type's table with `view/open`, kept by `items/query` and
  `items/subscribe` (`Feed`, which the agenda uses too); a cell's edit is
  `items/edit set-field`. `build` is also what `cold-lava table` prints.
- `list.rs`: how the terminal draws the declarative list / tree view, whose data is
  `cold-lava-views`' `list.rs` (groups of rows by id with a checkbox, pieces of
  text with looks, a right side; selection, folding, filtering; events back; serde like the
  table). `agenda.rs` is the agenda as a built-in extension: it follows tasks with
  `items/query` and `items/subscribe`, sends its list with `view/open` and ticks with
  `items/edit toggle` (`A`, `agenda.open`, a core command the menu hands to it).
- `shape_editor.rs` (paused, see `crates/canvas`). The shape editor (`S` on the whiteboard, a
  layer over it with no loop of its own: `Board::editor` gets the board's keys and mouse and the
  core's news and is drawn instead of it, so the menu's loop goes on and extensions are answered
  while it is open): draws a shape and saves the same `[[shapes]]` TOML into the workspace's
  `.cold-lava/shapes.toml`. `shape_editor/draft.rs` is the editing on the `Definition` itself
  (grips, bending sides into `curve`s, regions read from what contains a click, anchors, a
  selection made into one shape), with no drawing; the guides are `snap.rs` in the canvas crate;
  the block written back as text, everything else in the file kept byte for byte, is
  `shapes/file.rs` in the engine.
- `main.rs`: CLI. `term.rs`: suspend/editor/Obsidian-URI helpers. `config.rs`: `config.toml`.
- `cells.rs`: the only module that sets a symbol on a screen cell (`cells::put`,
  `put_char`). Text drawn through ratatui (`Span`, `Line`, `Paragraph`, `set_string`) loses
  its control characters there, but a symbol set on a cell is written to the terminal as it
  is, so never call `set_char` or `set_symbol` elsewhere (`tests/one_door.rs` lists them).
  `put` shows `�` for what a terminal would act on and a space for a tab, whoever wrote
  it. A label drawn a character a cell (a graph node's, an arrow's, a placed shape's
  region labels) goes through `cells::label` before it is measured or fitted: escaped at
  `Level::Content`. A shape's own and bound text is not a label and gets the `�`. A cell
  is content: direction characters are kept in it, a shape's text and a pane's cells
  included.
  Text drawn through ratatui still holds U+2028 and U+2029, and direction characters where it is a `Line`
  drawn on its own or a block's title (`cells.rs` pins what ratatui keeps).
- `notice.rs`: `Notice`, the words of a notice on a status line (what the app did, why it
  didn't, an extension's `ui/notify` and `ui/status` words). It keeps them as written and
  its only way out is `Display`, which escapes at `Level::Message`, so the escape happens
  where the status line is drawn and no store can forget it. `Home::msg`, `Board::msg`,
  the scene view's and the shape editor's message, the recipe dialog's refusal, the
  extensions' statuses and `refused::Refused` all hold one; `flash`, `say` and `refused`
  take `impl Into<Notice>`. Never keep a notice as a `String`. A view's title and rows are
  content, not notices.
- `say.rs`: the only module that prints to stdout and stderr. A line is escaped whole as it
  goes out (`cold_lava_views::shown::escape`), a line break in it included, so nothing has
  to be remembered where a line is made. Which entry is called says what the line is:
  `out!` (and `Out::listing`, `listing_block`) for a listing, a line that is the user's own
  content; `told!` (stdout) and `problem!` (stderr) for a message, what a command did or
  why it didn't; `say::fixed` for the app's own `&'static str`; `say::json` for JSON,
  written as it is. One call is one line: what has several lines by the app's own
  structure makes one call a line, or is a `Problem` error (the engine's `problem.rs`; a
  usage after what went wrong), which `main` prints a line each. Never `println!` or take `stdout()` elsewhere: `tests/one_door.rs` lists every
  print site (the host's, the engine's and the views crate's too) and every listing, and
  `tests/outside_text.rs` runs the binary on text with escape sequences planted.
- `drive.rs`: `cold-lava drive`, scripted UI runs (its `write` step changes a workspace file
  with `std::fs` on purpose, as another program would: the one exception `tests/one_door.rs`
  lists for it): the real binary in a pty, screen read back
  with vt100, cursor-position queries answered, the mouse clicked, dragged and scrolled.
  `graph.drive` and `board.drive` pin the graph and the whiteboard.

## Sharing files with other programs

cold-lava works on files that other programs (Obsidian, an editor, git) edit at the same time.
How another product reads them is a profile a workspace can choose, never the core's
definition; Obsidian's reading is the default profile today, until a workspace can choose
another (foundation 7, #235, #236). These rules keep the files safe for every program that
shares them, and the Obsidian ones are that profile's:

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

`CONTRIBUTING.md` is how a change gets merged, and the `ship` skill
(`.claude/skills/ship/SKILL.md`) runs it. In short: one request is one PR, squash-merged; a
refactor a feature needs is its own PR, merged first; the PR title is the line the release
notes are written from; two reviews by two agents other than the author (behaviour, and the
`design-reviewer` agent), where a must-fix from either blocks the merge; merge when CI is
green and the reviews are clean, without asking (unless the owner said to hold it). A pushed branch is never rebased or force-pushed. Releases are
automatic once a PR sets a new `version` (CONTRIBUTING.md, "Releases").

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

- The whole picture is `ARCHITECTURE.md`; the engine's layers and engine vs. frontends are
  `docs/engine.md`; the crate split
  keeps the engine free of ratatui/crossterm. The extension API design is in `docs/extensions.md`. The connection to extensions must stay
  behind the `Transport`/`Connector` abstraction (stdio first; Unix sockets etc. later).
- The direction and the order of work were decided on 2026-11-11 (`docs/engine.md`, "Direction
  and order"): finish the half-done migrations, one door into the engine, `cold-lava lsp`
  (dropped below), types and identity, the terminal views, drawings (paused on 2026-11-20,
  #375), then the notebook. On 2026-11-12 the refactor tracks were put in front of the
  features they underlie. On 2026-11-20 `cold-lava lsp` was dropped from the order (#195): the
  notebook, cold-lava's own editor, comes first, and a language server is not planned.
  **Roadmap issue #100 is the one list of open work**: every open issue is on it exactly once,
  in order. When you open an issue, add it there; an order written anywhere else is history.
- The `Extension` interface exists and Neovim runs on it, and so do the agenda, the table and
  the graph (over the view gateway). The menu, the page and the whiteboard stay built into the
  frontend: they are not meant to become extensions.
- The project's issues are tracked on GitHub. Don't leave a TODO in code or in a doc: open an
  issue and put it on the roadmap.
