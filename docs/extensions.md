# Extensions: design

Status: **draft**. This is the plan the code is being built towards; sections are marked with
what exists already.

cold-lava aims to be as extensible as Obsidian, and open-source. The product at the centre is
the notes app itself: open a vault, find a note, read and edit it, move between notes through
links. Everything else (the graph, whiteboards, the agenda, even the main menu) should be an
extension of that core.

## Layers

```
 extensions        graph · whiteboard · agenda · daily · menu · third-party…
                   ─────────────────────────────────────────────────────────
 extension host    commands · views · event subscriptions · permissions
                   ─────────────────────────────────────────────────────────
 protocol          JSON-RPC 2.0 messages (requests, responses, notifications)
                   ─────────────────────────────────────────────────────────
 transport         stdio · Unix socket · TCP · in-process … (pluggable)
                   ─────────────────────────────────────────────────────────
 core              live vault · index (links, backlinks, tags, aliases) ·
                   search · safe writes · events
```

Each layer only talks to the one below it, so any one of them can be replaced on its own.

## Core

- **Live vault.** One `Core` owns the vault. It is the only thing watching the folder; when a
  note changes it re-reads only that note and updates the index. Nothing else scans the disk.
- **Index.** Link resolution with Obsidian's rules, backlinks with line and context, tags,
  aliases, full text. (Exists: `index.rs`, `search.rs`.)
- **Safe writes.** Every write goes through one path that writes atomically and can refuse when
  the file changed since it was read. Renames, task toggles, whiteboards and extensions all use
  it, so nothing overwrites an edit made in Obsidian at the same time. (Exists: `write.rs`.)
- **Events.** `note.created`, `note.changed`, `note.removed`, `note.renamed`, emitted once per
  change, whatever caused it (cold-lava, Obsidian, Neovim, git). Anything can subscribe.

## Reading is core, editing is an extension

Reading a vault and moving through it by links is the product, so it works with nothing else
installed. Editing is left to the editor each person already uses: a text editor good enough to
live in is a project of its own, and it would be worse than the one they have.

- **The reader (core).** A Markdown view of the note: headings, lists, tasks and code blocks
  styled, links highlighted and followed from the keyboard, backlinks beside it. It replaces the
  plain-text preview the menu shows when no Vim is installed.
- **The editor (an extension slot).** An editor extension provides `note.edit`. There are three
  levels, and an editor can stop at any of them:

  | Level | What you get | Provided by |
  | --- | --- | --- |
  | Full screen | The editor takes over the terminal until it quits | Built in, for any `$EDITOR` |
  | In a pane | The editor runs beside the tree, as Neovim does today | Any TUI editor, through the "program in a pane" view |
  | Bridged | Follow links and open backlinks from inside the editor | One extension per editor |

- **Neovim is the first editor extension.** It keeps the Vim script and the meaning of the
  private OSC messages (`ESC ] 5379 ; kind ; payload BEL`) that make link following work inside
  it. The core keeps only the pane (`embed.rs`), which third-party views need as well. Without
  the Neovim-only checks, Helix, Kakoune or micro embed in the pane too, without the bridge.
  (Exists: `src/ext/nvim.rs`.)
- **Saves need no cooperation.** The core's file watcher sees every save, so an editor only has
  to write the file. Telling the core about a save is a hint that makes it show up sooner.

### Markdown flavours

Markdown is not one language. GitHub, Obsidian and plain CommonMark each read the same file a
little differently, so the reader reads a note in a **flavour**: a named set of syntax features
(`tables`, `task-lists`, `wikilinks`, `callouts`, `highlights`, `tags` and so on). Syntax a
flavour doesn't have stays plain text. Under `github`, `==x==` shows its equals signs.

- **Built in.** `commonmark` (only frontmatter is hidden), `github` (tables, task lists,
  strikethrough, footnotes, `> [!NOTE]` alerts) and `obsidian` (GitHub's, plus wikilinks,
  embeds, callouts, highlights, `%%comments%%`, tags, block ids and math). A vault with an
  `.obsidian` folder reads as `obsidian`, any other folder as `github`.
- **Your own.** `config.toml` defines more, as a built-in flavour with features added or
  removed, and picks the one to use.
