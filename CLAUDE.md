# cold-lava — notes for Claude

cold-lava is a terminal app for Obsidian vaults, written in Rust (ratatui). It works on a
workspace **without the Obsidian app, and safely alongside it**. That is the project's core value:
everything else (graph, whiteboards, agenda) is a feature on top, and is meant to become an
extension once there is an extension API.

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
```

All three checks must pass before pushing: CI runs them on Ubuntu and macOS, and they are
required checks on `main`.

## Architecture

A Cargo workspace of four crates, plus the GUI beside it. The version lives in the root `Cargo.toml` under
`[workspace.package]`; `example-workspace/`, `docs/` and `scripts/` stay at the root.

- `crates/engine`: the `cold-lava-engine` library (the core below). It has no UI and must not
  depend on ratatui, crossterm, portable-pty or vt100 (`cargo tree -p cold-lava-engine`).
- `crates/cold-lava`: the `cold-lava` binary (extensions and UI below, `tests/` with the drive
  scripts and goldens). `main.rs` imports the engine's modules by name, so `crate::workspace` etc.
  work in the binary.
- `crates/views`: `cold-lava-views`, the frontend framework, empty on purpose: pieces move into it
  from `crates/cold-lava` once a second frontend needs them (`docs/engine.md`, "The frontend").
- `crates/editor`: `cold-lava-editor`, the small text editor, headless (text, cursors, edits,
  undo; no drawing) so the TUI and the GUI render it alike. Empty until the document view edits
  in place. Saves go through the engine's safe writes.
- `crates/gui`: `cold-lava-gui`, a Tauri window on the same engine. It is excluded from the Cargo
  workspace (its own `[workspace]` and `Cargo.lock`) so `cargo build`/`test` and CI never compile
  a webview; build it with `cargo build --manifest-path crates/gui/Cargo.toml` (Linux needs
  webkit2gtk-4.1). The terminal UI stays the main frontend.

**Core** (`crates/engine/src`; no UI; other code asks it instead of re-parsing files):

- `core.rs`: `Core` = one live workspace. Owns the `Index`, is the only file watcher, re-reads only
  changed notes and emits `Event`s (`poll()`, `subscribe()`). Screens share it: the menu lends
  its core to the graph view and gets it back. Don't add another watcher or full rescan loop.
- `write.rs`: the only way to write to a workspace: `write_atomic`, `replace` (refused if the file
  changed since it was read), `create`, `move_file`. Never call `std::fs::write` on workspace files.
- `trash.rs`: the only way to delete: `delete` moves a file to the trash the workspace's
  `trashOption` names (system trash, or `.trash`; never a permanent delete), refused if it
  changed since it was read; `restore` puts it back.
- `workspace.rs`: scans a workspace: notes with links (`refs` keep line and context), tags,
  frontmatter aliases, full text. `search_key()` folds text for matching (lowercase, no
  accents, no ʻokina): use it for every user-facing search.
- `index.rs`: `Index` = workspace + the single link `Resolver` (Obsidian's rules: path, then name,
  shallowest wins) + backlinks. Don't write another link resolver.
- `search.rs`: fuzzy matching (quick switcher) and full-text search with match positions.
- `rename.rs` (in the `rename-core` PR until it's merged): rename/move with every link updated;
  `plan()` then `apply()`.
- `markdown.rs`: Markdown flavours (named sets of `Syntax`, built-in or from `[markdown]` in
  the config) and `parse()` into a `Doc` the reader draws. Obsidian extras pulldown-cmark lacks
  are rules in one section, each gated by its `Syntax`; the tag pattern is shared with `workspace.rs`.
- `items.rs`: notes as data. `extract()` reads a note into `Item`s (task, list, heading, block)
  in one pulldown-cmark pass in the workspace's flavour; `Workspace::items()` caches them per note on
  first use. Ids are stable (block id, heading path, or text hash under the parent), `rev`
  hashes the item's lines. Read tasks through items, not by scanning lines.
- `tasks.rs`, `daily.rs`: Obsidian Tasks settings (statuses, global filter); `agenda` and
  `toggle` on items of the built-in `task` type (`types/task.toml`, read by `Task::of`); Daily
  notes settings, moment.js date formats.
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
    into panes.
  - Side effects are queued: `Host` collects `Effect`s that `Home` applies after the call
    returns, so extensions never hold `&mut Home`.
- `ext/nvim.rs`: Neovim as the editor extension: `VIM_SCRIPT`, what each OSC message means, the
  commands it types into its pane.

**Features / UI** (`crates/cold-lava/src`):

- `home.rs`: the main menu (workspaces, note tree, the right pane with the reader or an extension's
  program, backlinks, switcher, search). It applies the extensions' `Effect`s.
- `embed.rs`: terminal-in-a-pane (portable-pty + vt100). Messages from the program arrive as a
  private OSC sequence `ESC ] 5379 ; kind ; payload BEL` (Neovim's come from `VIM_SCRIPT` in
  `ext/nvim.rs`); the payload encodes non-ASCII as `%u<hex>;`. The core passes them to the
  extension that owns the pane.
- `app.rs`, `graph.rs`, `scene.rs`, `kitty.rs`, `groups.rs`: graph view (force layout; braille
  or Kitty-graphics pixels drawn under the text with `z=-1`; colour groups).
- `board.rs`: whiteboard on Obsidian Canvas (JSON Canvas) files.
- `table.rs`: the declarative table view (columns, rows by id, events back; serde, so it can
  cross the view gateway). `type_table.rs` builds a type's table from the core's items; edits
  go through `edit::set_field`.
- `list.rs`: the declarative list / tree view (groups of rows by id with a checkbox, pieces of
  text with looks, a right side; selection, folding, filtering; events back; serde like the
  table). `agenda.rs` builds the agenda from typed tasks (`A`, `agenda.open`); ticks go through
  `tasks::toggle` with the row's `rev`, as `x` in the reader does.
- `main.rs`: CLI. `term.rs`: suspend/editor/Obsidian-URI helpers. `config.rs`: `config.toml`.
- `drive.rs`: `cold-lava drive`, scripted UI runs: the real binary in a pty, screen read back
  with vt100, cursor-position queries answered.

## Rules that keep it compatible with Obsidian

- Never overwrite edits made elsewhere: canvases three-way merge (`board.rs::merge3`), renames
  and task toggles check the file didn't change since it was read.
- Keep fields and formatting you don't understand (canvas `extra` maps; rename edits canvas files
  as text instead of re-serialising them).
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
  steps; `crates/cold-lava/tests/drive/*.drive` are examples). Wait with `wait-for TEXT`, not `wait`. Runs get an
  empty home (so they don't read your config or add workspaces to your list; `--real-home` to
  use yours), and `env COLD_LAVA_EDITOR=none` for the reader instead of Neovim. The embedded editor needs `nvim`
  (or `vim`) installed.
- Pixel mode only shows in terminals with the Kitty graphics protocol (Ghostty, Kitty, WezTerm),
  not in a pseudo-terminal or tmux; check it in the real terminal.

## Where things stand

- The whole picture (engine layers, engine vs. frontends) is `docs/engine.md`; the crate split
  keeps the engine free of ratatui/crossterm. The extension API design is in `docs/extensions.md`. The connection to extensions must stay
  behind the `Transport`/`Connector` abstraction (stdio first; Unix sockets etc. later).
- In progress: the extension API (step 3). The `Extension` interface exists and Neovim runs on
  it; the graph, canvas, agenda and the menu should become extensions of the core too. Then
  performance for large workspaces (Barnes–Hut layout), a config file, light-terminal colours.
- The project's issues are tracked on GitHub (#2 agenda and #3 daily/toggle are done).
