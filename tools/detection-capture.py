#!/usr/bin/env python3
"""Record Claude Code's screens for corpus/conformance/agent-detection-recorded.json.

Runs `claude` in a pseudo-terminal of a fixed size, walks it through the states detection has
to tell apart - the folder trust prompt, idle at its prompt, menus opened from the prompt,
working on a request, blocked on a Bash permission prompt, idle again after the prompt is
refused, and at work with its prompt box empty, holding a draft, and with a message queued -
and writes everything
it printed to <out>/raw, with the byte offset each state was reached at in <out>/marks.json.

The bytes are then rendered by the terminal the daemon runs, which is what makes the fixtures
recorded rather than transcribed:

    python3 tools/detection-capture.py <out>
    cargo run -q -p muster-detect --example render_capture -- <out>

The second prints the corpus cases. Read them before committing: a screen can carry a name,
an email or a path, and anything a rule does not read should be scrubbed.

It spends two short requests against whatever account `claude` is logged in to, and runs it
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

    def mark(name: str, expect: str, note: str, skip: bool = False) -> None:
        marks.append({"name": name, "offset": len(raw), "expect": expect, "note": note, "skip": skip})
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

        # Menus someone opens from the prompt, whose footers read like a dialog waiting on
        # them. Neither agent nor person is waiting on the other: detection leaves the state
        # as it was.
        # Their titles are drawn in pieces, so there is no text to wait for.
        for command in ["hooks", "mcp", "memory"]:
            send(f"/{command}")
            pump(1)
            send("\r")
            pump(4)
            mark(
                f"the /{command} menu",
                "unknown",
                f"/{command} opened from the prompt: nobody is waiting on anybody, and the state stays what it was.",
                skip=True,
            )
            if command == "hooks":
                # An event's matchers, then a matcher's hooks, end in the same footer. The
                # first event with a hook on the recording machine is the one opened.
                for level in ["an event's matchers", "a matcher's hooks"]:
                    send("\r")
                    pump(3)
                    mark(f"the /hooks menu, {level}", "unknown", f"/hooks, {level}: still a menu someone opened.", skip=True)
                send("\x1b")
                pump(1)
                send("\x1b")
                pump(1)
            send("\x1b")
            pump(2)

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

        # Work long enough to type into: what is typed while Claude Code works stays in its
        # prompt box, and a Return queues it for the running turn. No tool, so no dialog.
        send("Without using any tools, write a 600-word story about a lighthouse keeper.")
        pump(1)
        send("\r")
        pump(4)
        mark("working with an empty prompt box", "working", "Claude Code writing, its prompt box empty below the spinner.")
        send("and give it a title")
        pump(1.5)
        mark("working with a draft in the prompt box", "working", "Words typed into the prompt box while Claude Code works, not yet sent.")
        send("\r")
        pump(1.5)
        mark("working with a message queued", "working", "The words sent with Return while Claude Code works: queued for the running turn.")
        pump(90, b"\xe2\x9c\xb3 ")
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
