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
import re
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

# What the probe's own daemon is configured with, for the one check here that starts a daemon
# without it - the app's, on a scratch home of its own.
_ISOLATED_HERDR_CONFIG = """\
[terminal]
default_shell = "/bin/sh"
shell_mode = "non_login"
new_cwd = "current"

[update]
version_check = false
manifest_check = false
"""


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


def pointed_at(daemon: IsolatedDaemon) -> dict:
    """The app's environment, with a config file naming this daemon.

    The only way to point Muster at a daemon it did not start. It runs its own herdr on a
    session of its own and does not read HERDR_SOCKET_PATH, so a scratch daemon has to be
    asked for the way a person would ask for one - by naming its socket in the config file.
    """
    config = ROOT / "muster.toml"
    config.write_text(f'[[daemon]]\nid = "local"\nsocket = "{daemon.socket_path}"\n')
    return {**daemon.env, "MUSTER_CONFIG": str(config)}


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


def expect_nothing_wrong(records: list[dict], expected: tuple[str, ...] = ()) -> None:
    """Every warning and error the app raised, unless this scenario declared it.

    Warnings count, and that is the point. Muster's log lines carry their own impact - a
    `pane.surface.deferred` says "this pane is blank until the core opens its channel" in the
    record itself - so a run that raises one has already diagnosed a bug nobody has to think
    of in advance. The blank-window bug of 2026-08-15 logged exactly that, at warn, while
    every check here passed.

    Declared rather than filtered by level, so that a scenario about a refusal says which
    refusal it is about and a second unrelated one still fails the run.
    """
    wrong = [r for r in records if r["level"] in ("warn", "error") and r["event"] not in expected]
    if wrong:
        detail = "\n".join(f"      {r['level']}: {r['event']}: {r}" for r in wrong[:5])
        more = f"\n      ... and {len(wrong) - 5} more" if len(wrong) > 5 else ""
        raise Failure(
            f"{len(wrong)} record(s) the app itself called wrong:\n{detail}{more}\n"
            "    If one of these is expected here, name it in this check's `expected`."
        )


def expect_every_pane_painted(records: list[dict]) -> None:
    """Every pane the window was told to show got a surface, and something on it.

    The gap this closes: `app.ready` with `typeable=true` is the core's answer to "is there a
    pane the keyboard would go to", and it stays true while the window shows nothing at all.
    What a person sees is a surface with bytes on it, and that has its own records.
    """
    view = [r for r in records if r["event"] == "view.region"]
    if not view:
        raise Failure("the core never published a view, so the window was never told anything")

    # Read out of the published tree, which is the only place the shell's own list of panes
    # appears in the log. Pane ids are `w<n>:p<n>` in every daemon Muster talks to.
    wanted = set()
    for region in view:
        wanted |= set(re.findall(r"w\d+:p\d+", region.get("tree", "")))
    if not wanted:
        raise Failure(
            "the core published a view naming no panes at all, so the window is empty and "
            f"the last thing it said it was showing was {view[-1].get('tree')!r}"
        )

    surfaced = {r.get("pane") for r in records if r["event"] == "surface.create"}
    if wanted - surfaced:
        raise Failure(
            f"the window was told to show {sorted(wanted)} and built a surface for "
            f"{sorted(surfaced)}. {sorted(wanted - surfaced)} render as empty squares."
        )

    # A bridge names itself in `process` rather than repeating the pane on every record.
    painted = {
        r["process"].removeprefix("bridge:")
        for r in records
        if r["event"] == "bridge.frame.first"
    }
    if wanted - painted:
        raise Failure(
            f"{sorted(wanted - painted)} got a surface and never painted a frame, so they "
            "are blank squares in a window that believes it is showing them"
        )


def check_healthy_launch(daemon: IsolatedDaemon, pane: str) -> None:
    """The whole chain: app binds, bridge starts, dials back, and paints."""
    records = launch(pointed_at(daemon), [pane], "healthy")
    expect_nothing_wrong(records)
    expect_every_pane_painted(records)
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
    env = {**pointed_at(daemon), "MUSTER_LOG_FILE": str(log_path)}

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
    # A second tab, whose pane no region will show. Nothing below renders it, which is the
    # point: it is the pane the sidebar exists for, and the one a window alone loses.
    client.request("tab.create", {"cwd": "/tmp"})
    time.sleep(0.4)
    panes = [
        held["pane_id"]
        for held in client.request("pane.list", {})["panes"]
        if held["tab_id"] == tab
    ]
    if len(panes) != 3:
        raise Failure(f"the daemon was asked for three panes in the tab and holds {panes}")
    everything = client.request("pane.list", {})["panes"]
    if len(everything) != 4:
        raise Failure(
            "the daemon was asked for a fourth pane in a tab of its own and holds "
            f"{[held['pane_id'] for held in everything]}"
        )

    records = launch(pointed_at(daemon), [pane], "splits", settle=10.0)
    expect_nothing_wrong(records)
    expect_every_pane_painted(records)

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

    # The list, which is the half of "every agent at a glance" a window cannot carry: the
    # fourth pane is in a tab no region shows, so nothing on screen says anything about it.
    # Counts rather than ids because these two discriminate on their own - four panes with
    # three of them showing is the arrangement, and any other pair means the roster is
    # describing a different session from the one the daemon holds.
    roster = [r for r in records if r["event"] == "roster.received"]
    if not roster:
        raise Failure(
            "the window was never handed a roster, so nothing lists the panes no region is "
            "showing - which is exactly the pane most likely to have finished unnoticed"
        )
    listed, shown = roster[-1].get("panes"), roster[-1].get("on_screen")
    if (listed, shown) != ("4", "3"):
        raise Failure(
            f"the daemon holds four panes with three of them on screen, and the window was "
            f"handed {listed} panes with {shown} on screen. A roster that agrees with the "
            "window instead of with the session lists nothing worth surfacing."
        )


