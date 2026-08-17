"""Corpus writer.

Every scenario writes raw transcripts, not summaries: a reviewer must be able to
re-derive any verdict in the findings doc from these bytes. Timestamps are
milliseconds since scenario start, so two runs diff cleanly.
"""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any


class Recorder:
    def __init__(self, corpus_dir: Path, scenario: str, herdr_version: str, protocol: int,
                 platform: str):
        """`platform` is the machine the daemon runs on, which is not always this one.

        Asked for rather than read off this process, because a remote run records a Linux
        daemon from a Mac - and a recording that stamped the wrong platform made the whole
        Linux corpus claim to be Darwin/arm64. Nothing noticed, because nothing read it;
        diff-corpus now does, to decide which facts are comparable across the two.
        """
        self.dir = Path(corpus_dir) / scenario
        self.dir.mkdir(parents=True, exist_ok=True)
        self.scenario = scenario
        self._t0 = time.monotonic()
        self._notes: list[str] = []
        self._facts: dict[str, Any] = {}
        self._appended: set[str] = set()
        self.write_json(
            "META.json",
            {
                "scenario": scenario,
                "herdr_version": herdr_version,
                "herdr_protocol": protocol,
                "platform": platform,
                "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            },
        )

    def t_ms(self) -> int:
        return int((time.monotonic() - self._t0) * 1000)

    def append_ndjson(self, name: str, obj: dict) -> None:
        """Appends within one run, and starts fresh on the next one.

        Append mode is what makes a transcript ordered, and on its own it also made one
        accumulate: `META.json`, `FACTS.json` and `NOTES.txt` are whole-file writes, so a
        second run replaced those and concatenated onto this. What got committed then reads
        as one exchange and is several, with `t_ms` resetting partway down - which is worse
        than a stale file, because the facts beside it describe only the last run.
        """
        if name not in self._appended:
            self._appended.add(name)
            (self.dir / name).unlink(missing_ok=True)
        with (self.dir / name).open("a") as f:
            f.write(json.dumps({"t_ms": self.t_ms(), **obj}, sort_keys=True) + "\n")

    def write_json(self, name: str, obj: Any) -> None:
        (self.dir / name).write_text(json.dumps(obj, indent=2, sort_keys=True) + "\n")

    def write_text(self, name: str, text: str) -> None:
        (self.dir / name).write_text(text)

    def write_bytes(self, name: str, data: bytes) -> None:
        (self.dir / name).write_bytes(data)

    def note(self, line: str) -> None:
        """A human-readable breadcrumb, ordered with the transcript."""
        self._notes.append(f"[{self.t_ms():>6}ms] {line}")
        try:
            print(f"    {line}", flush=True)
        except BrokenPipeError:
            # `probe lifecycle | head` closes stdout partway through, and letting that
            # abort the run leaves a half-written recording that looks like a real
            # capture. The transcript is the output that matters; the console echo is
            # a convenience and may be dropped.
            pass

    def fact(self, key: str, value: Any) -> None:
        """A machine-checkable observation the findings doc can cite."""
        self._facts[key] = value

    def recall(self, key: str) -> Any:
        """Read back a fact, for scenarios that compare a later state to an earlier one."""
        return self._facts[key]

    def finish(self) -> dict[str, Any]:
        if self._notes:
            self.write_text("NOTES.txt", "\n".join(self._notes) + "\n")
        self.write_json("FACTS.json", self._facts)
        return self._facts


class RecordingClient:
    """Wraps a Client so every request and response lands in the transcript.

    The daemon-facing oracle testing.md asks for is the exact intent messages on the
    wire, so this records both directions verbatim - including failures, which are
    observations too.
    """

    def __init__(self, client, recorder: Recorder, name: str = "wire.ndjson"):
        self._client = client
        self._rec = recorder
        self._name = name

    def request(self, method: str, params: dict | None = None) -> Any:
        self._rec.append_ndjson(self._name, {"dir": "out", "method": method, "params": params or {}})
        try:
            result = self._client.request(method, params)
        except Exception as exc:
            self._rec.append_ndjson(self._name, {"dir": "in", "method": method, "error": str(exc)})
            raise
        self._rec.append_ndjson(self._name, {"dir": "in", "method": method, "result": result})
        return result

    def try_request(self, method: str, params: dict | None = None) -> tuple[bool, Any]:
        """Request without raising - for probing whether a method or shape is supported."""
        try:
            return True, self.request(method, params)
        except Exception as exc:
            return False, str(exc)

    def __getattr__(self, item):
        return getattr(self._client, item)
