# Changelog

All notable changes to cold-lava are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may
change commands, keys and file formats; patch versions only fix things.

## [Unreleased]

### Added

- Extensions in their own process. A folder in `~/.config/cold-lava/extensions/` with an
  `extension.toml` (`name`, `version`, `description`, `run`, `permissions`, `[[commands]]`,
  `[types.*]`) is started when a workspace opens, and spoken to in JSON-RPC 2.0 over its stdin
  and stdout, framed as LSP frames it. It can call the workspace gateway (`items/query`,
  `items/subscribe`, `items/edit`, `types/list`, within its permissions), `ui/notify`,
  `ui/status` (its own text in the status line) and `command/run`, and its commands join the
  palette, keys and `config.toml` like the core's. Nothing waits for it: a crash is reported
  once with the last line of its log (`~/.cache/cold-lava/extensions/NAME.log`) and its
  commands stop, and "restart extensions" (`extensions.restart`) starts it again.
  `cold-lava ext list [WORKSPACE]` prints every extension, its permissions and commands, and
  why any didn't load. Extensions inside a workspace aren't run.
- `examples/extensions/open-tasks`, an example extension in Python with no dependencies: a
  count of open tasks in the status line, kept current as notes change, and a command that moves
  overdue tasks to today. [docs/extensions-guide.md](docs/extensions-guide.md) explains how to
  write one.
- Views from extensions. An extension sends a list or a table with `view/open` (`view/update`
  replaces it, `view/close` closes it), and cold-lava draws it over the main area with its own
  keys, folding, filtering and sorting, and tells the extension what you did as `view/event`
  (Enter, `x` on a checkbox, a key the list declares, a table's edit) and `view/closed` (Esc or
  `q`). `workspace/open` opens a note at a line. An open view is a place in Back's history, kept
  as it was. Views are bounded (50,000 rows, 64 columns, 32 keys, 8 open per extension, updates
  drawn once a frame) and need no permission. The example extension's *list the open tasks by
  note* (`open-tasks.list`) shows one: `x` ticks, `t` sets the due date to today, Enter opens the
  task's note.
- An item that answers to `task` always has its `status` and `status_type` over the workspace
  gateway, even when its type reads neither, so the `status` filter finds such a subtype.

- The item messages of the workspace gateway, in-process (`crates/engine/src/gateway.rs`,
  through `Host`): `items/query` (a kind or type, a filter on folder, tag, status, a date range,
  text or a property, and pages), `items/subscribe` with `items/changed` and `items/reset`,
  `items/unsubscribe`, `items/edit` (`set-field` and `toggle`, checked by `rev` and read back,
  refused with a reason: `changed`, `refused`, `read-back`, `io`, `forbidden`) and `types/list`.
  A manifest's `permissions` say what an extension may do (`items.read` by default,
  `items.edit` to change items), and every edit names the extension that made it on the status
  line. The table view reads and edits its type through them, and ticking in the agenda and
  the reader goes through `items/edit`; neither looks or behaves differently.

- The agenda screen. `A` (`agenda.open`) lists the workspace's tasks (anything that answers to
  `task`, subtypes too) by day: Overdue, Today, each of the next seven days, Later, and Undated
  (folded) last, filed by the date `cold-lava agenda` uses (due, scheduled, start, else the
  daily note's). Rows show the checkbox, the words with links as text, `🔁`, the priority chip
  and the type's looks, the date and the note. `x` ticks a task through `tasks::toggle` (refused
  with the reason when it changed elsewhere), `Enter` opens its note at the task and `Ctrl-o`
  comes back, `h` / `l` / `Tab` fold groups, `/` filters, `d` shows done tasks. Tasks closed
  today stay in place, dimmed. The agenda follows changes on disk, keeping the selection.
- The declarative list view (`crates/cold-lava/src/list.rs`): groups of rows by id, with a
  checkbox, text pieces with looks, a right side and nesting depth; selection, folding and
  filtering in the view; `activated`, `toggled` and the sender's own key commands back. Serde
  like the table, shaped to cross the view gateway.
