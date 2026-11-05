---
tags: [guide]
---
# Graph view

Every note is a dot and every link is a line. Notes with more links are bigger.

## Moving around
- `↑↓←→` or `hjkl` jump to the nearest note in that direction; `Tab` cycles through a note's links.
- Scroll to zoom at the cursor, drag to pan, and drag a dot to pull it around.
- `/` searches: try `/kil` then `Enter` to jump to [[Kīlauea]].

## Things this vault shows off
- **Local graph:** select [[Mount St. Helens]] and press `f`. Use `]` to see two steps out.
- **Tags as nodes:** press `t`. Volcano notes use nested tags like `#volcano/shield`.
- **Unresolved links:** press `u` to see notes that are linked but don't exist yet, like
  [[Lahar]] and [[Unzen]]. Select one and press `Enter` to create it, just like in Obsidian.
- **Orphans:** the note *Loose thought* in the Inbox has no links at all, so it floats on its own.
  Press `O` to hide orphans.
- **Opening notes:** `Enter` opens the selected note in `$EDITOR`; `o` opens it in Obsidian.
  Edits show up in the graph as soon as you save.

Links inside code blocks are ignored, like Obsidian does:

```
[[This is not a link]]
```

Back to [[Start Here]].