- **Later, extensions.** A flavour decides what is parsed. What it leaves unrendered, such as a
  fenced code block in `mermaid` or a callout type of a plugin's own, an extension can draw
  through `view/render`.

## Commands

Every action has a name, such as `graph.open`, `note.rename`, `daily.today`. Commands are what
keys, the command palette, `cold-lava run <command>` and extensions all call. Keys are
configurable in `~/.config/cold-lava/config.toml`.

Extensions register their own commands in their manifest (`commands`). The menu has one command
table at runtime: the core's commands, then each extension's. Keys, the palette, the help,
`cold-lava keys` and the `[keys]` section of `config.toml` all read that table, so an extension's
command is configured, listed and reported exactly like a core one. *(Exists:
`commands::menu`, which `Registry` builds when it loads extensions.)*

- **Ids are namespaced.** An extension's command id is `<extension name>.<something>`, such as
  `nvim.restart`. The core leaves out a command whose id is outside that namespace or is already
  a command, and reports why in the status line and in `cold-lava keys`.
- **Default keys are a suggestion.** A default key that another command already has, or that
  doesn't parse, is dropped and reported. The command stays, without that key.
- **Bound in the menu only.** While a pane has focus every key goes to its program, except
  `pane.leave`. Extensions' commands run from the menu and the palette.
- **Running one.** A key or the palette sends the extension `command/run` with the command's
  `id` (and `null` args). An extension's own `command/run` goes through the same table, so it can
  run core commands and other extensions' commands as a key would.

Editing works the same way. `note.follow-link` is a core command: the editor runs it
(`command/run`) when you press Enter on a link. `note.edit` is the command the active editor
extension will fill in.

## Transport: an abstraction from the start

Extensions run as separate processes. How bytes travel between cold-lava and an extension is
hidden behind one interface, so new kinds of connection can be added without touching the
protocol, the host or any extension:

```rust
/// A bidirectional, message-framed connection to one extension.
pub trait Transport: Send {
    /// Send one complete message (a serialized JSON-RPC object).
    fn send(&mut self, message: &[u8]) -> io::Result<()>;
    /// The next complete message, or None when the other side closed the connection.
    fn recv(&mut self) -> io::Result<Option<Vec<u8>>>;
    fn close(&mut self) -> io::Result<()>;
}

/// Knows how to open a transport to an extension described by its manifest.
pub trait Connector {
    /// The `transport` value in a manifest this connector handles, e.g. "stdio".
    fn kind(&self) -> &'static str;
    fn connect(&self, manifest: &Manifest) -> io::Result<Box<dyn Transport>>;
}
```

- **Framing belongs to the transport.** Over stdio and sockets, messages are framed like LSP
  (`Content-Length: N\r\n\r\n` then N bytes). A WebSocket transport would use its own frames. The
  protocol layer only ever sees whole messages.
- **First implementation: stdio.** cold-lava starts the extension's command and talks over its
  stdin/stdout; stderr goes to a log.
- **Later, without other changes:** Unix sockets (connect to a long-running extension or
  daemon), TCP, named pipes on Windows, or an in-process transport for extensions built into
  cold-lava, which lets built-in and external extensions use exactly the same code path.
- The manifest picks the transport; connectors are registered by kind, so adding one is a new
  `Connector` and nothing else.

## Protocol

JSON-RPC 2.0. Both sides can send requests and notifications.

- The host calls the extension: `initialize` (capabilities, the API version and the
  `commands` it registers), `command/run` (`id`, `args`: one of its commands),
  `view/render`, `shutdown`.
  - The editor gets `editor/open` (a note, and a `jump` to a line, heading or block),
    `editor/save` and `editor/renamed` (`from`, `to`).
  - An extension hears from a pane it opened through `pane/message` (`kind` and `payload`, what
    its program wrote as `ESC ] 5379 ; kind ; payload BEL`) and `pane/exited` (with an `error`
    when the program never started).
- The extension calls the host: `vault/notes`, `vault/read`, `vault/write` (through the safe
  write path, with the expected previous content), `links/backlinks`, `search/text`,
  `tasks/agenda`, `ui/notify`, `ui/openNote`, `events/subscribe`, and:
  - `command/run`: run any command in the menu's table by `id` with `args`, as a key would: a
    core command or another extension's. Core commands ignore `args`, except
    `note.follow-link` (`from`, `link`), which only extensions can run, and `search.open`
    (`query`, optional). An unknown id is reported in the status line.
  - `editor/opened`: the editor shows this note now. It is a report, not a request, so the
    host never answers it with `editor/open`, which would echo.
  - `vault/saved`: a note was just written, so the host reads it now instead of when the file
    watcher notices.
  - `pane/open` (a `pane` id the extension picks, and the program with its arguments),
    `pane/input` (text to type into the pane) and `pane/close`.
