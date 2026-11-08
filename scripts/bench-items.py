#!/usr/bin/env python3
"""Time the item paths on a large synthetic workspace: wall time and peak RSS per command.

    scripts/bench-items.py [--runs N] [--notes N] BINARY [BINARY ...]

Each BINARY is a cold-lava build (`target/release/cold-lava`, or a copy of another commit's).
The workspace is generated once, the same every time (fixed seed), under
~/.cache/cold-lava-bench/ (not /tmp, which is in RAM). Runs alternate between the binaries so
drift and caches favour none; the table shows the median and range of each.
"""

import argparse
import os
import random
import statistics
import subprocess
import time
from datetime import date, timedelta
from pathlib import Path

CACHE = Path.home() / ".cache" / "cold-lava-bench"
FROM, TO = "2026-01-01", "2026-12-31"

TYPES = """version = 1

[types.volcano]
on = "note"
where.tag = ["volcano"]
fields.name = { note = "name" }
fields.country = { property = "country" }
fields.elevation = { property = "elevation_m", type = "number" }
fields.tags = { inlines = "tags", type = "tag" }

[types.eruption]
fields = { vei = "number", example = "text", volcanoes = "link" }

[[types.eruption.from]]
on = "table-row"
where.header = ["VEI", "Example"]
key = ["vei"]
fields = { vei = { column = "VEI" }, example = { column = "Example" }, volcanoes = { inlines = "links" } }

[types.vei]
on = "inline"
match = '\\bVEI (?<n>\\d)\\b'
fields.vei = { capture = "n", type = "number" }
"""

WORDS = (
    "ash basalt caldera crater dome eruption fissure geyser lava magma plume pumice "
    "rift scoria shield summit tephra vent survey sample report review call write read "
    "plan fix check map measure visit"
).split()
STATUS = [" "] * 6 + ["x"] * 3 + ["/", "-"]
PRIORITY = ["", "", "", "⏫", "🔼", "🔽", "🔺", "⏬"]


def words(r, n):
    return " ".join(r.choice(WORDS) for _ in range(n))


def day(r):
    return (date(2026, 1, 1) + timedelta(days=r.randrange(365))).isoformat()


def task(r, notes):
    status = r.choice(STATUS)
    parts = [f"- [{status}] {words(r, r.randint(2, 7)).capitalize()}"]
    if r.random() < 0.3:
        parts.append(f"[[Note {r.randrange(notes)}]]")
    if r.random() < 0.3:
        parts.append(f"#{r.choice(WORDS)}")
    if r.random() < 0.25:
        parts.append(f"[due:: {day(r)}]")
    elif r.random() < 0.7:
        parts.append(f"📅 {day(r)}")
    if r.random() < 0.2:
        parts.append(f"⏳ {day(r)}")
    if r.random() < 0.1:
        parts.append(f"🛫 {day(r)}")
    if r.random() < 0.1:
        parts.append("🔁 every week")
    if p := r.choice(PRIORITY):
        parts.append(p)
    if r.random() < 0.05:
        parts.append(f"`📅 {day(r)}`")
    if r.random() < 0.05:
        parts.append(f"[see](Notes/Note%20{r.randrange(notes)}.md)")
    if r.random() < 0.05:
        parts.append(f"^t{r.randrange(10**6)}")
    if status == "x":
        parts.append(f"✅ {day(r)}")
    return " ".join(parts)


