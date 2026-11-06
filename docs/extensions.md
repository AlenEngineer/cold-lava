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

## Core (being built now)

- **Live vault.** One `Core` owns the vault. It is the only thing watching the folder; when a
  note changes it re-reads only that note and updates the index. Nothing else scans the disk.
- **Index.** Link resolution with Obsidian's rules, backlinks with line and context, tags,
  aliases, full text. (Exists: `index.rs`, `search.rs`.)
- **Safe writes.** Every write goes through one path that writes atomically and can refuse when
  the file changed since it was read. Renames, task toggles, whiteboards and extensions all use
  it, so nothing overwrites an edit made in Obsidian at the same time. (Exists in pieces; being
  unified.)
- **Events.** `note.created`, `note.changed`, `note.removed`, `note.renamed`, emitted once per
  change, whatever caused it (cold-lava, Obsidian, Neovim, git). Anything can subscribe.

## Commands

Every action has a name, such as `graph.open`, `note.rename`, `daily.today`. Commands are what
keys, the command palette, `cold-lava run <command>` and extensions all call. Extensions register
their own. Keys are configurable in `~/.config/cold-lava/config.toml`.

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

- The host calls the extension: `initialize` (capabilities and the API version), `command/run`,
  `view/render`, `shutdown`.
- The extension calls the host: `vault/notes`, `vault/read`, `vault/write` (through the safe
  write path, with the expected previous content), `links/backlinks`, `search/text`,
  `tasks/agenda`, `ui/notify`, `ui/openNote`, `events/subscribe`.
- The host notifies subscribers: `event/noteChanged` and friends.

The method set is versioned (`apiVersion` in `initialize`), so old extensions keep working.

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
id = "word-count.show"
title = "Word count of this note"
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

## Order of work

1. Core: safe writes, live vault, events. *(in progress)*
2. Commands, command palette, configurable keys.
3. The `Extension` interface; move built-in features onto it.
4. Transports (stdio first), JSON-RPC, manifests; an example extension in Python and docs.
5. Installing extensions (`cold-lava ext install <git-url>`), permissions, versioning.

## Compatibility with Obsidian plugins

Obsidian plugins are JavaScript that draws into Electron, so running them unchanged isn't a
goal. Data compatibility is: cold-lava keeps reading and writing the same files and settings
(Tasks, Daily notes, graph colour groups, Canvas). A TypeScript extension could reuse a plugin's
logic that doesn't depend on its UI.