def check_bad_pane(daemon: IsolatedDaemon) -> None:
    """A pane that does not exist must say so rather than showing a blank window."""
    records = launch(pointed_at(daemon), ["w9:p99"], "badpane")
    # The one refusal this is about, and nothing else. A second unrelated warning here would
    # be a real finding hiding inside a scenario whose whole subject is a refusal.
    expect_nothing_wrong(records, expected=("core.refused",))
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
    """A bare `muster` opens a usable window, which is what double-clicking sends.

    It used to render the user's shell and drop every keystroke, because the only way in was
    to know a pane id and pass it. This is the check that the ordinary way to open the app is
    the ordinary way to open the app.
    """
    records = launch(pointed_at(daemon), [], "bare")
    expect_nothing_wrong(records)
    expect_every_pane_painted(records)
    ready = expect(records, "app.ready", "the app never finished launching")
    if ready.get("typeable") != "true":
        raise Failure(
            "a bare `muster` came up with nothing to type into, so double-clicking the app "
            "gives a window that ignores the keyboard"
        )
    expect(
        records,
        "channel.connected",
        "the bridge never dialed back, so the pane would swallow every keystroke",
    )


def check_cold_start() -> None:
    """No daemon, no config, nothing: the app has to produce a window anyway.

    The first launch on a machine, and the reason Muster carries a herdr at all. Nothing is
    running, nothing names a socket, and the app has to start its own daemon, ask it for a
    workspace, and end up with a pane somebody can type into. Every other check here is
    handed a daemon that already exists.

    Its own scratch home, so the daemon this starts is not the developer's - and stopped
    afterwards, since the whole point of the thing is that it outlives the app.
    """
    root = ROOT / "cold"
    shutil.rmtree(root, ignore_errors=True)
    for directory in ("home", "config/herdr", "state", "data", "cache"):
        (root / directory).mkdir(parents=True, exist_ok=True)
    # The same pinning the probe's daemon does, and for the same reason: a login shell under
    # a scratch HOME exits nonzero, which closes the pane, then the workspace, then the
    # server - so the check would be measuring the fixture rather than the app.
    (root / "config/herdr/config.toml").write_text(_ISOLATED_HERDR_CONFIG)
    env = {
        **os.environ,
        "HOME": str(root / "home"),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_STATE_HOME": str(root / "state"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "TERM": "xterm-256color",
    }
    for stale in ("HERDR_SOCKET_PATH", "HERDR_CLIENT_SOCKET_PATH", "HERDR_SESSION", "MUSTER_CONFIG"):
        env.pop(stale, None)

    socket = root / "config/herdr/sessions/muster/herdr.sock"
    try:
        # Longer than the rest: this one waits on a daemon starting from nothing.
        records = launch(env, [], "cold", settle=20.0)
        expect_nothing_wrong(records)
        expect_every_pane_painted(records)
        expect(
            records,
            "daemon.started",
            "no daemon was started, so a first launch on a clean machine shows nothing",
        )
        expect(
            records,
            "workspace.creating",
            "a daemon was started and never asked for a workspace, so the window is empty",
        )
        ready = expect(records, "app.ready", "the app never finished launching")
        if ready.get("typeable") != "true":
            raise Failure(
                "a cold start produced a window with nothing to type into - the daemon "
                "started, but no pane reached the keyboard"
            )
        if not socket.exists():
            raise Failure(f"the daemon did not bind Muster's own socket at {socket}")
    finally:
        subprocess.run(
            [str(APP.parent / "herdr"), "server", "stop"],
            env={**env, "HERDR_SESSION": "muster"},
            capture_output=True,
            check=False,
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
            ("a bare launch opens a usable window", lambda: check_bare_launch(daemon)),
            ("a clean machine gets a daemon and a workspace", check_cold_start),
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
        "\nsmoke: the app launches, connects, paints, renders a split tab as splits, shows "
        "what its agents are doing, lists the panes nothing is showing, and comes up on a "
        "machine with no daemon by starting one."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
