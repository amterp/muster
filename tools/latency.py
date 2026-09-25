#!/usr/bin/env python3
"""Where the time goes between a key and a glyph, stage by stage.

Muster's kill criterion (kan a_26BIX28HG): a keystroke crosses two terminal emulators and
a socket before a glyph appears, and if that is felt, the architecture is wrong. This puts
a number on each stage, so the cost of each can be told apart:

  plain     write a byte to a PTY -> the kernel echoes it -> read it back. The floor: what
            plain Ghostty pays before its own parse and draw.
  herdr     write a byte to a pane's control stream -> herdr's PTY echoes it -> the daemon's
            VT consumes it -> a frame is rendered, encoded and streamed -> read it back off
            the stream. Everything here is herdr's: its client process, its virtual render,
            its 16 ms render throttle, and the frame encoding.
  bridge    the same, but through the real `muster-bridge`: the byte goes in over the app's
            control socket as the core sends it, and the glyph is read off a PTY standing in
            for the surface's, after the bridge has unwrapped the frame. What this adds to
            the herdr row is Muster's.
  arrow     `pane.send_input` with a named key - how Muster sends an unmodified arrow, since
            only the daemon knows the pane's cursor mode. Timed to the daemon's answer.

Every sample runs every path once, in rotation, so load on the machine lands on all of
them alike rather than on whichever ran last. All panes run `cat`, so the inner program,
the line discipline and the kernel path are identical across rows.

Then a crowded window: fifteen panes, thirteen printing, measured with every pane attached
(what Muster does today) and with only the two measured panes attached (what a window that
detached its hidden panes would do), alternating. And the work behind each number: CPU time
per keystroke and per second for herdr, its client, and the bridge, and the bytes a surface
has to parse per echoed byte.

Wall time on a loaded machine is inflated, so every table prints the load average beside
it, and the CPU and byte counts are the numbers to trust when the two disagree.

Single-threaded on purpose. An earlier version read frames on a second thread and reported
three times the real latency: the measuring loop held the GIL and the reader could not run.

What is NOT in any number: the surface's own VT parse and GPU present. Plain Ghostty pays
those too - per byte, and Muster's surface parses a re-render rather than the program's own
bytes, which is why the bytes-per-echo row is here: multiply it by ./dev --perf's
`frame.vt_parse` to price it.
"""

from __future__ import annotations

import argparse
import base64
import ctypes
import fcntl
import json
import os
import pty
import re
import select
import socket
import statistics
import struct
import subprocess
import sys
import termios
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
# A frame is a re-render, so its escape sequences are full of letters (`ESC [ ? 25 h`, `m`,
# `H`). A glyph is a letter outside them.
ESCAPES = re.compile(
    rb"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[P^_][^\x1b]*\x1b\\|\x1b[ -~]"
)


# Stands in for `herdr terminal session control`: every input line comes straight back as a
# frame carrying the same bytes. What it costs is measured on its own and subtracted, so the
# bridge's own hop can be priced without herdr's render cadence in the number.
ECHO_DAEMON = """\
import base64, json, sys
out = sys.stdout.buffer
for seq, line in enumerate(sys.stdin.buffer):
    message = json.loads(line)
    if message.get("type") != "terminal.input":
        continue
    data = base64.b64decode(message["bytes"]) if "bytes" in message else message["text"].encode()
    frame = {"type": "terminal.frame", "seq": seq, "encoding": "ansi", "width": 80,
             "height": 24, "full": False, "bytes": base64.b64encode(data).decode()}
    out.write(json.dumps(frame).encode() + b"\\n")
    out.flush()
"""


class Failure(RuntimeError):
    pass


def shows(data: bytes, letter: str) -> bool:
    return letter.encode() in ESCAPES.sub(b"", data)


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
    lines = [f"{'path':46}  {'min':>8}  {'median':>8}  {'p95':>8}  {'max':>8}"]
    for row in rows:
        lines.append(
            f"{row['name']:46}  {row['min_ms']:8.2f}  {row['median_ms']:8.2f}  "
            f"{row['p95_ms']:8.2f}  {row['max_ms']:8.2f}"
        )
    return "\n".join(lines)


def load() -> str:
    return "load average " + " ".join(f"{value:.2f}" for value in os.getloadavg())


