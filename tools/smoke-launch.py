#!/usr/bin/env python3
"""Does the app actually come up, and say so?

Every part of Muster's launch had a test except the launch. The end-to-end input check
opened the control socket and spawned the bridge itself, so it proved the transport while
the app's own wiring never ran once - and two failures reached a user through that hole: a
mistyped pane id gave a blank window and a silent exit, and a bare `muster` dropped every
keystroke without saying so.

This spawns the real binary against an isolated daemon and reads the run log, which is a
machine-readable account of what the app did. No keyboard needed, so it says nothing about
what typing does; it says the app stood up and connected the things it has to connect.

Not part of `./dev` - the default gate stays offline and deterministic (docs/testing.md).
Run it with `./dev --contract`.
"""

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "tools/herdr-probe"))

from herdrprobe.daemon import IsolatedDaemon  # noqa: E402

APP = REPO / ".build/arm64-apple-macosx/debug/muster"
ROOT = Path("/private/tmp/muster-smoke")


class Failure(Exception):
    pass


def read_log(path: Path) -> list[dict]:
    if not path.exists():
        return []
    records = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            records.append(json.loads(line))
        except json.JSONDecodeError as exc:
            # A torn line is itself a finding: the log is what everything else here
            # asserts on, so it has to survive concurrent writers.
            raise Failure(f"unparseable log line: {line[:120]!r} ({exc})") from exc
    return records


def launch(env: dict, args: list[str], name: str, settle: float = 6.0) -> list[dict]:
    """Runs the app until it reports readiness, then stops it and returns its log."""
    log_path = ROOT / f"{name}.jsonl"
    log_path.unlink(missing_ok=True)
    env = {**env, "MUSTER_LOG_FILE": str(log_path)}

    app = subprocess.Popen(
        [str(APP), *args], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    try:
        deadline = time.time() + settle
        while time.time() < deadline:
            if any(r["event"] in ("app.ready", "app.setup.failed") for r in read_log(log_path)):
                # Ready is not settled: the bridge still has to start, dial back and paint.
                time.sleep(2.0)
                break
            if app.poll() is not None:
                break
            time.sleep(0.2)
        return read_log(log_path)
    finally:
        app.terminate()
        try:
            app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            app.kill()


def expect(records: list[dict], event: str, why: str) -> dict:
    for record in records:
        if record["event"] == event:
            return record
    seen = ", ".join(sorted({r["event"] for r in records})) or "(nothing at all)"
    raise Failure(f"no `{event}` record - {why}\n    the app logged: {seen}")


def expect_no_errors(records: list[dict]) -> None:
    errors = [r for r in records if r["level"] == "error"]
    if errors:
        detail = "\n".join(f"      {r['event']}: {r}" for r in errors[:5])
        raise Failure(f"{len(errors)} error record(s):\n{detail}")


def check_healthy_launch(daemon: IsolatedDaemon, pane: str) -> None:
    """The whole chain: app binds, bridge starts, dials back, and paints."""
    records = launch(daemon.env, [pane], "healthy")
    expect_no_errors(records)
    expect(records, "app.ready", "the app never finished launching")
    expect(
        records,
        "channel.connected",
        "the bridge never dialed back, so the pane would swallow every keystroke",
    )
    expect(
        records,
        "bridge.frame.first",
        "nothing was ever painted, so the window would be empty",
    )
    # Socket discovery is reimplemented from herdr's own rules, so it is worth proving
    # against a daemon whose socket is somewhere unusual - which the isolated one is.
    expect(
        records,
        "server_channel.ready",
        "the daemon socket was not found, so arrows and paste fall back to a guess",
    )
    if records[-1]["event"] == "app.ready":
        raise Failure("the app logged nothing after startup, so nothing was running")


def check_bad_pane(daemon: IsolatedDaemon) -> None:
    """A pane that does not exist must say so rather than showing a blank window."""
    records = launch(daemon.env, ["w9:p99"], "badpane")
    expect(
        records,
        "bridge.attach.failed",
        "a mistyped pane id was silent, which is how it reached a user as an empty window",
    )
    failure = expect(records, "bridge.attach.failed", "")
    if "not found" not in failure.get("reason", ""):
        raise Failure(f"the failure did not carry herdr's reason: {failure}")


def check_bare_launch(daemon: IsolatedDaemon) -> None:
    """A window with no pane must admit it cannot be typed into."""
    records = launch(daemon.env, [], "bare")
    ready = expect(records, "app.ready", "the app never finished launching")
    if ready.get("typeable") != "false":
        raise Failure(
            "a bare `muster` claimed to be typeable, but it has no control stream to "
            "put keystrokes on"
        )


def main() -> int:
    if not APP.exists():
        print(f"smoke: {APP} is missing. Run `./dev -b` first.", file=sys.stderr)
        return 2

    shutil.rmtree(ROOT, ignore_errors=True)
    ROOT.mkdir(parents=True, exist_ok=True)
    daemon = IsolatedDaemon(ROOT / "d")
    daemon.prepare()
    daemon.start()
    failures = 0
    try:
        created = daemon.client().request(
            "workspace.create", {"cwd": "/tmp", "focus": True, "label": None}
        )
        pane = created["root_pane"]["pane_id"]
        time.sleep(0.5)

        checks = [
            ("a named pane comes up and paints", lambda: check_healthy_launch(daemon, pane)),
            ("a pane that does not exist says why", lambda: check_bad_pane(daemon)),
            ("a window with no pane admits it", lambda: check_bare_launch(daemon)),
        ]
        for title, check in checks:
            try:
                check()
                print(f"  ok    {title}")
            except Failure as exc:
                failures += 1
                print(f"  FAIL  {title}\n    {exc}")
    finally:
        daemon.stop()

    if failures:
        print(
            f"\nsmoke: {failures} check(s) failed.\n"
            "These are wiring failures - the app started but did not connect something it "
            "must. The full log for each check is under "
            f"{ROOT}/<check>.jsonl.",
            file=sys.stderr,
        )
        return 1
    print("\nsmoke: the app launches, connects and paints.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
