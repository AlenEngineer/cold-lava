# cold-lava — notes for Claude

cold-lava is a terminal workspace for Obsidian vaults, written in Rust (ratatui). It works on a
vault **without the Obsidian app, and safely alongside it**. That is the project's core value:
everything else (graph, whiteboards, agenda) is a feature on top, and is meant to become an
extension once there is an extension API.

## Commands

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings   # CI runs the latest stable clippy, which can be stricter than yours
cargo test
cargo run --release -- example-vault        # main menu on the example vault
scripts/release-check.sh                    # is this commit a release, and is it ready?
```

All three checks must pass before pushing: CI runs them on Ubuntu and macOS, and they are
required checks on `main`.

## Architecture

**Core** (no UI; other code asks it instead of re-parsing files):

- `vault.rs`: scans a vault: notes with links (`refs` keep line and context), tags,
  frontmatter aliases, full text. `search_key()` folds text for matching (lowercase, no
  accents, no ʻokina): use it for every user-facing search.
- `index.rs`: `Index` = vault + the single link `Resolver` (Obsidian's rules: path, then name,
  shallowest wins) + backlinks. Don't write another link resolver.
- `search.rs`: fuzzy matching (quick switcher) and full-text search with match positions.
- `rename.rs` (in the `rename-core` PR until it's merged): rename/move with every link updated;
  `plan()` then `apply()`.
- `tasks.rs`, `daily.rs`: Obsidian Tasks format, Daily notes settings, moment.js date formats.

**Features / UI:**

- `home.rs`: the main menu (vaults, note tree, embedded Neovim, backlinks, switcher, search).
- `embed.rs`: terminal-in-a-pane (portable-pty + vt100). Messages from the embedded Vim arrive
  as a private OSC sequence `ESC ] 5379 ; kind ; payload BEL` (see `VIM_SCRIPT` in `home.rs`);
  the payload encodes non-ASCII as `%u<hex>;`.
- `app.rs`, `graph.rs`, `scene.rs`, `kitty.rs`, `groups.rs`: graph view (force layout; braille
  or Kitty-graphics pixels drawn under the text with `z=-1`; colour groups).
- `board.rs`: whiteboard on Obsidian Canvas (JSON Canvas) files.
- `main.rs`: CLI. `term.rs`: suspend/editor/Obsidian-URI helpers.

## Rules that keep it compatible with Obsidian

- Never overwrite edits made elsewhere: canvases three-way merge (`board.rs::merge3`), renames
  and task toggles check the file didn't change since it was read.
- Keep fields and formatting you don't understand (canvas `extra` maps; rename edits canvas files
  as text instead of re-serialising them).
- Follow the vault's own settings where Obsidian has one: `.obsidian/app.json` (new-note
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
- **Releases are automatic**: a PR that sets a new `version` in `Cargo.toml` and moves the
  Unreleased entries into `## [x.y.z] - date` makes `.github/workflows/release.yml` tag the merge
  commit, build Linux/macOS binaries and publish the release. Before 1.0, minor = may break,
  patch = fixes only.
- Commit messages: a short summary line, then what changed and why.

## Testing the UI

- `example-vault/` is the showcase vault and is used by tests (they count its notes and groups).
  Copy it to a temp folder before trying anything that writes.
- The TUI can be driven headless with tmux (`tmux new-session -d -x 160 -y 44 …`,
  `send-keys`, `capture-pane -p`). The embedded editor needs `nvim` (or `vim`) installed.
- Pixel mode only shows in terminals with the Kitty graphics protocol (Ghostty, Kitty, WezTerm),
  not in tmux; check it in the real terminal.

## Where things stand

- The extension API design is in `docs/extensions.md`. The connection to extensions must stay
  behind the `Transport`/`Connector` abstraction (stdio first; Unix sockets etc. later).
- Next on the core list: an extension API (graph, canvas, agenda and the menu should become
  extensions of the core); then performance for large vaults (Barnes–Hut layout), a config file,
  light-terminal colours.
- The project's issues are tracked on GitHub (#2 agenda and #3 daily/toggle are done).
