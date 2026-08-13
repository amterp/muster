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


def check_agent_state_reaches_the_app(daemon: IsolatedDaemon, pane: str) -> None:
    """The founding desideratum, end to end, against a real daemon.

    Everything below this is verified elsewhere - the fold by corpus cases, the
    subscription by tests that spawn their own daemon - and every one of those could pass
    with the app wired up wrong. This is the only check that runs the whole chain the user
    does: daemon, subscription, mirror, seam, window.

    The transition is driven through pane.report_agent rather than by running an agent,
    because herdr's own detection screen-scrapes on a two-second timer and this is not a
    test of herdr's detection.
    """
    log_path = ROOT / "agentstate.jsonl"
    log_path.unlink(missing_ok=True)
    env = {**daemon.env, "MUSTER_LOG_FILE": str(log_path)}

    app = subprocess.Popen(
        [str(APP), pane], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    try:
        deadline = time.time() + 10.0
        while time.time() < deadline:
            if any(r["event"] == "mirror.bootstrap" for r in read_log(log_path)):
                break
            if app.poll() is not None:
                break
            time.sleep(0.2)

        client = daemon.client()
        for state in ("working", "blocked", "idle"):
            client.request(
                "pane.report_agent",
                {"pane_id": pane, "agent": "probe", "source": "probe", "state": state},
            )
            time.sleep(0.3)
        time.sleep(1.0)
        records = read_log(log_path)
    finally:
        app.terminate()
        try:
            app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            app.kill()

    expect(
        records,
        "mirror.bootstrap",
        "the app never built a picture of the daemon, so it knows no agent states at all",
    )
    transitions = [r for r in records if r["event"] == "agent.state"]
    if not transitions:
        raise Failure(
            "no `agent.state` record - the agent changed state three times and the app "
            "noticed none of it. Every pane shows what its agent is doing is the one thing "
            "this product is for.\n    the app logged: "
            + (", ".join(sorted({r["event"] for r in records})) or "(nothing at all)")
        )
    saw = [r.get("to") for r in transitions]
    for state in ("working", "blocked"):
        if state not in saw:
            raise Failure(
                f"the agent went {state} and the app never logged it. Saw: {saw}"
            )


def check_a_split_tab_becomes_splits(daemon: IsolatedDaemon, pane: str) -> None:
    """Every pane in the tab gets a surface, a channel and something painted into it.

    The one check that runs the whole chain for more than one pane: daemon, tree, mirror,
    composition, view, window. Everything under it is verified against a fake or a corpus,
    and every one of those could pass with the app wiring up one surface for a tab that has
    three - which is the failure this exists for, because a window showing one pane of three
    looks exactly like a session with one pane in it.

    The splits are made through herdr rather than through Muster's own keybinding, because a
    key equivalent needs a focused window and this runs headless. What Muster's side of that
    does is asserted in the seam's own test against a real daemon.
    """
    client = daemon.client()
    tab = client.request("pane.get", {"pane_id": pane})["pane"]["tab_id"]
    client.request("pane.split", {"target_pane_id": pane, "direction": "right", "cwd": "/tmp"})
    time.sleep(0.4)
    client.request("pane.split", {"target_pane_id": pane, "direction": "down", "cwd": "/tmp"})
    time.sleep(0.4)
    panes = [
        held["pane_id"]
        for held in client.request("pane.list", {})["panes"]
        if held["tab_id"] == tab
    ]
    if len(panes) != 3:
        raise Failure(f"the daemon was asked for three panes in the tab and holds {panes}")

    records = launch(daemon.env, [pane], "splits", settle=10.0)
    expect_no_errors(records)

    surfaced = {r.get("pane") for r in records if r["event"] == "surface.create"}
    missing = set(panes) - surfaced
    if missing:
        raise Failure(
            f"the tab holds {len(panes)} panes and the window built surfaces for "
            f"{sorted(surfaced)}. Nothing renders {sorted(missing)}, so those agents are "
            "invisible - which is the whole product."
        )

    # A surface that renders and swallows the keyboard is the failure that has cost this
    # project the most time, and it is invisible without asking per pane. `pane.typeable` is
    # the moment a bridge dialed back, which is the one that decides it.
    connected = {r.get("pane") for r in records if r["event"] == "pane.typeable"}
    silent = set(panes) - connected
    if silent:
        raise Failure(
            f"{sorted(silent)} rendered and no bridge dialed back, so those panes swallow "
            "every keystroke while looking alive"
        )

    # A bridge names itself in `process` rather than repeating the pane on every record, so
    # this is where its records are attributed back.
    painted = {
        r["process"].removeprefix("bridge:")
        for r in records
        if r["event"] == "bridge.frame.first"
    }
    if set(panes) - painted:
        raise Failure(f"{sorted(set(panes) - painted)} never painted, so they render empty")

    view = [r for r in records if r["event"] == "view.region"]
    if not view:
        raise Failure("the core never published a view, so the window was never told anything")
    # The tree, not just the count: three panes arranged flat and three panes nested are the
    # same number and a different window.
    tree = view[-1].get("tree", "")
    if "columns(" not in tree or "rows(" not in tree:
        raise Failure(
            f"the tab was split right and then down, and the core published {tree!r}. A tree "
            "with one axis in it means the reconstruction collapsed a level."
        )


def check_bad_pane(daemon: IsolatedDaemon) -> None:
    """A pane that does not exist must say so rather than showing a blank window."""
    records = launch(daemon.env, ["w9:p99"], "badpane")
    refused = expect(
        records,
        "core.refused",
        "a mistyped pane id was silent, which is how it reached a user as an empty window",
    )
    if refused.get("request") != "attach_pane":
        raise Failure(f"something other than the attach was refused: {refused}")
    # The id the user typed, so the log answers "which pane" without them re-running it.
    if "w9:p99" not in refused.get("reason", ""):
        raise Failure(f"the refusal did not name the pane that was asked for: {refused}")


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
            (
                "an agent's state reaches the window",
                lambda: check_agent_state_reaches_the_app(daemon, pane),
            ),
            ("a pane that does not exist says why", lambda: check_bad_pane(daemon)),
            ("a window with no pane admits it", lambda: check_bare_launch(daemon)),
            # Last, because it splits the tab the checks above are written against.
            (
                "a split tab becomes splits, all of them typeable",
                lambda: check_a_split_tab_becomes_splits(daemon, pane),
            ),
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
    print(
        "\nsmoke: the app launches, connects, paints, renders a split tab as splits, and "
        "shows what its agents are doing."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
