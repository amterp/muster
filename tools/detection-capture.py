#!/usr/bin/env python3
"""Record a harness's screens for corpus/conformance/agent-detection-recorded.json.

Runs the harness in a pseudo-terminal of a fixed size and walks it through the states detection
has to tell apart, writing everything it printed to <out>/raw, and the byte offset each state
was reached at to <out>/marks.json, with the harness's id and version.

    python3 tools/detection-capture.py [--harness claude] [--size 100x30] [--only a,b] <out>
    cargo run -q -p muster-detect --example render_capture -- <out>

The second renders each marked moment through the terminal the daemon runs and prints the corpus
cases, named "<harness> <version>: <state>". Read them before committing: a screen can carry a
name, an email or a path, and anything a rule does not read should be scrubbed.

Claude Code's phases, in order: `start` (the folder trust prompt, idle at its prompt), `menus`
(menus opened from the prompt), `permission` (working on a request, blocked on a Bash
permission prompt, idle after refusing it), `queue` (at work with its prompt box empty, holding a
draft, and with a message queued), `plan` (plan mode's approval dialog, at work right after
approving it, idle after declining another). `--only` runs the named phases after `start`, and
`--size` a narrower pane, where a dialog's question wraps.

It spends a few short requests against whatever account the harness is logged in to, and runs it
in <out>, which it trusts when asked. Nothing is sent to the network by this script itself.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


class Capture:
    def __init__(self, out: Path, argv: list[str], strip: tuple[str, ...], columns: int, rows: int):
        self.columns, self.rows = columns, rows
        self.raw = bytearray()
        self.marks: list[dict] = []
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(out)
            # A capture run from inside an agent inherits that agent's session markers, and
            # a harness takes them as a reason to skip its prompts - the very screens wanted.
            for name in [name for name in os.environ if name.startswith(strip)]:
                del os.environ[name]
            os.environ["TERM"] = "xterm-256color"
            os.execvp(argv[0], argv)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        self.pid, self.fd = pid, fd

    def pump(self, seconds: float, until: bytes | None = None) -> bool:
        deadline = time.monotonic() + seconds
        seen_from = len(self.raw)
        while time.monotonic() < deadline:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    return False
                if not chunk:
                    return False
                self.raw.extend(chunk)
                if until is not None and until in self.raw[seen_from:]:
                    return True
        return until is None

    def mark(self, name: str, expect: str, note: str, skip: bool = False) -> None:
        self.marks.append({"name": name, "offset": len(self.raw), "expect": expect, "note": note, "skip": skip})
        print(f"marked {name} at {len(self.raw)} bytes", file=sys.stderr)

    def send(self, text: str) -> None:
        os.write(self.fd, text.encode())

    def end(self) -> None:
        os.kill(self.pid, signal.SIGTERM)
        self.pump(1)
        os.close(self.fd)
        os.waitpid(self.pid, 0)


@dataclass
class Harness:
    argv: list[str]
    # Environment variables a parent agent leaves behind, by prefix.
    strip: tuple[str, ...]
    start: Callable[[Capture], None]
    phases: dict[str, Callable[[Capture], None]]


# --- Claude Code -------------------------------------------------------------------------------


def claude_start(capture: Capture) -> None:
    if capture.pump(30, b"trust"):
        capture.pump(1.5)
        capture.mark("folder trust prompt", "blocked", "Claude Code asks before it works in a folder it has not seen.")
        capture.send("\r")
    capture.pump(10, b"shortcuts")
    capture.pump(3)
    capture.mark("idle at the prompt", "idle", "Claude Code waiting at its prompt box for a request.")


def claude_menus(capture: Capture) -> None:
    # Menus someone opens from the prompt, whose footers read like a dialog waiting on them.
    # Neither agent nor person is waiting on the other: detection leaves the state as it was.
    # Their titles are drawn in pieces, so there is no text to wait for.
    for command in ["hooks", "mcp", "memory"]:
        capture.send(f"/{command}")
        capture.pump(1)
        capture.send("\r")
        capture.pump(4)
        capture.mark(
            f"the /{command} menu",
            "unknown",
            f"/{command} opened from the prompt: nobody is waiting on anybody, and the state stays what it was.",
            skip=True,
        )
        if command == "hooks":
            # An event's matchers, then a matcher's hooks, end in the same footer. The first
            # event with a hook on the recording machine is the one opened.
            for level in ["an event's matchers", "a matcher's hooks"]:
                capture.send("\r")
                capture.pump(3)
                capture.mark(f"the /hooks menu, {level}", "unknown", f"/hooks, {level}: still a menu someone opened.", skip=True)
            capture.send("\x1b")
            capture.pump(1)
            capture.send("\x1b")
            capture.pump(1)
        capture.send("\x1b")
        capture.pump(2)


def claude_permission(capture: Capture) -> None:
    # A command that asks permission on any account, and harms nothing if it is allowed: the
    # directory does not exist, and would be inside the capture's own folder anyway.
    capture.send("Run this exact shell command with the Bash tool and nothing else: rm -rf ./no-such-dir")
    capture.pump(1)
    capture.send("\r")
    capture.pump(2.5)
    capture.mark("working on a request", "working", "Claude Code thinking about the request, its spinner in the title.")
    if capture.pump(90, b"proceed"):
        capture.pump(1.5)
        capture.mark("bash permission prompt", "blocked", "Claude Code asking whether it may run the command.")
        capture.send("\x1b")
        capture.pump(4)
        capture.mark("idle after refusing", "idle", "The prompt refused with Esc; Claude Code back at its prompt.")


def claude_queue(capture: Capture) -> None:
    # Work long enough to type into: what is typed while Claude Code works stays in its prompt
    # box, and a Return queues it for the running turn. No tool, so no dialog.
    capture.send("Without using any tools, write a 600-word story about a lighthouse keeper.")
    capture.pump(1)
    capture.send("\r")
    capture.pump(4)
    capture.mark("working with an empty prompt box", "working", "Claude Code writing, its prompt box empty below the spinner.")
    capture.send("and give it a title")
    capture.pump(1.5)
    capture.mark("working with a draft in the prompt box", "working", "Words typed into the prompt box while Claude Code works, not yet sent.")
    capture.send("\r")
    capture.pump(1.5)
    capture.mark("working with a message queued", "working", "The words sent with Return while Claude Code works: queued for the running turn.")
    capture.pump(90, b"\xe2\x9c\xb3 ")


PLAN = (
    "Plan to create a file named {name} holding the word hi. The plan is one line. Do not explore "
    "anything: call the ExitPlanMode tool with that plan right away."
)


def claude_plan(capture: Capture) -> None:
    # Shift+Tab cycles the permission mode: from the default, twice is plan mode.
    for _ in range(2):
        capture.send("\x1b[Z")
        capture.pump(1)
    capture.pump(3, b"plan mode on")
    size = f"{capture.columns} columns"
    for name, answer in [("approved.txt", "\r"), ("declined.txt", "\x1b")]:
        capture.send(PLAN.format(name=name))
        capture.pump(1)
        capture.send("\r")
        if not capture.pump(120, b"proceed"):
            return
        capture.pump(2)
        if answer == "\r":
            capture.mark(
                "plan approval dialog",
                "blocked",
                f"Plan mode's ExitPlanMode dialog, asking whether to go ahead with the plan, in a pane {size} wide.",
            )
            capture.send(answer)
            capture.pump(0.8)
            capture.mark(
                "working right after approving the plan",
                "working",
                "The plan approved with Return: Claude Code at work on it, the dialog's question gone from below the last rule.",
            )
            capture.pump(90, b"\xe2\x9c\xb3 ")
            capture.pump(2)
            # Approving left plan mode; back into it for the second plan.
            for _ in range(3):
                capture.send("\x1b[Z")
                capture.pump(1)
                if b"plan mode on" in capture.raw[-2000:]:
                    break
        else:
            capture.send(answer)
            capture.pump(4)
            capture.mark("idle after declining the plan", "idle", "The plan dialog left with Esc; Claude Code back at its prompt, still planning.")


CLAUDE = Harness(
    argv=["claude", "--permission-mode", "default"],
    strip=("CLAUDE", "MUSTER_"),
    start=claude_start,
    phases={"menus": claude_menus, "permission": claude_permission, "queue": claude_queue, "plan": claude_plan},
)

HARNESSES = {"claude": CLAUDE}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out", type=Path)
    parser.add_argument("--harness", choices=sorted(HARNESSES), default="claude")
    parser.add_argument("--size", default="100x30", help="COLUMNSxROWS")
    parser.add_argument("--only", help="comma-separated phases to run after the start")
    arguments = parser.parse_args()
    harness = HARNESSES[arguments.harness]
    columns, rows = (int(part) for part in arguments.size.split("x"))
    phases = list(harness.phases) if arguments.only is None else arguments.only.split(",")
    unknown = [phase for phase in phases if phase not in harness.phases]
    if unknown:
        sys.exit(f"{arguments.harness} has no phase {', '.join(unknown)}; it has {', '.join(harness.phases)}")
    out = arguments.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    said = subprocess.run([harness.argv[0], "--version"], capture_output=True, text=True).stdout
    version = re.search(r"\d+(?:\.\d+)+", said)
    version = version.group(0) if version else said.strip()

    capture = Capture(out, harness.argv, harness.strip, columns, rows)
    try:
        harness.start(capture)
        for phase in phases:
            harness.phases[phase](capture)
    finally:
        capture.end()

    (out / "raw").write_bytes(bytes(capture.raw))
    (out / "marks.json").write_text(
        json.dumps(
            {"agent": arguments.harness, "version": version, "columns": columns, "rows": rows, "marks": capture.marks},
            indent=2,
        )
        + "\n"
    )
    print(f"wrote {len(capture.raw)} bytes and {len(capture.marks)} marks to {out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
