#!/usr/bin/env python3
"""What does double emulation actually cost?

Muster's kill criterion (kan a_26BIX28HG): a keystroke crosses two terminal emulators and
a socket before a glyph appears, and if that is felt, the architecture is wrong. This puts
a number on it.

Two echo loops, measured identically, differing in one thing:

  plain     write a byte to a PTY -> the kernel echoes it -> read it back
  muster    write a byte to a pane's control stream -> herdr's PTY echoes it ->
            the daemon's VT consumes it -> a frame is rendered and streamed ->
            read it back off the stream

Both run `cat`, so the inner program, the line discipline and the kernel path are
identical, and what is left is the daemon plus the frame layer.

Two numbers out of the daemon side, because they answer different questions and the
difference between them turned out to be the whole story:

  first frame   the stream responded at all
  glyph         a frame arrived that actually paints the byte

Single-threaded on purpose. An earlier version read frames on a second thread and
reported three times the real latency: the measuring loop held the GIL and the reader
could not run. A blocking read on the measuring thread has no such artifact.

What is NOT in the number, and why the omissions are safe to state:

  - the app's own hops (surface -> control socket -> bridge, and bridge -> surface PTY).
    Two unix writes; the run log's mono_ns fields measure them during ./dev --contract.
  - the surface's VT parse and GPU present. Plain Ghostty pays those too, and the extra
    bytes Muster's surface must parse - a cell-addressed re-render rather than a raw
    echo - are budgeted per byte by ./dev --perf.

Needs a real herdr on PATH. Not part of the gate.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import pty
import select
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "tools/herdr-probe"))

from herdrprobe.daemon import IsolatedDaemon  # noqa: E402

ROOT = Path("/private/tmp/muster-latency")
# Distinct per sample, so a stale echo can never be mistaken for the one being timed.
ALPHABET = "abcdefghijklmnopqrstuvwxyz"
# Roughly a fast typist. Back-to-back sends would measure the daemon's throttle recovering
# from a burst rather than what a person doing one keystroke at a time sees.
TYPING_GAP = 0.15


class Failure(RuntimeError):
    pass


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * fraction))]


def summarize(name: str, samples: list[float]) -> dict:
    return {
        "name": name,
        "samples": len(samples),
        "min_ms": min(samples),
        "median_ms": statistics.median(samples),
        "p95_ms": percentile(samples, 0.95),
        "max_ms": max(samples),
    }


def render(rows: list[dict]) -> str:
    lines = [f"{'path':38}  {'min':>8}  {'median':>8}  {'p95':>8}  {'max':>8}"]
    for row in rows:
        lines.append(
            f"{row['name']:38}  {row['min_ms']:8.2f}  {row['median_ms']:8.2f}  "
            f"{row['p95_ms']:8.2f}  {row['max_ms']:8.2f}"
        )
    return "\n".join(lines)


# MARK: the floor


def measure_plain_pty(samples: int, timeout: float) -> list[float]:
    parent, child = pty.openpty()
    process = subprocess.Popen(["cat"], stdin=child, stdout=child, stderr=child, close_fds=True)
    os.close(child)
    try:
        time.sleep(0.3)
        drain_fd(parent)
        timings = []
        for index in range(samples):
            letter = ALPHABET[index % len(ALPHABET)]
            start = time.perf_counter_ns()
            os.write(parent, letter.encode())
            if not wait_for_byte(parent, letter, timeout):
                raise Failure(f"the PTY never echoed {letter!r} within {timeout}s")
            timings.append((time.perf_counter_ns() - start) / 1_000_000)
            time.sleep(TYPING_GAP)
            drain_fd(parent, 0.02)
        return timings
    finally:
        process.terminate()
        process.wait(timeout=5)
        os.close(parent)


def wait_for_byte(fd: int, letter: str, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not select.select([fd], [], [], max(0.0, deadline - time.monotonic()))[0]:
            return False
        if letter.encode() in os.read(fd, 4096):
            return True
    return False


def drain_fd(fd: int, seconds: float = 0.05) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not select.select([fd], [], [], max(0.0, deadline - time.monotonic()))[0]:
            return
        if not os.read(fd, 65536):
            return


# MARK: the daemon path


class ControlStream:
    """A `terminal session control` subprocess read on the calling thread."""

    def __init__(self, daemon: IsolatedDaemon, pane: str, cols: int = 80, rows: int = 24):
        args = daemon.herdr_argv(
            "terminal", "session", "control", pane, "--cols", str(cols), "--rows", str(rows)
        )
        self.process = subprocess.Popen(
            args, env=daemon.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, bufsize=0,
        )

    def send(self, text: str) -> None:
        self.process.stdin.write((json.dumps({"type": "terminal.input", "text": text}) + "\n").encode())
        self.process.stdin.flush()

    def read_frame(self) -> dict:
        line = self.process.stdout.readline()
        if not line:
            raise Failure("the pane's stream ended mid-measurement")
        return json.loads(line)

    def drain(self, seconds: float) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if not select.select([self.process.stdout], [], [], max(0.0, deadline - time.monotonic()))[0]:
                return
            self.process.stdout.readline()

    def close(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()


def measure_daemon_path(
    stream: ControlStream, samples: int, timeout: float
) -> tuple[list[float], list[float], int]:
    """Returns time-to-first-frame, time-to-glyph, and how often the glyph needed a second frame."""
    first_frame, to_glyph, late = [], [], 0
    for index in range(samples):
        letter = ALPHABET[index % len(ALPHABET)]
        start = time.perf_counter_ns()
        stream.send(letter)

        seen = 0
        first = None
        deadline = time.monotonic() + timeout
        while True:
            if time.monotonic() > deadline:
                raise Failure(
                    f"no frame carried {letter!r} within {timeout}s. Either the pane is not "
                    "running `cat`, or the daemon stopped rendering."
                )
            frame = stream.read_frame()
            elapsed = (time.perf_counter_ns() - start) / 1_000_000
            if frame.get("type") != "terminal.frame":
                continue
            seen += 1
            if first is None:
                first = elapsed
            if letter.encode() in base64.b64decode(frame.get("bytes", "")):
                first_frame.append(first)
                to_glyph.append(elapsed)
                if seen > 1:
                    late += 1
                break
        time.sleep(TYPING_GAP)
        stream.drain(0.02)
    return first_frame, to_glyph, late


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=60)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--json", action="store_true")
    options = parser.parse_args()

    if shutil.which("herdr") is None:
        print(
            "latency: no `herdr` on PATH, so the daemon half cannot be measured.\n"
            "Reporting only the plain-PTY floor would look like a result and would not be one.",
            file=sys.stderr,
        )
        return 2

    shutil.rmtree(ROOT, ignore_errors=True)
    ROOT.mkdir(parents=True, exist_ok=True)
    daemon = IsolatedDaemon(ROOT / "d")
    daemon.prepare()
    daemon.start()
    stream = None

    try:
        created = daemon.client().request(
            "workspace.create", {"cwd": "/tmp", "focus": True, "label": None}
        )
        pane = created["root_pane"]["pane_id"]
        stream = ControlStream(daemon, pane)
        stream.read_frame()

        # Same inner program on both sides. A shell would put readline's redraw in one
        # measurement and not the other, and readline is not what is being compared.
        stream.send("exec cat\n")
        time.sleep(1.0)
        stream.drain(0.8)

        first_frame, to_glyph, late = measure_daemon_path(
            stream, options.samples, options.timeout
        )
        plain = summarize("plain pty (the floor)", measure_plain_pty(
            options.samples, options.timeout))
    except Failure as exc:
        print(f"latency: {exc}", file=sys.stderr)
        return 1
    finally:
        if stream is not None:
            stream.close()
        daemon.stop()

    rows = [
        plain,
        summarize("daemon: stream responded", first_frame),
        summarize("daemon: glyph painted", to_glyph),
    ]
    print(render(rows))

    glyph = rows[2]
    print(
        f"\nThe stream answers every keystroke in about {rows[1]['median_ms']:.1f} ms. "
        f"In {late} of {options.samples} samples that first frame did not yet carry the "
        "byte, and the glyph waited for the next render."
    )
    print(
        f"Input-to-glyph over a plain PTY: {glyph['median_ms']:.1f} ms at the median, "
        f"{glyph['p95_ms']:.1f} ms at p95."
    )
    print(
        "None of this is Muster's code - it is herdr answering its own control stream, "
        "and every client of that stream pays it. What Muster adds on top is the frame "
        "decode and VT parse budgeted by ./dev --perf."
    )

    if options.json:
        print(json.dumps({"rows": rows, "late_frames": late}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