def note(r, i, notes):
    lines = []
    volcano = r.random() < 0.1
    lines += ["---", f"tags: [{'volcano' if volcano else r.choice(WORDS)}]"]
    if volcano:
        lines += ["country: Iceland", f"elevation_m: {r.randint(100, 6000)}"]
    lines += ["---", f"# Note {i}", ""]
    for s in range(r.randint(2, 4)):
        lines += [f"## {words(r, 3).capitalize()} {s}", ""]
        lines.append(
            f"{words(r, 12).capitalize()} [[Note {r.randrange(notes)}]] #{r.choice(WORDS)} "
            f"its eruption (VEI {r.randint(0, 7)}) {words(r, 8)}. owner:: {r.choice(WORDS)}"
        )
        lines.append("")
        for _ in range(r.randint(1, 3)):
            lines.append(f"- {words(r, 6)} [[Note {r.randrange(notes)}]]")
            if r.random() < 0.3:
                lines.append(f"    - {words(r, 4)} VEI {r.randint(0, 7)}")
        lines.append("")
        for _ in range(r.randint(2, 4)):
            lines.append(task(r, notes))
            if r.random() < 0.15:
                lines.append("    " + task(r, notes))
        lines.append("")
        if r.random() < 0.1:
            lines += [f"> {task(r, notes)}", ""]
        if r.random() < 0.2:
            lines += ["| VEI | Example |", "| --- | --- |"]
            lines += [f"| {v} | [[Note {r.randrange(notes)}]] |" for v in range(3)]
            lines.append("")
    newline = "\r\n" if r.random() < 0.05 else "\n"
    return newline.join(lines) + newline


def generate(notes):
    ws = CACHE / f"ws-{notes}"
    stamp = ws / ".generated"
    if stamp.exists():
        return ws
    r = random.Random(7)
    (ws / ".obsidian").mkdir(parents=True, exist_ok=True)
    (ws / ".cold-lava").mkdir(exist_ok=True)
    (ws / ".obsidian/daily-notes.json").write_text('{"folder": "Journal", "format": "YYYY-MM-DD"}')
    (ws / ".cold-lava/types.toml").write_text(TYPES)
    for d in ("Notes", "Journal"):
        (ws / d).mkdir(exist_ok=True)
    for i in range(notes):
        (ws / "Notes" / f"Note {i}.md").write_bytes(note(r, i, notes).encode())
    for k in range(notes // 10):
        d = (date(2026, 1, 1) + timedelta(days=k)).isoformat()
        tasks = [task(r, notes) for _ in range(r.randint(3, 8))]
        (ws / "Journal" / f"{d}.md").write_text(f"# {d}\n\n" + "\n".join(tasks) + "\n")
    stamp.write_text("")
    return ws


def commands(ws):
    w = str(ws)
    return {
        "items task": ["items", w, "task"],
        "items task --json": ["items", w, "task", "--json"],
        "items vei": ["items", w, "vei"],
        "items heading": ["items", w, "heading"],
        "agenda --json": ["agenda", w, "--from", FROM, "--to", TO, "--json"],
        "table task --json": ["table", w, "task", "--json"],
    }


def run(binary, args):
    start = time.perf_counter()
    p = subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    out = p.stdout.read()
    _, status, usage = os.wait4(p.pid, 0)
    wall = time.perf_counter() - start
    if status != 0:
        return None
    return wall, usage.ru_maxrss / 1024, len(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--notes", type=int, default=3000)
    ap.add_argument("--only", help="run only the commands whose name contains this")
    ap.add_argument("binaries", nargs="+")
    a = ap.parse_args()
    ws = generate(a.notes)
    print(f"workspace {ws} ({a.notes} notes), {a.runs} runs per binary")
    for name, args in commands(ws).items():
        if a.only and a.only not in name:
            continue
        results = {b: [] for b in a.binaries}
        sizes = {}
        for b in a.binaries:
            run(b, args)
        for _ in range(a.runs):
            for b in a.binaries:
                if r := run(b, args):
                    results[b].append(r[:2])
                    sizes.setdefault(b, set()).add(r[2])
        print(f"\n{name}")
        for b in a.binaries:
            if not results[b]:
                print(f"  {b:<50} failed")
                continue
            walls = [w for w, _ in results[b]]
            rsses = [m for _, m in results[b]]
            print(
                f"  {b:<50} {statistics.median(walls) * 1000:7.0f} ms "
                f"({min(walls) * 1000:.0f}-{max(walls) * 1000:.0f})  "
                f"{statistics.median(rsses):6.1f} MB peak  {sorted(sizes[b])[0]} bytes out"
            )


if __name__ == "__main__":
    main()
