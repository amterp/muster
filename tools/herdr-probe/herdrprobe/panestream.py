"""The pane data plane: `herdr terminal session control|observe`.

This is the channel Muster's renderer surfaces will be fed from, so what it carries
is the most load-bearing thing the probe measures. The protocol, read off herdr's
client (src/client/mod.rs):

  out (stdout, NDJSON)
    {"type":"terminal.frame","seq":N,"encoding":"ansi","width":W,"height":H,
     "full":bool,"bytes":"<base64 ANSI>"}
    {"type":"terminal.closed","reason":"..."}

  in (stdin, NDJSON, control sessions only)
    {"type":"terminal.input","text":"..."}   or  {"type":"terminal.input","bytes":"<b64>"}
    {"type":"terminal.resize","cols":N,"rows":N,"cell_width_px":N,"cell_height_px":N}
"""

from __future__ import annotations

import base64
import json
import subprocess
import threading
import time
from pathlib import Path
from typing import Callable


class PaneStream:
    """A `terminal session control|observe` subprocess, with its frames captured."""

    def __init__(self, daemon, target: str, mode: str = "control", cols: int = 80, rows: int = 24):
        self.target = target
        self.mode = mode
        self.cols = cols
        self.rows = rows
        args = daemon.herdr_argv("terminal", "session", mode, target, "--cols", str(cols), "--rows", str(rows))
        self._proc = subprocess.Popen(
            args, env=daemon.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, bufsize=0,
        )
        self.frames: list[dict] = []
        self.stderr: list[str] = []
        self._cv = threading.Condition()
        self._t0 = time.monotonic()
        threading.Thread(target=self._read_frames, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()

    def _read_frames(self) -> None:
        for line in self._proc.stdout:
            try:
                frame = json.loads(line)
            except ValueError:
                continue
            frame["_t_ms"] = int((time.monotonic() - self._t0) * 1000)
            with self._cv:
                self.frames.append(frame)
                self._cv.notify_all()

    def _read_stderr(self) -> None:
        for line in self._proc.stderr:
            self.stderr.append(line.decode(errors="replace").rstrip())

    def send_input_text(self, text: str) -> None:
        self._write({"type": "terminal.input", "text": text})

    def send_input_bytes(self, data: bytes) -> None:
        self._write({"type": "terminal.input", "bytes": base64.b64encode(data).decode()})

    def resize(self, cols: int, rows: int) -> None:
        self._write({"type": "terminal.resize", "cols": cols, "rows": rows})

    def _write(self, obj: dict) -> None:
        self._proc.stdin.write((json.dumps(obj) + "\n").encode())
        self._proc.stdin.flush()

    def wait_for_frames(self, count: int, timeout: float = 5.0) -> bool:
        with self._cv:
            return self._cv.wait_for(lambda: len(self.frames) >= count, timeout)

    def wait_quiet(self, quiet_for: float = 0.6, timeout: float = 5.0) -> None:
        """Wait until no new frame has arrived for `quiet_for` seconds."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._cv:
                before = len(self.frames)
                self._cv.wait(quiet_for)
                if len(self.frames) == before:
                    return

    def decoded(self) -> list[bytes]:
        return [base64.b64decode(f["bytes"]) for f in self.frames if f.get("type") == "terminal.frame"]

    def snapshot(self) -> list[dict]:
        with self._cv:
            return list(self.frames)

    def close(self) -> None:
        try:
            if self._proc.stdin:
                self._proc.stdin.close()
        except OSError:
            pass
        try:
            self._proc.terminate()
            self._proc.wait(timeout=5)
        except (subprocess.TimeoutExpired, OSError):
            self._proc.kill()

    def __enter__(self) -> PaneStream:
        return self

    def __exit__(self, *_exc) -> None:
        self.close()


# Mode-setting sequences a real agent TUI emits. If these survive into the frame
# stream, a surface could track pane modes and encode input locally; if the daemon
# consumes them, input encoding cannot live in the surface (architecture.md).
MODE_SEQUENCES: dict[str, bytes] = {
    "bracketed_paste_2004": b"\x1b[?2004h",
    "alt_screen_1049": b"\x1b[?1049h",
    "kitty_keyboard_push": b"\x1b[>1u",
    "mouse_1000": b"\x1b[?1000h",
    "mouse_sgr_1006": b"\x1b[?1006h",
    "focus_reporting_1004": b"\x1b[?1004h",
}