- The host notifies subscribers: `event/noteChanged` and friends.

The method set is versioned (`apiVersion` in `initialize`), so old extensions keep working.
Paths are absolute and inside the vault. A message's tag is its method name, for example
`{"method": "pane/input", "params": {"pane": 1, "text": ":w\r"}}`.

Built-in extensions already use, in-process (`src/ext/mod.rs`): `initialize` (with
`commands`), `command/run`, `editor/open`, `editor/save`, `editor/renamed`, `pane/message` and
`pane/exited` from the host, and `ui/notify`, `command/run`, `editor/opened`, `vault/saved`,
`pane/open` and `pane/input` from the extension.

- **Saves are not acknowledged yet.** `editor/save` gets no answer, so before it reads the files
  (to rename a note, to show the reader, to quit) the core waits 300 ms. A later version answers
  `editor/save` once the files are written.
- **`editor/renamed` is temporary.** Until extensions can subscribe to events, a rename tells the
  editor directly. Then the editor follows `note.renamed` like any other subscriber.

### Panes and keys

The core owns every pane. It starts the program an extension names in `pane/open`, draws it,
resizes it and sends it the keys you type. The extension only types text of its own
(`pane/input`) and hears what the program sends back. Extensions never see keystrokes.

While a pane has focus, the core keeps one key: `pane.leave` (`Ctrl-q`) takes you back to the
menu. It can be moved in `config.toml` but not unbound, so every pane has a way out. The core
shows one pane at a time and answers a second `pane/open` with `pane/exited`.

## Manifest

```toml
# .cold-lava/extensions/word-count/extension.toml (per vault)
# or ~/.config/cold-lava/extensions/word-count/extension.toml (for every vault)
name = "word-count"
version = "0.1.0"
api = "1"
transport = "stdio"
command = ["python3", "main.py"]
permissions = ["read"]          # "read", "write", "network"…

[[commands]]
id = "word-count.show"           # must start with the extension's name
title = "Word count of this note"
keys = ["W"]                     # default keys; config.toml can change them
```

## Views

Extensions can't draw into cold-lava's terminal directly. A view is one of:

- **Declarative:** a list, a tree or Markdown, described as JSON and drawn by cold-lava (fits most
  panels: outlines, task lists, search results).
- **A terminal program in a pane:** the extension names a command and cold-lava runs it in a pane,
  the same way the main menu embeds Neovim. Full freedom, any TUI library.

## Built-in features become extensions

The graph, whiteboards, agenda, daily notes and rename move onto the same `Extension` interface
(in-process, through the in-process transport). If our own features fit the interface, it is
good enough for everyone else.

A built-in extension implements the `Extension` trait (`src/ext/mod.rs`) instead of reading
messages from a transport. The trait is the protocol: each of its methods is one host→extension
method, and the `Host` it is given has one method per extension→host method. Every type that
crosses derives serde, and tests pin its JSON, so an extension can move to its own process
without changing shape. What an extension asks for is queued and applied after its call
returns, so it never holds the menu's state.

## Order of work

1. Core: safe writes, live vault, events. *(done)*
2. Commands, command palette, configurable keys. The Markdown reader can land alongside, since
   it's core.
3. The `Extension` interface; move built-in features onto it, starting with the Neovim editor.
   *(in progress: the interface, extension commands and the Neovim editor exist)*
4. Transports (stdio first), JSON-RPC, manifests; an example extension in Python and docs.
5. Installing extensions (`cold-lava ext install <git-url>`), permissions, versioning.

## Compatibility with Obsidian plugins

Obsidian plugins are JavaScript that draws into Electron, so running them unchanged isn't a
goal. Data compatibility is: cold-lava keeps reading and writing the same files and settings
(Tasks, Daily notes, graph colour groups, Canvas). A TypeScript extension could reuse a plugin's
logic that doesn't depend on its UI.
