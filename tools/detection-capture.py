#!/usr/bin/env python3
"""Record Claude Code's screens for corpus/conformance/agent-detection-recorded.json.

Runs `claude` in a pseudo-terminal of a fixed size, walks it through the states detection has
to tell apart - the folder trust prompt, idle at its prompt, working on a request, blocked on
a Bash permission prompt, and idle again after the prompt is refused - and writes everything
it printed to <out>/raw, with the byte offset each state was reached at in <out>/marks.json.

The bytes are then rendered by the terminal the daemon runs, which is what makes the fixtures
recorded rather than transcribed:

    python3 tools/detection-capture.py <out>
    cargo run -q -p muster-detect --example render_capture -- <out>

The second prints the corpus cases. Read them before committing: a screen can carry a name,
an email or a path, and anything a rule does not read should be scrubbed.

It spends one short request against whatever account `claude` is logged in to, and runs it
in <out>, which it trusts when asked. Nothing is sent to the network by this script itself.
"""

from __future__ import annotations

import json
import os
import pty
import select
import signal
import struct
import sys
import termios
import time
import fcntl
from pathlib import Path

COLUMNS, ROWS = 100, 30


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1]).resolve()
    out.mkdir(parents=True, exist_ok=True)

    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(out)
        # A capture run from inside an agent inherits that agent's session markers, and
        # Claude Code takes them as a reason to skip its prompts - the very screens wanted.
        for name in [name for name in os.environ if name.startswith(("CLAUDE", "MUSTER_"))]:
            del os.environ[name]
        os.environ["TERM"] = "xterm-256color"
        os.execvp("claude", ["claude", "--permission-mode", "default"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLUMNS, 0, 0))

    raw = bytearray()
    marks: list[dict] = []

    def pump(seconds: float, until: bytes | None = None) -> bool:
        deadline = time.monotonic() + seconds
        seen_from = len(raw)
        while time.monotonic() < deadline:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    return False
                if not chunk:
                    return False
                raw.extend(chunk)
                if until is not None and until in raw[seen_from:]:
                    return True
        return until is None

    def mark(name: str, expect: str, note: str) -> None:
        marks.append({"name": name, "offset": len(raw), "expect": expect, "note": note})
        print(f"marked {name} at {len(raw)} bytes", file=sys.stderr)

    def send(text: str) -> None:
        os.write(fd, text.encode())

    try:
        if pump(30, b"trust"):
            pump(1.5)
            mark("folder trust prompt", "blocked", "Claude Code asks before it works in a folder it has not seen.")
            send("\r")
        pump(10, b"shortcuts")
        pump(3)
        mark("idle at the prompt", "idle", "Claude Code waiting at its prompt box for a request.")

        # A command that asks permission on any account, and harms nothing if it is allowed:
        # the directory does not exist, and would be inside the capture's own folder anyway.
        send("Run this exact shell command with the Bash tool and nothing else: rm -rf ./no-such-dir")
        pump(1)
        send("\r")
        pump(2.5)
        mark("working on a request", "working", "Claude Code thinking about the request, its spinner in the title.")

        if pump(90, b"proceed"):
            pump(1.5)
            mark("bash permission prompt", "blocked", "Claude Code asking whether it may run the command.")
            send("\x1b")
            pump(4)
            mark("idle after refusing", "idle", "The prompt refused with Esc; Claude Code back at its prompt.")
    finally:
        os.kill(pid, signal.SIGTERM)
        pump(1)
        os.close(fd)
        os.waitpid(pid, 0)

    (out / "raw").write_bytes(bytes(raw))
    (out / "marks.json").write_text(
        json.dumps({"columns": COLUMNS, "rows": ROWS, "marks": marks}, indent=2) + "\n"
    )
    print(f"wrote {len(raw)} bytes and {len(marks)} marks to {out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