class _RusageInfo(ctypes.Structure):
    _fields_ = [
        ("uuid", ctypes.c_uint8 * 16),
        ("user_time", ctypes.c_uint64),
        ("system_time", ctypes.c_uint64),
        ("rest", ctypes.c_uint64 * 8),
    ]


class _Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


_LIBC = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
_TIMEBASE = _Timebase()
_LIBC.mach_timebase_info(ctypes.byref(_TIMEBASE))


def cpu_seconds(pids: list[int]) -> float:
    """Summed user+system CPU time of these processes, to the nanosecond.

    From `proc_pid_rusage` rather than `ps`, whose hundredths of a second are coarser than
    what one keystroke costs. The times come in Mach absolute units, which on Apple silicon
    are not nanoseconds.
    """
    total = 0
    for pid in pids:
        info = _RusageInfo()
        if _LIBC.proc_pid_rusage(pid, 0, ctypes.byref(info)) == 0:
            total += info.user_time + info.system_time
    return total * _TIMEBASE.numer / _TIMEBASE.denom / 1e9


def children(pid: int) -> list[int]:
    out = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout
    return [int(line) for line in out.split()]


def drain_fd(fd: int, seconds: float = 0.05) -> int:
    drained = 0
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not select.select([fd], [], [], max(0.0, deadline - time.monotonic()))[0]:
            break
        chunk = os.read(fd, 65536)
        if not chunk:
            break
        drained += len(chunk)
    return drained


# MARK: the floor


class PlainPty:
    """`cat` on a PTY with nothing between it and the reader."""

    def __init__(self):
        self.parent, child = pty.openpty()
        self.process = subprocess.Popen(
            ["cat"], stdin=child, stdout=child, stderr=child, close_fds=True
        )
        os.close(child)
        time.sleep(0.3)
        drain_fd(self.parent)

    def sample(self, letter: str, timeout: float) -> float:
        start = time.perf_counter_ns()
        os.write(self.parent, letter.encode())
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not select.select([self.parent], [], [], max(0.0, deadline - time.monotonic()))[0]:
                break
            if letter.encode() in os.read(self.parent, 4096):
                return (time.perf_counter_ns() - start) / 1_000_000
        raise Failure(f"the PTY never echoed {letter!r} within {timeout}s")

    def settle(self) -> None:
        drain_fd(self.parent, 0.02)

    def close(self) -> None:
        self.process.terminate()
        self.process.wait(timeout=5)
        os.close(self.parent)


