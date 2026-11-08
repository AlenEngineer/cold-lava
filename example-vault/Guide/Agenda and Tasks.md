---
tags: [guide]
---
# Agenda and tasks

cold-lava understands tasks written for the Obsidian **Tasks** plugin, so other programs (a
status bar, a calendar widget, a script) can ask it what's due without parsing the vault.

```bash
cold-lava agenda example-vault --from 2026-11-01 --to 2026-11-30 --json
cold-lava daily example-vault today
cold-lava task toggle "example-vault/Journal/2026-11-05.md:10" --id <id from the agenda>
```

- Tasks with a due (📅), scheduled (⏳) or start (🛫) date show up on that day. Tasks in a daily
  note without a date belong to that note's day.
- Toggling adds the done date (✅) and, for recurring tasks (🔁), adds the next occurrence, like
  Obsidian Tasks does.
- Daily notes follow this vault's Daily notes settings: the `Journal` folder and the template in
  `Templates/Daily`. In the main menu, press `t` for today's note.

In the main menu, `A` opens the **agenda**: every task by day (overdue, today, the next seven
days, later, undated), to tick with `x` or open with `Enter`.

In the reader, `x` ticks a task the same way: the one the selected link is in, else the
highlighted line's (`j` / `k` move it on the last page), else the first one on screen.

Try ticking a task in the [[2026-11-05]] journal note.

Back to [[Start Here]].
