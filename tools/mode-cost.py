#!/usr/bin/env python3
"""Which arrow encoding does a pager actually obey?

Muster encodes for a terminal in its power-on state, so arrows go out as CSI A/B even
after a program has turned application cursor mode on and started expecting SS3. Whether
that costs anything is a question about real programs, so this asks them - in a local
pty, with no daemon in the way.
"""

import os
import pty
import select
import subprocess
import sys
import time

PROGRAMS = [
    ("less", ["less", "/etc/services"]),
    ("vim", ["vim", "-u", "NONE", "/etc/services"]),
]
ARROWS = [("CSI B  (what Muster sends)", b"\x1b[B"), ("SS3 B  (what terminfo says)", b"\x1bOB")]


def drain(fd: int, seconds: float = 0.6) -> bytes:
    out = b""
    deadline = time.time() + seconds
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if ready:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    return out


def run(name: str, argv: list[str], payload: bytes) -> str:
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execvp(argv[0], argv)
    try:
        startup = drain(fd, 1.5)
        # Did the program ask for application cursor keys? This is the mode Muster cannot
        # see, because it is consumed by the daemon's terminal before any frame reaches us.
        appmode = b"\x1b[?1h" in startup
        os.write(fd, payload)
        response = drain(fd, 0.8)
        # The bytes themselves, because "it wrote something" is not "it scrolled": a bell
        # or a redrawn status line looks identical to movement by that measure.
        bell = "BELL " if b"\x07" in response else ""
        return f"app_mode={appmode!s:5} {bell}{len(response):4}B {response[:60]!r}"
    finally:
        os.write(fd, b"q")
        time.sleep(0.2)
        os.close(fd)
        try:
            os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            pass


for name, argv in PROGRAMS:
    if subprocess.run(["which", argv[0]], capture_output=True).returncode != 0:
        print(f"{name}: not installed, skipped")
        continue
    print(name)
    for label, payload in ARROWS:
        print(f"    {label:30} {run(name, argv, payload)}")
