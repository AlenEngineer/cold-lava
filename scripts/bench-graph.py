#!/usr/bin/env python3
"""Time the graph view on a large synthetic workspace, per binary: how long its layout takes to
settle after `g`, the CPU time (user + sys) cold-lava spends until then, and that CPU per frame.

    scripts/bench-graph.py [--runs N] [--notes N] [--seconds S] BINARY [BINARY ...]

The workspace is bench-items.py's (the same notes, under ~/.cache/cold-lava-bench/). Each run
starts BINARY in a pseudo-terminal of 160x44 with an empty home, waits for the menu, types `g`
and reads the program's CPU time from /proc every 0.1 s for up to S seconds. Every frame hides
the cursor (ESC [?25l), so the frames are counted; a frame that moves the cursor (ESC [r;cH)
changed the screen. The graph has settled at the last frame that changed the screen before two quiet
seconds: then the layout has stopped and the camera has finished easing. The layout takes the
same number of steps in every build, so a slower frame shows as a later settle and more CPU.
A run that never goes quiet shows its CPU per frame alone. Runs alternate between the
binaries; the table shows the median and range. Linux only (/proc).
"""

import argparse
import fcntl
import importlib.util
import os
import pty
import re
import select
import shutil
import statistics
import struct
import tempfile
import termios
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("bench_items", HERE / "bench-items.py")
bench_items = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench_items)
TICK = os.sysconf("SC_CLK_TCK")


def cpu(pid):
    """CPU seconds the process has used so far."""
    with open(f"/proc/{pid}/stat") as f:
        fields = f.read().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / TICK


HIDE = b"\x1b[?25l"
QUERY = b"\x1b[6n"
# A frame that changed the screen moves the cursor to what it rewrites.
MOVE = re.compile(rb"\x1b\[\d+;\d+H")
QUIET = 2.0


def run(binary, ws, seconds):
    """(seconds to settle or None, CPU seconds, frames) until it settled or S ran out."""
    home = tempfile.mkdtemp(prefix="cold-lava-bench-home-", dir=bench_items.CACHE)
    pid, fd = pty.fork()
    if pid == 0:
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": home,
            "XDG_CONFIG_HOME": f"{home}/.config",
            "XDG_CACHE_HOME": f"{home}/.cache",
            "XDG_DATA_HOME": f"{home}/.local/share",
            "TERM": "xterm-256color",
            "COLORTERM": "truecolor",
            "COLD_LAVA_EDITOR": "none",
        }
        os.execve(binary, [binary, "--braille", str(ws)], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 160, 0, 0))
    seen = b""
    frames = 0
    changed = None

    def pump(wait):
        nonlocal seen, frames, changed
        end = time.monotonic() + wait
        while (left := end - time.monotonic()) > 0:
            r, _, _ = select.select([fd], [], [], left)
            if not r:
                continue
            try:
                data = os.read(fd, 1 << 16)
            except OSError:
                return
            if QUERY in data:
                os.write(fd, b"\x1b[1;1R")
            frames += data.count(HIDE)
            if MOVE.search(data):
                changed = time.monotonic()
            seen = (seen + data)[-4096:]

    deadline = time.monotonic() + 60
    while ws.name.encode() not in seen and time.monotonic() < deadline:
        pump(0.2)
    pump(1.0)
    start, before, frames = time.monotonic(), cpu(pid), 0
    os.write(fd, b"g")
    samples = []
    result = None
    while time.monotonic() - start < seconds:
        pump(0.1)
        now = time.monotonic()
        samples.append((now, cpu(pid) - before, frames))
        if changed and changed > start + 0.5 and now - changed >= QUIET:
            _, c, n = next(s for s in samples if s[0] >= changed)
            result = (changed - start, c, n)
            break
    if result is None:
        result = (None, samples[-1][1], samples[-1][2])
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    os.close(fd)
    shutil.rmtree(home, ignore_errors=True)
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--notes", type=int, default=3000)
    ap.add_argument("--seconds", type=float, default=90)
    ap.add_argument("binaries", nargs="+")
    a = ap.parse_args()
    ws = bench_items.generate(a.notes)
    print(f"workspace {ws} ({a.notes} notes), {a.runs} runs, at most {a.seconds:g} s each")
    results = {b: [] for b in a.binaries}
    for _ in range(a.runs):
        for b in a.binaries:
            t, c, n = run(b, ws, a.seconds)
            results[b].append((t, c, n))
            settle = "did not settle" if t is None else f"settled in {t:.1f} s"
            print(f"  {b}: {settle}, {c:.1f} cpu s, {n} frames, {per_frame(c, n):.1f} ms/frame", flush=True)
    print(
        f"\n{'binary':40} {'settle s':>8} {'range':>13} {'cpu s':>7} {'range':>13}"
        f" {'cpu ms/frame':>12} {'range':>13}"
    )
    for b in a.binaries:
        rs = results[b]
        ok = [r for r in rs if r[0] is not None]
        fs = [per_frame(r[1], r[2]) for r in rs]
        if ok:
            ts, cs = [r[0] for r in ok], [r[1] for r in ok]
            head = f"{span(ts)} {span(cs, 7)}"
        else:
            head = f"{'did not settle':>22} {'':>21}"
        print(f"{b:40} {head} {span(fs, 12)}")


def per_frame(c, n):
    return 1000 * c / max(n, 1)


def span(xs, width=8):
    return f"{statistics.median(xs):{width}.1f} {min(xs):6.1f}–{max(xs):<6.1f}"


if __name__ == "__main__":
    main()