# MARK: herdr's own client


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
        self.late = 0
        self.pending = b""

    def send(self, text: str) -> None:
        self.process.stdin.write((json.dumps({"type": "terminal.input", "text": text}) + "\n").encode())
        self.process.stdin.flush()

    def read_frame(self) -> dict:
        # In chunks, as the bridge reads, and never `readline` on the pipe: unbuffered, that
        # is a read per byte, and on a loaded machine a few thousand of those per frame
        # measured this path at 20 ms against the bridge's 2 in the same run.
        while b"\n" not in self.pending:
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise Failure("the pane's stream ended mid-measurement")
            self.pending += chunk
        line, _, self.pending = self.pending.partition(b"\n")
        return json.loads(line)

    def readable(self) -> bool:
        return b"\n" in self.pending or bool(select.select([self.process.stdout], [], [], 0)[0])

    def sample(self, letter: str, timeout: float) -> tuple[float, float]:
        """Time to the first frame, and to the frame that paints the letter."""
        start = time.perf_counter_ns()
        self.send(letter)
        seen, first = 0, None
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            frame = self.read_frame()
            elapsed = (time.perf_counter_ns() - start) / 1_000_000
            if frame.get("type") != "terminal.frame":
                continue
            seen += 1
            if first is None:
                first = elapsed
            if shows(base64.b64decode(frame.get("bytes", "")), letter):
                if seen > 1:
                    self.late += 1
                return first, elapsed
        raise Failure(
            f"no frame carried {letter!r} within {timeout}s. Either the pane is not running "
            "`cat`, or the daemon stopped rendering."
        )

    def drain(self, seconds: float) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if b"\n" in self.pending:
                self.read_frame()
                continue
            if not select.select([self.process.stdout], [], [], max(0.0, deadline - time.monotonic()))[0]:
                return
            self.read_frame()

    def settle(self) -> None:
        self.drain(0.02)

    def pids(self) -> list[int]:
        return [self.process.pid]

    def close(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()


# MARK: Muster's bridge


class BridgePath:
    """The real `muster-bridge`, driven the way the app drives it.

    This script plays the app: it listens on the control socket the bridge dials, writes
    `terminal.input` lines onto it exactly as the core's control channel does, and reads
    the bridge's output off a PTY whose slave is the bridge's stdout - the position a
    libghostty surface's PTY holds in the real window.
    """

    def __init__(
        self, daemon: IsolatedDaemon, bridge: Path, pane: str, root: Path,
        herdr: str | None = None,
    ):
        socket_path = root / f"b-{pane.replace(':', '-')}.sock"
        socket_path.unlink(missing_ok=True)
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(socket_path))
        listener.listen(1)

        self.parent, child = pty.openpty()
        fcntl.ioctl(child, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        env = dict(daemon.env)
        # As the app launches it: logging to a file, which puts the bridge at debug and so
        # writes a line per keystroke - part of what a keystroke costs in the shipped app.
        env["MUSTER_LOG_FILE"] = str(root / "bridge.jsonl")
        self.process = subprocess.Popen(
            [str(bridge), pane, "--control-socket", str(socket_path),
             "--herdr-socket", str(daemon.socket_path),
             "--herdr-binary", herdr or daemon.herdr_bin,
             "--pane-name", pane],
            stdin=child, stdout=child, stderr=child, env=env, close_fds=True,
        )
        os.close(child)
        listener.settimeout(10)
        try:
            self.app, _ = listener.accept()
        except socket.timeout as exc:
            raise Failure(
                f"muster-bridge never dialed {socket_path}. Check {root / 'bridge.jsonl'} for "
                "bridge.control.failed or bridge.herdr.failed."
            ) from exc
        finally:
            listener.close()
        self.app.setblocking(False)
        self.surface_bytes = 0

    def send(self, text: str) -> None:
        line = json.dumps({"type": "terminal.input", "bytes": base64.b64encode(text.encode()).decode()})
        self.app.setblocking(True)
        self.app.sendall((line + "\n").encode())
        self.app.setblocking(False)

    def sample(self, letter: str, timeout: float) -> float:
        start = time.perf_counter_ns()
        self.send(letter)
        seen = b""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not select.select([self.parent], [], [], max(0.0, deadline - time.monotonic()))[0]:
                break
            chunk = os.read(self.parent, 65536)
            self.surface_bytes += len(chunk)
            seen += chunk
            if shows(seen, letter):
                return (time.perf_counter_ns() - start) / 1_000_000
        raise Failure(f"the bridge never painted {letter!r} onto its surface within {timeout}s")

    def settle(self) -> None:
        self.surface_bytes += drain_fd(self.parent, 0.02)
        # The bridge reports what it painted on the socket; left unread, a long run would
        # fill the buffer and stall the bridge's reporting thread.
        try:
            while self.app.recv(65536):
                pass
        except BlockingIOError:
            pass

    def pids(self) -> list[int]:
        return [self.process.pid, *children(self.process.pid)]

    def close(self) -> None:
        self.app.close()
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
        os.close(self.parent)


class EchoDirect:
    """The stand-in daemon read directly, to price the stand-in itself."""

    def __init__(self, script: Path):
        self.process = subprocess.Popen(
            [str(script)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0
        )

    def sample(self, letter: str, timeout: float) -> float:
        start = time.perf_counter_ns()
        line = json.dumps({"type": "terminal.input", "bytes": base64.b64encode(letter.encode()).decode()})
        self.process.stdin.write((line + "\n").encode())
        pending = b""
        deadline = time.monotonic() + timeout
        while b"\n" not in pending and time.monotonic() < deadline:
            pending += os.read(self.process.stdout.fileno(), 65536)
        if not shows(base64.b64decode(json.loads(pending.partition(b"\n")[0])["bytes"]), letter):
            raise Failure("the stand-in daemon echoed something other than what it was sent")
        return (time.perf_counter_ns() - start) / 1_000_000

    def settle(self) -> None:
        pass

    def close(self) -> None:
        self.process.terminate()
        self.process.wait(timeout=5)


def echo_daemon(root: Path) -> Path:
    script = root / "echo-daemon"
    script.write_text(f"#!{sys.executable}\n{ECHO_DAEMON}")
    script.chmod(0o755)
    return script


def measure_hop(samples: int, timeout: float, direct: EchoDirect, bridge: BridgePath) -> dict:
    got: dict[str, list[float]] = {"direct": [], "bridge": []}
    for index in range(samples):
        letter = ALPHABET[index % len(ALPHABET)]
        for key, path in (("direct", direct), ("bridge", bridge))[:: 1 if index % 2 else -1]:
            got[key].append(path.sample(letter, timeout))
            path.settle()
            time.sleep(0.01)
    return got


def measure_output(daemon: IsolatedDaemon, bridge_bin: Path, pane: str, herdr_pid: int) -> dict:
    """A pane printing at an agent's pace: what the program wrote, what the surface parsed.

    The surface parses what herdr re-rendered rather than what the program wrote, so the ratio
    of the two is the per-byte cost the surface pays over plain Ghostty's.
    """
    lines = 300
    text = "the quick brown fox jumps over the lazy dog"
    wrote = sum(len(f"line {i}: {text}\r\n") for i in range(lines))
    bridge = BridgePath(daemon, bridge_bin, pane, ROOT)
    try:
        time.sleep(0.5)
        bridge.settle()
        bridge.surface_bytes = 0
        herdr_cpu, bridge_cpu = cpu_seconds([herdr_pid]), cpu_seconds([bridge.process.pid])
        client_cpu = cpu_seconds(children(bridge.process.pid))
        began = time.monotonic()
        run(daemon, pane, f"i=0; while [ $i -lt {lines} ]; do echo \"line $i: {text}\"; "
            "i=$((i+1)); sleep 0.01; done")
        quiet_since = time.monotonic()
        while time.monotonic() - quiet_since < 1.0 and time.monotonic() - began < 60:
            before = bridge.surface_bytes
            bridge.settle()
            if bridge.surface_bytes != before:
                quiet_since = time.monotonic()
            time.sleep(0.05)
        seconds = quiet_since - began
        return {
            "program_bytes": wrote,
            "surface_bytes": bridge.surface_bytes,
            "seconds": seconds,
            "herdr_cpu_ms": (cpu_seconds([herdr_pid]) - herdr_cpu) * 1000,
            "herdr_client_cpu_ms": (cpu_seconds(children(bridge.process.pid)) - client_cpu) * 1000,
            "bridge_cpu_ms": (cpu_seconds([bridge.process.pid]) - bridge_cpu) * 1000,
            "load": load(),
        }
    finally:
        bridge.close()


# MARK: the window


def split(daemon: IsolatedDaemon, pane: str) -> str:
    made = daemon.client().request(
        "pane.split", {"target_pane_id": pane, "direction": "right", "cwd": "/tmp"}
    )
    return made["pane"]["pane_id"]


def run(daemon: IsolatedDaemon, pane: str, command: str) -> None:
    # Text and then Enter as a key: text alone arrives as a bracketed paste, and a shell
    # leaves a pasted newline on the line to edit rather than running it
    # (`observations/herdr-0.8.0.md` section 25).
    daemon.client().request("pane.send_input", {"pane_id": pane, "text": command, "keys": ["enter"]})


def start_cat(daemon: IsolatedDaemon, pane: str) -> None:
    # Same inner program on every path. A shell would put its line editor's redraw in the
    # measurement, and the line editor is not what is being compared.
    run(daemon, pane, "exec cat")


def arrow(daemon: IsolatedDaemon, pane: str) -> float:
    start = time.perf_counter_ns()
    daemon.client(timeout=5).request("pane.send_input", {"pane_id": pane, "keys": ["up"]})
    return (time.perf_counter_ns() - start) / 1_000_000


def measure_round(
    samples: int, timeout: float, plain: PlainPty | None, stream: ControlStream,
    bridge: BridgePath, daemon: IsolatedDaemon, arrow_pane: str,
) -> dict[str, list[float]]:
    """Every path once per sample, in an order that rotates, so load lands on all alike."""
    got: dict[str, list[float]] = {"plain": [], "first": [], "herdr": [], "bridge": [], "arrow": []}
    for index in range(samples):
        letter = ALPHABET[index % len(ALPHABET)]
        steps = ["plain", "herdr", "bridge", "arrow"]
        if plain is None:
            steps.remove("plain")
        rotated = steps[index % len(steps):] + steps[: index % len(steps)]
        for step in rotated:
            if step == "plain":
                got["plain"].append(plain.sample(letter, timeout))
                plain.settle()
            elif step == "herdr":
                first, glyph = stream.sample(letter, timeout)
                got["first"].append(first)
                got["herdr"].append(glyph)
                stream.settle()
            elif step == "bridge":
                got["bridge"].append(bridge.sample(letter, timeout))
                bridge.settle()
            else:
                got["arrow"].append(arrow(daemon, arrow_pane))
            time.sleep(TYPING_GAP / len(rotated))
    return got


def printing(daemon: IsolatedDaemon, pane: str) -> None:
    # Paced rather than saturating: a busy window is agents printing, not `yes`. Twenty
    # lines a second each.
    run(daemon, pane, "while true; do date; sleep 0.05; done")


def attach_all(daemon: IsolatedDaemon, panes: list[str]) -> tuple[list[ControlStream], list[float]]:
    """A client on each pane, and how long each took to deliver its first frame."""
    streams, reattach = [], []
    for pane in panes:
        start = time.perf_counter_ns()
        stream = ControlStream(daemon, pane)
        stream.read_frame()
        reattach.append((time.perf_counter_ns() - start) / 1_000_000)
        streams.append(stream)
    return streams, reattach


def drain_all(streams: list[ControlStream], seconds: float) -> None:
    """Reads what every attached stream has queued, as a bridge would, so none blocks herdr."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        ready = select.select([s.process.stdout for s in streams], [], [], 0.05)[0]
        for out in ready:
            os.read(out.fileno(), 65536)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--herdr", required=True, help="the herdr binary to measure against")
    parser.add_argument("--bridge", required=True, help="a release build of muster-bridge")
    parser.add_argument("--samples", type=int, default=60)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument(
        "--panes", type=int, default=15,
        help="how full the window is for the crowded measurement (1 skips it)",
    )
    parser.add_argument("--json", action="store_true")
    options = parser.parse_args()

    for path, what in ((options.herdr, "--herdr"), (options.bridge, "--bridge")):
        if not os.access(path, os.X_OK):
            print(f"latency: {what} {path} is not an executable file.", file=sys.stderr)
            return 2

    subprocess.run(["rm", "-rf", str(ROOT)], check=False)
    ROOT.mkdir(parents=True, exist_ok=True)
    daemon = IsolatedDaemon(ROOT / "d", herdr_bin=str(Path(options.herdr).resolve()))
    daemon.prepare()
    daemon.start()
    herdr_pid = daemon._process.pid
    opened: list = []
    report: dict = {"load_before": load()}
    try:
        created = daemon.client().request(
            "workspace.create", {"cwd": "/tmp", "focus": True, "label": None}
        )
        stream_pane = created["root_pane"]["pane_id"]
        bridge_pane = split(daemon, stream_pane)
        arrow_pane = split(daemon, stream_pane)
        for pane in (stream_pane, bridge_pane, arrow_pane):
            start_cat(daemon, pane)
        time.sleep(1.0)

        plain = PlainPty()
        opened.append(plain)
        panes = (stream_pane, bridge_pane)

        def attach(swapped: bool) -> tuple[ControlStream, BridgePath]:
            """herdr's client on one pane and the bridge on the other.

            Swapped halfway through every phase, because the two panes do not echo alike: on
            a loaded machine one of them measured 15-20 ms slower than its neighbour through
            either path, for no reason visible from outside herdr. Swapping gives both paths
            the same panes.
            """
            first, second = reversed(panes) if swapped else panes
            # And whichever attaches first swaps too: herdr's first-attached client measured
            # slow far more often than the second, whichever pane each was on.
            if swapped:
                bridge = BridgePath(daemon, Path(options.bridge), second, ROOT)
                stream = ControlStream(daemon, first)
            else:
                stream = ControlStream(daemon, first)
                bridge = BridgePath(daemon, Path(options.bridge), second, ROOT)
            stream.read_frame()
            opened.extend([stream, bridge])
            stream.drain(0.8)
            bridge.settle()
            time.sleep(0.5)
            bridge.settle()
            bridge.surface_bytes = 0
            return stream, bridge

        def detach(*things) -> None:
            for thing in things:
                thing.close()
                opened.remove(thing)

        # One pane each. CPU is read around each half and split per keystroke.
        idle: dict[str, list[float]] = {}
        late = 0
        herdr_ms = bridge_ms = client_ms = surface = 0.0
        half = max(1, options.samples // 2)
        for swapped in (False, True):
            stream, bridge = attach(swapped)
            herdr_cpu = cpu_seconds([herdr_pid])
            bridge_cpu = cpu_seconds(bridge.pids())
            client_cpu = cpu_seconds(stream.pids())
            got = measure_round(half, options.timeout, plain, stream, bridge, daemon, arrow_pane)
            herdr_ms += (cpu_seconds([herdr_pid]) - herdr_cpu) * 1000
            bridge_ms += (cpu_seconds(bridge.pids()) - bridge_cpu) * 1000
            client_ms += (cpu_seconds(stream.pids()) - client_cpu) * 1000
            surface += bridge.surface_bytes
            late += stream.late
            for key, values in got.items():
                idle.setdefault(key, []).extend(values)
            detach(stream, bridge)
        keys = half * 2
        work = {
            # Three of the four paths in a sample go through herdr.
            "herdr_ms_per_key": herdr_ms / (keys * 3),
            "bridge_ms_per_key": bridge_ms / keys,
            "herdr_client_ms_per_key": client_ms / keys,
            "surface_bytes_per_key": surface / keys,
        }
        report["idle"] = {
            "rows": [
                summarize("plain pty (the floor)", idle["plain"]),
                summarize("herdr: stream responded", idle["first"]),
                summarize("herdr: glyph painted", idle["herdr"]),
                summarize("bridge: glyph on the surface pty", idle["bridge"]),
                summarize("arrow: pane.send_input answered", idle["arrow"]),
            ],
            "samples": keys,
            "late": late,
            "work": work,
            "load": load(),
        }

        output_pane = split(daemon, stream_pane)
        time.sleep(0.5)
        report["output"] = measure_output(daemon, Path(options.bridge), output_pane, herdr_pid)

        # Muster's own hop, with herdr taken out of it.
        script = echo_daemon(ROOT)
        direct = EchoDirect(script)
        hop_bridge = BridgePath(daemon, Path(options.bridge), "hop", ROOT, herdr=str(script))
        opened.extend([direct, hop_bridge])
        time.sleep(0.3)
        hop_bridge.settle()
        hop_bridge.surface_bytes = 0
        bridge_only = cpu_seconds([hop_bridge.process.pid])
        hop = measure_hop(options.samples, options.timeout, direct, hop_bridge)
        report["hop"] = {
            "rows": [
                summarize("stand-in daemon, read directly", hop["direct"]),
                summarize("bridge + stand-in, surface pty", hop["bridge"]),
            ],
            "bridge_ms_per_key": (cpu_seconds([hop_bridge.process.pid]) - bridge_only)
            * 1000 / options.samples,
            "added_median_ms": statistics.median(hop["bridge"]) - statistics.median(hop["direct"]),
            "load": load(),
        }
        detach(direct, hop_bridge)

        crowd_rows = None
        if options.panes > 3:
            extra = [split(daemon, stream_pane) for _ in range(options.panes - 3)]
            for pane in extra:
                printing(daemon, pane)
            time.sleep(1.0)
            rounds: dict[str, dict[str, list[float]]] = {"all": {}, "shown": {}}
            cpu: dict[str, list[float]] = {"all": [], "shown": []}
            reattach: list[float] = []
            block = max(10, options.samples // 4)
            # Alternating blocks, so a change in the machine's load lands on both, and the
            # measured panes swapped between the two pairs.
            for swapped in (False, True):
                stream, bridge = attach(swapped)
                for config in ("all", "shown"):
                    attached: list[ControlStream] = []
                    if config == "all":
                        attached, times = attach_all(daemon, extra)
                        reattach += times
                        opened.extend(attached)
                        drain_all(attached, 1.0)
                    else:
                        time.sleep(1.0)
                    before = cpu_seconds([herdr_pid])
                    began = time.monotonic()
                    got = measure_round_crowded(
                        block, options.timeout, stream, bridge, daemon, arrow_pane, attached
                    )
                    cpu[config].append(
                        (cpu_seconds([herdr_pid]) - before) / (time.monotonic() - began)
                    )
                    for key, values in got.items():
                        rounds[config].setdefault(key, []).extend(values)
                    detach(*attached)
                detach(stream, bridge)
            crowd_rows = {
                "rows": [
                    summarize(f"herdr glyph, {options.panes - 1} attached", rounds["all"]["herdr"]),
                    summarize("herdr glyph, 2 attached", rounds["shown"]["herdr"]),
                    summarize(f"bridge glyph, {options.panes - 1} attached", rounds["all"]["bridge"]),
                    summarize("bridge glyph, 2 attached", rounds["shown"]["bridge"]),
                    summarize(f"arrow answered, {options.panes - 1} attached", rounds["all"]["arrow"]),
                    summarize("arrow answered, 2 attached", rounds["shown"]["arrow"]),
                    summarize("reattach: spawn to first frame", reattach),
                ],
                "herdr_cores": {k: statistics.mean(v) for k, v in cpu.items()},
                "load": load(),
            }
            report["crowded"] = crowd_rows
    except Failure as exc:
        print(f"latency: {exc}", file=sys.stderr)
        return 1
    finally:
        for thing in opened:
            thing.close()
        daemon.stop()

    idle_report = report["idle"]
    print(f"One pane each, idle ({idle_report['load']})")
    print(render(idle_report["rows"]))
    print(
        f"\n{idle_report['late']} of {idle_report['samples']} herdr glyphs waited for a second frame "
        "(the 16 ms render throttle)."
    )
    work = idle_report["work"]
    print(
        f"Work per keystroke: herdr {work['herdr_ms_per_key']:.3f} ms CPU, herdr's client "
        f"{work['herdr_client_ms_per_key']:.3f} ms, muster-bridge and its client "
        f"{work['bridge_ms_per_key']:.3f} ms; {work['surface_bytes_per_key']:.0f} bytes onto "
        "the surface per echoed byte."
    )
    out = report["output"]
    print(
        f"\nA pane printing {out['program_bytes']} bytes over {out['seconds']:.1f} s "
        f"({out['load']}): {out['surface_bytes']} bytes reached the surface "
        f"({out['surface_bytes'] / out['program_bytes']:.2f}x). herdr spent "
        f"{out['herdr_cpu_ms']:.0f} ms of CPU, its client {out['herdr_client_cpu_ms']:.0f} ms, "
        f"the bridge {out['bridge_cpu_ms']:.0f} ms."
    )
    hop_report = report["hop"]
    print(f"\nMuster's bridge alone, against a stand-in daemon that echoes at once ({hop_report['load']})")
    print(render(hop_report["rows"]))
    print(
        f"\nThe bridge adds {hop_report['added_median_ms']:.2f} ms at the median and "
        f"{hop_report['bridge_ms_per_key']:.3f} ms of its own CPU per keystroke."
    )
    if crowd_rows is not None:
        print(f"\n{options.panes} panes, {options.panes - 3} printing ({crowd_rows['load']})")
        print(render(crowd_rows["rows"]))
        cores = crowd_rows["herdr_cores"]
        print(
            f"\nherdr CPU: {cores['all']:.3f} cores with every pane attached, "
            f"{cores['shown']:.3f} with only the two measured panes attached."
        )
    if options.json:
        print(json.dumps(report, indent=2))
    return 0


def measure_round_crowded(
    samples: int, timeout: float, stream: ControlStream, bridge: BridgePath,
    daemon: IsolatedDaemon, arrow_pane: str, attached: list[ControlStream],
) -> dict[str, list[float]]:
    """As `measure_round` without the floor, reading every attached stream between samples."""
    got: dict[str, list[float]] = {"herdr": [], "bridge": [], "arrow": []}
    for index in range(samples):
        letter = ALPHABET[index % len(ALPHABET)]
        steps = ["herdr", "bridge", "arrow"]
        rotated = steps[index % 3:] + steps[: index % 3]
        for step in rotated:
            if step == "herdr":
                got["herdr"].append(stream.sample(letter, timeout)[1])
                stream.settle()
            elif step == "bridge":
                got["bridge"].append(bridge.sample(letter, timeout))
                bridge.settle()
            else:
                got["arrow"].append(arrow(daemon, arrow_pane))
            drain_all(attached, TYPING_GAP / 3)
    return got


if __name__ == "__main__":
    sys.exit(main())