- `x` in the reader ticks a task the way `task toggle` does (done and cancelled dates, the next
  occurrence of a recurring task, custom statuses), refused with the reason on the status line
  when the task changed since the note was read. It ticks the task the selected link is in, else
  the one on the highlighted line, else the first task on screen below it. On the last page of
  a note, or in a note shorter than the pane, `j` / `k` move the highlighted line instead of
  doing nothing, so every task can be reached. The reader keeps its place.
- `COLD_LAVA_TODAY=YYYY-MM-DD` pins today for the agenda, `task toggle` and ticking, for
  scripted runs and tests.

- The table view. `i` (`table.open`) picks a type, listed with how many items each has, and
  shows its items across the workspace as a table: a column per field in the type's order, then the
  note, values drawn with the type's looks. `s` / `S` sort by the selected column (numbers,
  dates, enums in their declared order, text with accents ignored; empty values last), `/`
  filters rows, `Enter` opens the item's note at its line (`Ctrl-o` comes back to the table as
  it was), and `e` edits a value through `edit::set_field`: refused with the reason on the
  status line when it doesn't fit, the field is read-only or the item changed elsewhere (the
  table is read again then). The table follows changes on disk, keeping the selected row; wide
  tables scroll sideways. The table itself is a declarative view (`crates/cold-lava/src/table.rs`:
  columns with a value type and a look, rows by id, events back), shaped to cross the view
  gateway. `cold-lava table WORKSPACE TYPE [--sort FIELD] [--json]` prints the same rows.
- The reader draws typed values with their types' looks: `badge` as a chip, `icon`, a colour by
  name (one table in the frontend, so a light terminal can swap it), `dim`, `strike`, `bold`,
  `hide`, `link` (the value becomes a link `Tab` and `Enter` follow), `whole` (the whole item)
  and `by-value`. The note's typed frontmatter fields with a look go on a line above it. A look
  is skipped rather than drawn in the wrong place when its value can't be found exactly in the
  rendered text. The built-in task fades and strikes through done and cancelled tasks and puts
  its priority on a chip coloured by urgency; the example workspace's `vei` chip covers the whole
  `VEI 6`. With `NO_COLOR`, chips are reverse video and colours are left out. `by-value` on a
  `${tasks.statuses}` field takes the status types (`done`, `cancelled`, …).
