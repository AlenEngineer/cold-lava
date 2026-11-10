#!/usr/bin/env python3
"""Time the graph view on a large synthetic workspace, per binary: how long its layout takes to
settle after `g`, and the CPU time (user + sys) cold-lava spends until then.

    scripts/bench-graph.py [--runs N] [--notes N] [--seconds S] BINARY [BINARY ...]

The workspace is bench-items.py's (the same notes, under ~/.cache/cold-lava-bench/). Each run
starts BINARY in a pseudo-terminal of 160x44 with an empty home, waits for the menu, types `g`
and reads the program's CPU time from /proc every 0.2 s for up to S seconds. The layout takes
the same number of steps to settle in every build, one per frame, so a slower frame shows as a
later settle and more CPU. The graph has settled at the start of the first second in which the
program used less than half a CPU: a settled graph only redraws four times a second. Runs
alternate between the binaries; the table shows the median and range. Linux only (/proc).
"""

import argparse
import fcntl
import importlib.util
import os
import pty
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


def run(binary, ws, seconds):
    """(seconds to settle, CPU seconds until then), or None if it didn't settle."""
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

    def pump(wait):
        nonlocal seen
        end = time.monotonic() + wait
        while (left := end - time.monotonic()) > 0:
            r, _, _ = select.select([fd], [], [], left)
            if not r:
                continue
            try:
                data = os.read(fd, 1 << 16)
            except OSError:
                return
            if b"\x1b[6n" in data:
                os.write(fd, b"\x1b[1;1R")
            seen = (seen + data)[-4096:]

    deadline = time.monotonic() + 60
    while ws.name.encode() not in seen and time.monotonic() < deadline:
        pump(0.2)
    pump(1.0)
    start, before = time.monotonic(), cpu(pid)
    os.write(fd, b"g")
    samples = []
    settled = None
    while time.monotonic() - start < seconds:
        pump(0.2)
        samples.append((time.monotonic() - start, cpu(pid) - before))
        recent = [s for s in samples if s[0] >= samples[-1][0] - 1.0]
        if samples[-1][0] > 2 and len(recent) > 3:
            (t0, c0), (t1, c1) = recent[0], recent[-1]
            if (c1 - c0) / (t1 - t0) < 0.5:
                settled = (t0, c0)
                break
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)
    os.close(fd)
    shutil.rmtree(home, ignore_errors=True)
    return settled


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
            r = run(b, ws, a.seconds)
            results[b].append(r)
            said = "did not settle" if r is None else f"{r[0]:.1f} s, {r[1]:.1f} cpu s"
            print(f"  {b}: {said}", flush=True)
    print(f"\n{'binary':48} {'settle s':>9} {'range':>13} {'cpu s':>7} {'range':>13}")
    for b in a.binaries:
        ok = [r for r in results[b] if r]
        if not ok:
            print(f"{b:48} did not settle")
            continue
        ts, cs = [r[0] for r in ok], [r[1] for r in ok]
        print(
            f"{b:48} {statistics.median(ts):9.1f} {min(ts):6.1f}–{max(ts):<6.1f}"
            f" {statistics.median(cs):7.1f} {min(cs):6.1f}–{max(cs):<6.1f}"
        )


if __name__ == "__main__":
    main()