- Inline types: a type with `on = "inline"` is found by its `match` anywhere in the text of
  paragraphs, list items (tasks too), headings and table rows, any number of times per block,
  and never in code spans, code blocks, math or frontmatter. Each match is an item of that type
  with its `parent` the block it sits in, its fields read from the pattern's groups, the
  section's heading or the note; `where` takes the note's folder, properties and tags, the
  section, and `inside` (the block's primitive or kind, such as `task`). `cold-lava items WORKSPACE
  TYPE` lists them and `cold-lava types` counts them. The example workspace's `vei` type finds a VEI
  written in prose (`its 1883 eruption (VEI 6, …)`).
- `cold-lava item set FILE ID FIELD VALUE` (`--remove`, `--rev REV`, `--dry-run`, `--json`)
  sets one field of a typed item through the source it is read from: a marker, a Dataview field
  (brackets kept), a flag or flags glyph, a table cell (not re-padded), a frontmatter property
  (line by line; nested YAML refused) or a pattern's group. A missing value is added the way the
  line writes its others, and `--remove` takes one out. The note is read again in memory first,
  and nothing is written unless the item reads back exactly the new value with nothing else in
  the note changed. A stale rev exits with code 3. The engine's `edit::set_field` does it, for
  the coming `items/edit set-field`.
- The task is a type: `crates/engine/types/task.toml` defines `task` in the same TOML as
  anyone's types, with the workspace's Tasks plugin statuses and global filter filled in, and every
  task is read through it; `cold-lava types` lists it. `cold-lava items`, `agenda` and `task
  toggle` read the same tasks with the same ids, and `agenda --json` and `task toggle` print the
  same as before. A type that `extends = "task"` (an `idea` with `where.tag = ["idea"]`) takes
  the tasks it matches, still answers to `task`, is in the agenda and is ticked by `task
  toggle`.
- `task toggle` follows the Tasks plugin's custom statuses: the status goes to its
  `nextStatusSymbol` (` ` → `/` → `x`, say), with the done date when the next one is done.
- `cold-lava item set` works on tasks: `status " "` unticks through the checkbox (a space is a
  status), `status_type done` writes the workspace's done symbol, and dates, priority and recurrence
  go through their markers or Dataview fields. A field read from the same text (a status's type)
  may change with the one set.
- Types can read a task's checkbox (`checkbox = true`, a space included), leave strings out of
  the rest of the text (`rest = { without = ["#task"] }`), read the date of the daily note they
  are in (`note = "date"`), take an enum's values from `${tasks.statuses}`, and name fields a
  block must have (`required = ["text"]`; without one, the next type that matches takes it).
  `${tasks.global-filter}`, `${tasks.statuses}`, `${tasks.format}` and `${daily.format}` in a
  pattern now stand for the workspace's settings. A marker reads past an emoji's variation selector
  (`⏳️ 2026-11-05`), and a `flags` source leaves every one of its glyphs out of the rest.

### Changed

- The agenda (`A`) is a built-in extension: it reads its tasks through `items/query` and
  `items/subscribe`, ticks through `items/edit` and shows its list through `view/open`, the path
  an extension in Python takes. It looks and works as before. `cold-lava ext list` names it.
- cold-lava calls a folder of notes a **workspace** now, not a vault: in the menu, the status
  line, `--help` (`cold-lava items WORKSPACE KIND`) and the docs. "Vault" is left for Obsidian's
  own. The commands `vault.add`, `vault.rescan` and `vault.tidy` are now `workspace.add`,
  `workspace.rescan` and `workspace.tidy`; keys set under the old names in `config.toml` still
  apply, with a warning to rename them (not an error: `cold-lava keys` still exits 0). `cold-lava agenda --json` names the folder `"workspace"`
  instead of `"vault"`, `cold-lava types --json` gives a type from `.cold-lava/types.toml` the
  origin `{"kind": "workspace"}` instead of `"vault"`, and the extension message
  `vault/saved` is now `workspace/saved`. The list of opened workspaces moves from `vaults.json` to
  `workspaces.json` in the config folder; the old file is read once and left in place. The
  example is now `example-workspace`.
- `item set` (and editing in a table) adds a date a task doesn't have the way its line writes its
  other fields, else in the workspace's Tasks plugin format: `📅 2026-12-01` in an emoji workspace, where
  it used to add `[due:: 2026-12-01]` everywhere.
- Tasks in `cold-lava items --json` list their tags without `#` (`"tags": ["home"]`), as every
  type's tags are; the other keys, their order and the ids stay.
- Tasks are read as the Tasks plugin and the workspace read them where the old reading differed:
  nothing in a code span is a date or a tag; of a field written twice (`📅 a 📅 b`) the first
  counts and both leave the words; an unbracketed `due:: x` in the middle of a task's line is
  words (Dataview reads it only at a line's start); `#2026` is no tag and `(#paren)` is.
- Type matching: an enum's written form that isn't in its table doesn't parse (`null`, counted
  by `cold-lava types`) instead of reading `other`; an unbracketed Dataview field (`key:: value`)
  is read only at the start of a line; no source reads in code spans or math; every source of a
  field, and every occurrence of a marker or Dataview field, is left out of the rest of the text;
  a type without a `priority` has its base's.
- The built-in task has `priority = 1`, so a type on list items takes a task only when it
  extends `task` or sets a higher priority (before, tasks were never typed).
- Removing a marker with an emoji variation selector (`⏳️ 2026-11-05`) through `item set`
  takes the selector too.
- A typed item whose shape has a `key` with a text field is identified by that text as written,
  not as a JSON string, as tasks are; such items get a new id once.

### Fixed

- A custom status whose next status is cancelled (`nextStatusSymbol`) now gets the cancelled date
  (`❌ 2026-11-07`, or `[cancelled:: …]` in the Dataview format) and loses its done date, as the
  Tasks plugin does; `setCancelledDate: false` leaves it out.
- The agenda follows the day: left open past midnight, it files tasks by the new day (Today,
  Tomorrow, Overdue) instead of keeping yesterday's groups until a task changed.

## [0.3.0] - 2026-11-07

### Added

- Tidying a vault. `T` (`vault.tidy`) lists orphan notes (no links in or out; `a` adds notes
  only without backlinks, daily notes are grouped last), attachments no note, embed or
  whiteboard links to, empty folders (not the ones the vault's settings name), and unresolved
  links by target with where each is. `space` marks rows, `d` deletes the marked or selected
  ones to the trash after one prompt that lists each, `u` undoes the whole batch, `Enter` opens
  the note, link, attachment or folder, and `n` creates the missing note for an unresolved link.
  `cold-lava tidy VAULT_DIR [--json] [--no-backlinks]` prints the same, read-only.

- Duplicating and merging notes. `D` (`note.duplicate`) copies the selected note or whiteboard
  to `Name 1` beside it (Obsidian's naming) and opens it. `M` (`note.merge`) merges the
  selected note into another picked in the switcher, like Obsidian's Note composer: its text is
  appended after a blank line, its frontmatter merged line by line (missing keys copied, lists
  such as `tags` and `aliases` joined, conflicting values kept as the target has them and listed),
  its name added to the target's aliases, every link to it rewritten to the target in its own
  style (headings, embeds, Markdown and relative links, frontmatter, whiteboard cards,
  bookmarks), and it goes to the trash. A preview shows all of it first; nothing is written if a
  file changed since. `u` undoes the whole merge.

- Bookmarks, shared with Obsidian: they live in Obsidian's own `.obsidian/bookmarks.json`, so
  a bookmark made in either app shows in both, and a change Obsidian makes shows at once. `B`
  (`note.bookmark`) bookmarks the selected note or folder, or in the reader the heading you are
  in; `Ctrl-b` (`search.bookmark`) in the search bookmarks what is typed; again removes it. `'`
  (`bookmarks.open`) lists them with Obsidian's groups (`←`/`→` fold), type to filter, `Enter`
  opens one (a note at its heading or block, a folder in the tree, a search, the graph, a board,
  a link) and Back returns, `Del` removes one. Bookmarked notes and folders get a `★` in the tree.
  Renaming or moving a note or folder moves its bookmarks with it; a deleted note's bookmark is
  kept, like Obsidian keeps it, and shown as missing. Items and fields cold-lava doesn't know are
  written back as they were, in their order.

- Several notes at once: `space` (`note.mark`) marks notes and folders in the tree, shown with a
  `✓`, and `Esc` clears the marks. With marks, `m` moves them all into one folder as one plan
  (one preview, links between the moved notes kept right, refused if two would land on the same
  path or a folder would move into a marked one), `d` deletes them all to the trash after one
  prompt and `u` brings the whole batch back. `#` (`note.tag`) adds a tag to the marked notes'
  frontmatter, or the selected note's, completing from the vault's tags; `-tag` removes it. The
  `tags` list keeps its style (`[a, b]` or `- a` lines), only that line changes, and a note
  whose `tags` isn't a plain list is left alone and named.

- Folders in the main menu: `R` renames the selected folder and `m` moves it into another, with
  every link into it, out of it and inside it kept correct (by path, by name, relative Markdown
  links, attachments, frontmatter, whiteboard cards), shown in a preview first. A `[[Name]]`
  link becomes a path only when the move would make it lead to another note. `d` deletes a
  folder to the trash after saying how many notes it holds and which links into it will break,
  and `u` puts it back. `N` (`note.new-folder`) makes a new folder in the selected one. Neovim
  follows a note whose folder moved and drops one whose folder was deleted, and Back/Forward
  follow moved notes. `cold-lava rename VAULT FOLDER NEW` renames or moves a folder too.

- `cold-lava drive SCRIPT -- ARGS` runs cold-lava in a hidden terminal, types keys from a script,
  waits for text and checks the screen (`wait-for`, `expect`, `exits 0`, …). A failing step
  names its line and prints the screen. It drives the real binary, embedded Neovim included, with an empty home folder so
  it doesn't read your config or add to your list of vaults (`--real-home` to use yours).
  After a lone Esc it waits for the program to answer before typing on, so `keys esc q` isn't
  read as Alt-q on a slow machine.
- `COLD_LAVA_EDITOR=none` opens notes in the reader even when Neovim is installed.
- Link hints: `f` (`note.hints`) in the reader, or in the menu while the reader shows a note,
  labels every link on screen and every backlink in view with one or two letters from the home
  row. Typing a label follows that link the way `Enter` does (Back returns), `Esc` or a letter
  no label has cancels, and after the first letter of a pair only the labels it starts stay.
- Outline: `o` (`note.outline`) in the reader, or in the menu while the reader shows a note,
  lists the note's headings indented by level, with the one you're reading marked. Type to
  filter (fuzzy, accents ignored), `↑↓` or `Ctrl-n`/`Ctrl-p` to choose, `Enter` to jump there
  (Back returns to where you were in the note), `Esc` to close.
- Peek: `K` (`note.peek`) on the reader's selected link shows where it leads in a popup under
  the link, without going there: the note, drawn by the reader in the vault's flavour and
  scrolled to the link's `#heading` or `#^block`, sized to its text up to 40% of the screen.
  The reader's keys scroll it, `Tab` / `Shift-Tab` move to the next or previous link and the
  peek follows, `Enter` goes there (Back returns), `Esc` or `K` closes it. A missing note says
  `Enter` creates it, a URL says `Enter` opens it in the browser, and a link to an attachment
  names its type. On a backlink, `K` peeks at the linking note at the line with the link, and
  `Tab` sweeps through the backlinks.
- The reader's title shows where you are in the note: `Concepts/Lava › Temperature`, the
  headings above the top line, cut from the left with `…` when too long. A first heading that
  only repeats the note's name is left out.
- `]` / `[` (`reader.next-heading`, `reader.previous-heading`) jump to the next heading, and to
  the start of the section you're in or the heading before it.
- Back and forward, like a browser's, across the menu, the reader, search, the graph and
  whiteboards: `Ctrl-o`, `Alt-←` or `Backspace` (`history.back`) returns to where you were
  before following a link or opening a note, a search result, the graph or a board, with the
  reader's scroll position and selected link or the search's words; `Alt-→` (`history.forward`)
  goes forward again. The status line shows where they lead (`Ctrl-o ⟵ Lava`).
- `cold-lava items VAULT KIND [--json]` lists the vault's tasks, list items, headings or blocks
  with a `^id`, one per line (`note:line` and a short form), or as JSON lines with each item's
  id, line range, section, parent, block id and data. A task's id stays the same when it is
  ticked, re-dated, re-indented or moved down by lines added above it.
- Types, the first step: `[types.NAME]` in `config.toml` and `.cold-lava/types.toml` in a vault
  (starting with `version = 1`) define what notes, table rows, list items and other blocks mean:
  fields with value types, where each is read, and how it looks. `cold-lava types [VAULT]
  [--json]` lists every type with where it comes from, its shapes and fields, and whether each
  field can be written back, then one line per mistake (the broken type is left out, the rest
  load, exit code 1). The example vault defines `volcano` notes and `eruption` table rows.
- Types find their notes and blocks: `cold-lava items VAULT TYPE` lists the notes, table rows,
  list items, headings or paragraphs a type matches, with every field of the type read and
  parsed (`null` when missing or when it doesn't read as the field's type), and `cold-lava
  types VAULT` says how many items each type found and how many values didn't parse. One type
  wins per note and per block (priority, then subtype, then the more specific shape, then
  origin, then name); a type that `extends` another is also listed under it. Tasks stay tasks.
- Which key: `,` (`app.keys`, `graph.keys`, `board.keys`) opens a popup at the bottom of the
  menu, the reader, the graph or a whiteboard that lists the commands you can run there now,
  grouped (move, go, links, note, view, app, extensions), each with its key and a short name.
  Typing a listed key runs that command, `Esc` closes it. The groups and names live in the
  command tables, so the popup follows `[keys]` in config.toml. `?` still opens the full help.
- Delete a note: `d` (`note.delete`) in the menu, or in the reader, asks first and lists the
  notes that link to it, with the line, since their links will become unresolved (they aren't
  rewritten, as in Obsidian). `y` deletes, `n` cancels. Whiteboards can be deleted the same way;
  folders are refused for now. Nothing is deleted for good: the note goes where the vault's
  Obsidian setting (`trashOption` in `.obsidian/app.json`) says, the system trash by default
  (the freedesktop trash on Linux, `~/.Trash` on macOS) or the vault's `.trash` folder for
  "local". A vault set to delete permanently gets `.trash` too, and the status line says so. If
  the note changed on disk after the prompt opened, it asks again with the new version. A note
  open in Neovim is saved first and its buffer dropped.
- Undo a delete: `u` (`note.undo-delete`) puts the last deleted note back where it was (up to
  20, this session). If something new is at that path, it offers a free name (`Lava 1.md`).
- `cold-lava drive --home DIR` runs the program with DIR as its home and keeps it afterwards,
  so a test can look at what the program left there (the system trash, for one).
- New notes from templates: if the vault has templates (the folder in Obsidian's core Templates
  settings, `.obsidian/templates.json`, or Templater's templates folder), `n` lists them under the
  name prompt, with "No template" first. `Tab` / `Shift-Tab` (or `↓`/`↑`) picks one. The note is
  filled the way Obsidian fills it: `{{title}}`, `{{date}}`, `{{time}}` (in the settings' formats,
  `YYYY-MM-DD` and `HH:mm` by default), `{{date:FORMAT}}` and `{{time:FORMAT}}` (moment.js).
  Other `{{…}}` text and Templater's `<% … %>` are kept as written.
- The example vault has a templates folder set up, with a new **Volcano** template.

### Changed

- `cold-lava rename --json`: the plan lists what moves as `moves: [{from, to, folder}]` instead
  of `from`, `to` and `folder` at the top, since a plan can now hold several moves.
- `Esc` (`filter.clear`) clears the tree's marks first, then the filter.
- Notes in the templates folders aren't read as items: a template's tasks and typed blocks no
  longer show up in the agenda or `cold-lava items`.
- The status line shows only a few keys and `, keys` / `? help`; the reader's no longer runs
  past the edge of the screen.
- `note.open-in-obsidian` moved from `o` to `O`: `o` opens the outline, which you use far more
  often while reading. To get the old key back, give the outline another key too (two commands
  on one key both keep their defaults): `"note.open-in-obsidian" = "o"` and
  `"note.outline" = "ctrl+l"`. The graph keeps `o` for Obsidian.
- `Ctrl-o` is now Back (`history.back`) instead of a second key for the quick switcher, which
  keeps `p`. To get the old key back: `"note.switch" = ["p", "ctrl+o"]` and
  `"history.back" = ["alt+left", "backspace"]` in `[keys]`.
- Building from source is now `cargo install --path crates/cold-lava`: the repository is a
  Cargo workspace, with the engine as its own crate (`cold-lava-engine`) beside the terminal UI.
- `agenda` and `task toggle` read tasks the way Obsidian and the Tasks plugin do, from the same
  parse as the rest of cold-lava:
  - Tasks in callouts and quotes (`> - [ ] …`) are listed, and `task toggle` ticks them.
  - Tasks in `~~~` code blocks, indented code blocks, `%%comments%%`, or text that only looks
    like a list (an indented line continuing a paragraph) are no longer listed.
  - A task's text leaves out a trailing `^block-id` and inline `%%comments%%`.
  - Custom statuses follow the Tasks plugin's status settings (`[?]` set to "todo" counts as
    open).
  - A malformed date (`📅 2026-13-45`) is ignored, so the task is filed under its next date
    instead of being left out.
  - Notes under `node_modules` are skipped, as everywhere else in cold-lava.
- `agenda` is about six times faster on large vaults: the daily-note date format is compiled once
  instead of once per note.

### Fixed

- Markdown links with percent-encoded names, the way Obsidian writes non-ASCII names
  (`[x](P%C4%81hoehoe.md)`), now lead to their note or attachment: in backlinks, the reader,
  the peek and following links. Before, only `%20` was decoded. Renaming, moving and merging now
  update these links too, and keep how they were written: a link that encoded non-ASCII
  characters gets the new name encoded, one that only encoded spaces gets only spaces encoded.
- Opening a note at a heading, block or line (a link, a search result, a bookmark) in a note
  short enough not to scroll no longer loses where it landed: the reader clamped the jump to a
  page height it didn't know yet, so it fell back to the top and the breadcrumb lost the heading.
- The menu no longer crashes when the last row of the note tree disappears (deleted here or in
  Obsidian): the selection was read through rows of the old tree.
- Following a link to an attachment (`[[map.pdf]]`, `[map](Maps/map.pdf)`) no longer creates a
  note named `map.pdf.md`. The file opens in the system's app (`xdg-open`, `open` on macOS) when
  it's in the vault, found the way Obsidian finds it: by path, next to the note, then by name
  anywhere; a link to a canvas opens it in the whiteboard. A missing attachment says
  `map.pdf isn't in the vault` and nothing is created. Notes with dots in their names
  (`Mount St. Helens`) are still notes: only known attachment types (images, audio, video, PDF,
  canvas, base) count. The peek says whether `Enter` opens the attachment or it's missing.
- The whiteboard no longer overwrites a canvas file changed outside cold-lava between its merge
  and its save, or one it couldn't read (for example while another app was halfway through
  writing it). Your edits are kept and saved once the file can be read, and quitting first warns
  that they aren't saved yet.

## [0.2.0] - 2026-11-06

### Added

- `cold-lava rename VAULT NOTE NEW` renames or moves a note and updates every link to it, like
  Obsidian's "Automatically update internal links": `[[wikilinks]]` (with aliases, headings,
  blocks and embeds), Markdown links (relative or vault paths), links in frontmatter, and
  `.canvas` note cards and text cards. Each link keeps its style; the moved note's own relative
  links are fixed; code is left alone. `--dry-run` shows the changes first, and nothing is
  written if a file changed after the plan was made.
- In the main menu, `R` renames the selected note and `m` moves it to another folder, with a
  preview of every link that will change before you confirm. The open note is saved first, and
  Neovim shows it under its new name afterwards.
- Command palette (`:`) in the main menu: run any command by name, with its key shown.
- Keys are configurable in `~/.config/cold-lava/config.toml` (`[keys]`, by command id);
  `cold-lava keys` lists every command and its keys and reports mistakes in the file.
- Markdown flavours (`commonmark`, `github`, `obsidian`, or your own in `config.toml`) for the
  reader.
- The reader, in the main menu's right pane: the note rendered in the vault's flavour, with
  headings, lists, tasks, quotes, callouts (folded ones closed), code, tables, highlights, tags,
  embed placeholders and links (unresolved ones dimmer). `Tab` selects a link and `Enter`
  follows it like from Neovim, URLs open in the browser, and a click follows a link. It replaces
  the plain preview, and follows changes on disk without losing its place. With Neovim, `v`
  (`note.read`) switches the pane between the reader and Neovim. Its keys are under `reader.*`
  in `config.toml`, and the key names `up`, `down`, `left`, `right`, `pageup`, `pagedown`,
  `home` and `end` are new.
- `nvim.restart` restarts the embedded Neovim from the menu: it saves every note, quits, and
  starts again on the note it showed. Run it from the palette (`:`), or give it a key in
  `config.toml`. It also starts Neovim again after it was closed.
- Extensions register commands like the core's. Their commands are in the palette and the help,
  `cold-lava keys` lists them, and `[keys]` in `config.toml` rebinds them by id with the same
  rules. An extension's command id must start with its name, such as `nvim.`.

### Changed

- The embedded Neovim is now the first editor extension, on cold-lava's new extension interface
  (see `docs/extensions.md`). The pane's title names the editor (Neovim or Vim) instead of the
  program, and the help lists the editor's keys under its name.
- `Ctrl-q`, which leaves Neovim for the menu, is now the command `pane.leave`. Set its key under
  `[keys]` in `config.toml` if you use `Ctrl-q` in Neovim; it can't be `[]`. The pane's title,
  the status line and the help show the key you set.
- Without an embedded Neovim or Vim, `Enter` on a note in the main menu goes into the reader
  instead of opening `$EDITOR` full screen; `e` still does that.
- Every write to a vault now goes through one path: atomic (a hidden temporary file, then a
  rename) and refused if the file changed since it was read. Task toggles, renames, new notes,
  daily notes and whiteboards all use it.
- The main menu now re-reads only the notes that changed on disk instead of the whole vault;
  a full rescan only happens when a folder moves or a whiteboard changes.
- The graph view and the main menu share one live vault with a single file watcher, so changes
  made while the graph is open show up in the menu too.
- Every action in the main menu, the graph view and the whiteboard is a named command (e.g.
  `graph.open`, `graph.local`, `board.new-card`); each screen's help (`?`) is generated from
  them.

### Fixed

- When the embedded editor can't start, the message no longer repeats itself.
- `Ctrl-q` in the main menu no longer quits; it goes back to the editor.
- Control keys (such as `Ctrl-u`) are no longer typed as letters in text prompts; `Ctrl-u`
  clears the field.

## [0.1.0] - 2026-11-05

The first release.

### Added

- **Main menu**: your vaults (found from Obsidian's own vault list, recently opened ones, or
  added by hand), each vault's notes and whiteboards as a folder tree with backlink counts, and
  the selected note in an embedded Neovim (or Vim) that follows the selection. A rendered
  preview is shown when neither is installed.
- **Links and backlinks**: `Enter` or `gf` on a `[[link]]` in Neovim opens the note, at its
  `#heading` or `#^block`, or creates it in Obsidian's default new-note location. A backlinks
  pane lists every note that links to the open one, with the line it's on.
- **Quick switcher** (`p`) with fuzzy, accent-insensitive matching on names, aliases and paths,
  and **full-text search** (`s`) with highlighted matches; also from Neovim with
  `:ColdLavaSwitcher` and `:ColdLavaSearch`, and from scripts with `cold-lava search`.
- **Graph view**: a live force-directed graph of the vault, drawn with braille characters or, in
  Ghostty, Kitty and WezTerm, as real pixels through the Kitty graphics protocol. Local graph,
  search, tags and unresolved links as nodes, and colour groups read from Obsidian's graph
  settings or made by folder or tag.
- **Whiteboards** that read and write Obsidian Canvas (`.canvas`) files: text, note, link and
  group cards, arrows, colours, undo, and three-way merging so edits made in Obsidian at the same
  time are never overwritten.
- **Agenda, daily notes and tasks for other programs**: `cold-lava agenda --json` (Obsidian Tasks
  format, Dataview fields, global filter), `cold-lava daily` (Daily notes settings and
  templates) and `cold-lava task toggle` (done dates, recurring tasks, a guard against edits in
  between).
- Live updates when notes change on disk, accent-insensitive matching everywhere, and an example
  vault that shows every feature.

[Unreleased]: https://github.com/AlenEngineer/cold-lava/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/AlenEngineer/cold-lava/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/AlenEngineer/cold-lava/releases/tag/v0.1.0
