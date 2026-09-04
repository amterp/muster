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
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "tools/herdr-probe"))

from herdrprobe.client import Client  # noqa: E402
from herdrprobe.daemon import IsolatedDaemon  # noqa: E402

APP = REPO / ".build/arm64-apple-macosx/debug/muster"

# The same app as it ships, which is a different layout and not a cosmetic one: the daemon
# moves into a helper bundle in Contents/Library, and nothing sits beside the bridge.
BUNDLED_APP = REPO / ".build/muster.app/Contents/MacOS/muster"
ROOT = Path("/private/tmp/muster-smoke")


def pinned_herdr() -> str:
    """The daemon `deps/herdr.pin` names, which is what every other tier runs against.

    Not `herdr` on PATH, which is what this tier used to reach for. A contract tier judged
    against whatever daemon the developer happens to have installed is judged against a
    version nothing here recorded, and on a machine set up the way this project intends there
    is no herdr on PATH at all - the stop that silently did nothing (a_2I7ASgulK) was that,
    exiting quietly under check=False.
    """
    pin = json.loads((REPO / "deps/herdr.pin").read_text())
    binary = REPO / "deps/herdr" / pin["version"] / "herdr"
    if not binary.is_file():
        sys.exit(
            f"smoke: no pinned herdr at {binary}.\n"
            f"  Impact: this tier has no daemon to launch the app against.\n"
            f"  Fix: ./dev -t downloads and verifies it."
        )
    return str(binary)


def stop_daemon(socket: Path, env: dict | None = None) -> None:
    """Ends the daemon listening on one socket, by the one spelling that works.

    `HERDR_SOCKET_PATH` rather than a `--socket` option herdr 0.8.0 does not have, and rather
    than the session variable, which `scratch_home` pops - so the CLI resolved the default
    session while the daemon sat on `muster` and found no socket to talk to. Both spellings
    exited quietly and left the daemon running, which is how this went unnoticed for as long
    as it did (a_2I7ASgulK).

    One helper rather than a line per check, because three call sites got it three ways and
    only one of them worked.
    """
    if not socket.exists():
        return
    subprocess.run(
        [pinned_herdr(), "server", "stop"],
        env={**(env or os.environ), "HERDR_SOCKET_PATH": str(socket)},
        capture_output=True,
        check=False,
    )


def sockets_under(root: Path) -> list[Path]:
    """Every herdr socket beneath a directory, whoever put it there."""
    return sorted(root.glob("**/herdr.sock")) if root.is_dir() else []

# What launchd gives a GUI process, which is what an app opened from the Dock, Finder or
# Spotlight actually has. Every directory on it is SIP-protected, so a herdr cannot be put on it
# even deliberately - which is why a bundle whose bridge has to *find* a daemon cannot work, and
# why the bundle check below runs with this rather than with the developer's PATH. With a real
# PATH the check would pass on any machine that happens to have a herdr installed.
LAUNCHD_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"

# What the app is configured with for the one check here that starts a daemon of its own, on a
# scratch home of its own.
#
# Muster's file rather than herdr's, and that is the point rather than a detail. The app writes
# the daemon's config now, so a `[terminal]` block put where herdr's own config lives would be
# read by nobody - the daemon is told to read Muster's file instead. Pinning it here means the
# fixture goes through the same translation a person's settings do, so a launch that ended up
# running the wrong shell would fail this check rather than passing it quietly.
_ISOLATED_MUSTER_CONFIG = """\
[shell]
command = "/bin/sh"
mode = "non_login"
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


def stop(app: subprocess.Popen) -> None:
    """Stops the app and the bridges it spawned.

    The whole group, because a bridge is a child of the app and holds herdr's attach on its pane -
    and herdr refuses a second client on an attached terminal. Terminating only the app leaves the
    bridge holding it, so the *next* check to open the same pane fails with `already has an
    attached client`, which reads as a wiring bug in the launch it happens to land on.

    Each app is started in a session of its own for this, so the group is exactly its own.
    """
    try:
        os.killpg(os.getpgid(app.pid), signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        app.terminate()
    try:
        app.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(app.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            app.kill()


def launch(
    env: dict, args: list[str], name: str, settle: float = 6.0, app_path: Path = APP
) -> list[dict]:
    """Runs the app until it reports readiness, then stops it and returns its log.

    `app_path` is the SwiftPM binary for every check but one. The bundle is a different
    layout - the daemon lives in a helper bundle rather than beside the bridge - and that
    difference is a shipped bug rather than a detail (kan a_2Hnh3g0Y5).
    """
    log_path = ROOT / f"{name}.jsonl"
    log_path.unlink(missing_ok=True)
    env = {**env, "MUSTER_LOG_FILE": str(log_path)}

    app = subprocess.Popen(
        [str(app_path), *args],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
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
        stop(app)


def expect(records: list[dict], event: str, why: str) -> dict:
    for record in records:
        if record["event"] == event:
            return record
    seen = ", ".join(sorted({r["event"] for r in records})) or "(nothing at all)"
    raise Failure(f"no `{event}` record - {why}\n    the app logged: {seen}")


# The two warnings this tier cannot avoid, and must not be declared per scenario.
#
# Both are about how the tier launches Muster rather than about anything a check is doing.
# A notification permission is granted against a bundle identifier signed by a Developer ID:
# a binary out of `.build` has no identifier at all, and the ad-hoc signature `--bundle`
# applies is refused outright by macOS (`docs/observations/macos-26.4.1.md`). So every check
# raises one or the other, whichever way it launches, and no check here can ever raise
# neither.
#
# Listed once because the cause is one thing. Naming them in each scenario's `expected` would
# read as seven separate decisions and would quietly stop a scenario that genuinely wanted to
# assert notifications work - which none can, and `a_2IneJXhwU` is where that gap is tracked.
UNGRANTABLE_NOTIFICATIONS = ("notifications.unbundled", "notifications.refused")


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
    allowed = (*expected, *UNGRANTABLE_NOTIFICATIONS)
    wrong = [r for r in records if r["level"] in ("warn", "error") and r["event"] not in allowed]
    if wrong:
        detail = "\n".join(f"      {r['level']}: {r['event']}: {r}" for r in wrong[:5])
        more = f"\n      ... and {len(wrong) - 5} more" if len(wrong) > 5 else ""
        raise Failure(
            f"{len(wrong)} record(s) the app itself called wrong:\n{detail}{more}\n"
            "    If one of these is expected here, name it in this check's `expected`."
        )


# Muster names for panes an earlier launch already learned, so that four checks against one
# daemon cost one extra launch rather than four.
_LEARNED: dict[str, str] = {}


def naming(records: list[dict]) -> dict[str, str]:
    """Which Muster name belongs to which pane the daemon holds.

    Muster mints its own pane names, so one log carries two vocabularies and neither is guessable
    from the other: the core's records say `p1w3r07bsd` and a bridge's say the `w1:p3` it streams
    from. That is deliberate - a bridge talks to the daemon - and `surface.create` is the one
    record naming both, because it says which pane it built and which command it spawned to paint
    it. So it is the join, and this script needs one to assert against the daemon.
    """
    found = {}
    for record in records:
        if record["event"] != "surface.create":
            continue
        backend = re.search(r"'(w\d+:p\d+)'", record.get("command", ""))
        if backend and record.get("pane"):
            found[backend.group(1)] = record["pane"]
    return found


def muster_name_of(daemon: IsolatedDaemon, backend_pane: str) -> str:
    """What Muster calls a pane the daemon is already holding.

    This script cannot predict one. A name is minted when Muster first hears about the pane, so
    the only way to learn it is to let the app look - which is also how a person gets one, from
    `muster window` or from a pane's own `$MUSTER_PANE`. A bare launch is enough, and the name
    survives into the next launch because Muster writes them down.
    """
    if backend_pane in _LEARNED:
        return _LEARNED[backend_pane]
    records = launch(pointed_at(daemon), [], "naming")
    found = naming(records)
    if backend_pane not in found:
        raise Failure(
            f"a bare launch never built a surface for {backend_pane}, so nothing here can learn "
            f"what Muster calls it. It named {sorted(found)}."
        )
    _LEARNED.update(found)
    return found[backend_pane]


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
    # appears in the log. Muster's own names: `p` and nine characters.
    wanted = set()
    for region in view:
        wanted |= set(re.findall(r"\bp[0-9a-z]{9}\b", region.get("tree", "")))
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

    # A bridge names itself in `process` rather than repeating the pane on every record, and it
    # names itself the way the daemon does - so its records are joined back through the surface
    # that spawned it.
    found = naming(records)
    painted = {
        found[r["process"].removeprefix("bridge:")]
        for r in records
        if r["event"] == "bridge.frame.first"
        and r["process"].removeprefix("bridge:") in found
    }
    if wanted - painted:
        raise Failure(
            f"{sorted(wanted - painted)} got a surface and never painted a frame, so they "
            "are blank squares in a window that believes it is showing them"
        )


def check_healthy_launch(daemon: IsolatedDaemon, pane: str) -> None:
    """The whole chain: app binds, bridge starts, dials back, and paints.

    Opened on a pane by name, which is the argument a person has: Muster's own name for it, from
    `muster window` or from the pane's own environment. Learning it takes a launch of its own, and
    that launch is itself worth something - the name has to survive into this one, or a pane could
    only ever be named while the app that minted it was still running.
    """
    records = launch(pointed_at(daemon), [muster_name_of(daemon, pane)], "healthy")
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


def check_the_bundle_paints_panes(daemon: IsolatedDaemon, pane: str) -> None:
    """The app as it ships, which is a layout no other check here runs against.

    kan a_2Hnh3g0Y5, and the reason it reached a release. `1d7ace3` moved the daemon into
    Contents/Library/MusterSessions.app and dropped the copy that had been going into
    Contents/MacOS, so a released bundle had no herdr beside its bridge - and the bridge
    looked for one there. Every pane of the 0.3.0 cask rendered nothing. The whole suite was
    green over it: `./dev` stages a daemon beside the SwiftPM binary, which is the one layout
    the old rule was right about, and every check above launches that binary.

    The PATH matters as much as the bundle. libghostty spawns the bridge, so a bridge inherits
    the *app's* environment, and an app opened by Launch Services has launchd's four
    SIP-protected directories and nothing else. Handing this check the developer's PATH would
    let it pass by finding a herdr somebody installed, which is exactly the state no user is in.

    Pointed at the daemon the other checks share, so nothing is started through Launch
    Services: what is under test is the bridge finding a daemon binary, not the app starting
    one.
    """
    if not BUNDLED_APP.exists():
        raise Failure(
            f"{BUNDLED_APP} is missing, so the one check that runs against a shipped layout "
            "ran nothing. `./dev --contract` assembles a bundle first; if it did not, that "
            "step is what broke."
        )
    environment = {**pointed_at(daemon), "PATH": LAUNCHD_PATH}
    records = launch(
        environment, [muster_name_of(daemon, pane)], "bundle", app_path=BUNDLED_APP
    )
    failed = [r for r in records if r["event"] == "bridge.herdr.failed"]
    if failed:
        raise Failure(
            "a bridge in the assembled bundle could not start a daemon: "
            f"{failed[0].get('herdr')!r} ({failed[0].get('error')}). Every pane of this bundle "
            "renders nothing, which is what a `brew install` produces. Either the bundle no "
            "longer carries a daemon where HerdrLocation.swift looks for it, or the app "
            "stopped telling each bridge which one to run."
        )
    expect_nothing_wrong(records)
    expect_every_pane_painted(records)


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

    # Named the way a person would name it; the reports below stay in the daemon's own
    # vocabulary, because they are sent to the daemon.
    app = subprocess.Popen(
        [str(APP), muster_name_of(daemon, pane)],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
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
        stop(app)

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

    records = launch(pointed_at(daemon), [muster_name_of(daemon, pane)], "splits", settle=10.0)
    expect_nothing_wrong(records)
    expect_every_pane_painted(records)

    # Asserted against the daemon's own ids, because the daemon is the oracle for "the tab holds
    # three panes" - so what the window built surfaces for is translated back rather than the
    # other way round.
    surfaced = set(naming(records))
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
    typeable = {r.get("pane") for r in records if r["event"] == "pane.typeable"}
    named = naming(records)
    connected = {backend for backend, muster in named.items() if muster in typeable}
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
    """A pane that does not exist must say so rather than showing a blank window.

    A well-formed name for a pane nobody holds, because that is the mistake somebody makes: a name
    copied from a window that has since closed the pane, or from another machine's notes.
    """
    records = launch(pointed_at(daemon), ["p1w3r07bsd"], "badpane")
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
    if "p1w3r07bsd" not in refused.get("reason", ""):
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


def scratch_home(name: str) -> tuple[Path, dict, Path]:
    """A home of Muster's own, so the daemon it starts is nobody's but this check's.

    Both checks that need Muster to run its own daemon need this, and they need it identical: what
    a pane is handed comes from the daemon, and the daemon comes from the launch.
    """
    root = ROOT / name
    shutil.rmtree(root, ignore_errors=True)
    for directory in ("home", "home/.muster", "config/herdr", "state", "data", "cache"):
        (root / directory).mkdir(parents=True, exist_ok=True)
    # The same pinning the probe's daemon does, and for the same reason: a login shell under
    # a scratch HOME exits nonzero, which closes the pane, then the workspace, then the
    # server - so the check would be measuring the fixture rather than the app.
    (root / "home/.muster/config.toml").write_text(_ISOLATED_MUSTER_CONFIG)
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
    return root, env, root / "config/herdr/sessions/muster/herdr.sock"


def check_a_pane_can_drive_its_own_window() -> None:
    """A program inside a pane runs `muster` and is answered by the window it is drawn in.

    Three things have to be true at once and each is invisible on its own: the app has to put a
    link to its CLI in `~/.muster/bin`, the daemon it starts has to carry that directory on the
    PATH it hands every pane, and the pane has to be handed `MUSTER_PANE` and `MUSTER_SOCKET` in
    the request that made it. Every one has a test of its own; nothing but this says they meet.

    A cold start, because only a daemon Muster started gets that PATH. The text is typed through
    herdr rather than through Muster's own endpoint, so what is being proved is the pane's own
    environment rather than a path this script already knows.
    """
    root, env, socket = scratch_home("driving")
    answered = root / "answered.txt"
    # A file of its own, and written first. `answered` is read as soon as it is non-empty and
    # its last line has to be `$MUSTER_PANE`, so a second command appending to it would be a
    # race this check loses roughly one run in three - measured, by adding one.
    census = root / "census.txt"
    app = subprocess.Popen(
        [str(APP)],
        env={**env, "MUSTER_LOG_FILE": str(ROOT / "driving.jsonl")},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        deadline = time.time() + 25.0
        while time.time() < deadline:
            if any(r["event"] == "app.ready" for r in read_log(ROOT / "driving.jsonl")):
                break
            if app.poll() is not None:
                raise Failure("the app exited before it was ready")
            time.sleep(0.2)

        link = root / "home/.muster/bin/muster"
        if not link.exists():
            raise Failure(
                f"the app put no muster command at {link}, so no pane can run one. A build "
                "stages it beside the app - try `./dev -b`."
            )

        client = Client(socket, root)
        panes = client.request("pane.list", {})["panes"]
        if len(panes) != 1:
            raise Failure(f"a cold start should leave one pane to type into, and left {panes}")
        pane = panes[0]["pane_id"]

        # Written to a file rather than read off the pane's screen: a grid wraps at its width and
        # carries the shell's own echo of the command, so reading one cannot tell an answer from
        # the question.
        client.request(
            "pane.send_text",
            {
                "pane_id": pane,
                "text": (
                    f"muster daemons > {census} 2>&1; "
                    f"muster window > {answered} 2>&1; echo $MUSTER_PANE >> {answered}"
                ),
            },
        )
        client.request("pane.send_input", {"pane_id": pane, "keys": ["enter"]})

        deadline = time.time() + 20.0
        while time.time() < deadline:
            if answered.exists() and answered.read_text().strip():
                break
            time.sleep(0.25)
        said = answered.read_text() if answered.exists() else ""
        if "connected" not in said:
            raise Failure(
                "a pane ran `muster window` and did not get a window back, so nothing running "
                f"in a pane can drive the window it is in. It got:\n{said or '(nothing at all)'}"
            )
        named = said.strip().splitlines()[-1]
        if not re.fullmatch(r"p[0-9a-z]{9}", named):
            raise Failure(
                f"the pane read $MUSTER_PANE as {named!r}, so a program inside it cannot say "
                "which pane it is and every command it sends would act on whichever pane the "
                "keyboard happens to be on"
            )
        if named not in said:
            raise Failure(
                f"the window did not list {named}, which is the pane that asked - so a pane's "
                f"own name is not one the window answers to. It said:\n{said}"
            )

        # And the other half of the same question, from the same pane: `muster window` says what
        # this window is attached to, `muster daemons` says what is on the machine. Only a cold
        # start can prove this one, because only a daemon Muster started is written down.
        counted = census.read_text() if census.exists() else ""
        if str(socket) not in counted:
            raise Failure(
                "a pane ran `muster daemons` and did not get back the daemon this window "
                f"started at {socket}, so nothing running in a pane can find out what is on "
                f"this machine before ending something. It got:\n{counted or '(nothing at all)'}"
            )
    finally:
        stop(app)
        stop_daemon(socket, env)


def check_cold_start() -> None:
    """No daemon, no config, nothing: the app has to produce a window anyway.

    The first launch on a machine, and the reason Muster carries a herdr at all. Nothing is
    running, nothing names a socket, and the app has to start its own daemon, ask it for a
    workspace, and end up with a pane somebody can type into. Every other check here is
    handed a daemon that already exists.

    Its own scratch home, so the daemon this starts is not the developer's - and stopped
    afterwards, since the whole point of the thing is that it outlives the app.
    """
    root, env, socket = scratch_home("cold")
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
        # The daemon reads a config the app derived from the config file above, rather than
        # whatever herdr config the machine happens to hold. Checked here because it is only
        # true end to end: the shell decides where that file goes, the core writes it, and the
        # daemon is told its name on the command that starts it.
        derived = root / "home/.muster/state/herdr.toml"
        starting = expect(
            records, "daemon.starting", "no daemon was started, so nothing was configured"
        )
        if starting.get("config") != str(derived):
            raise Failure(
                f"the daemon was pointed at {starting.get('config')!r} rather than the file "
                f"Muster derives at {derived}. Impact: what a pane runs and how deep its "
                "scrollback is come from a config file Muster did not write, and the pinned "
                "daemon's update checks are back on."
            )
        if not derived.exists():
            raise Failure(
                f"the app named {derived} as its daemon's config and wrote no such file, so "
                "the daemon fell back to defaults for everything including its update checks"
            )
        # And Muster wrote down the daemon it started, which is what `muster daemons` reads
        # back. Only a real launch takes this path - every other test points Muster at a daemon
        # somebody else started, and an adopted daemon is deliberately not written down.
        written = [
            record
            for record in sorted((root / "home/.muster/state/daemons").glob("*.toml"))
            if str(socket) in record.read_text()
        ]
        if not written:
            raise Failure(
                f"the app started a daemon on {socket} and wrote no record of it under "
                f"{root / 'home/.muster/state/daemons'}. Impact: `muster daemons` cannot name "
                "the daemon Muster started, which is the whole of what makes ending a stray "
                "one safe - the process holding somebody's live agent looks exactly like the "
                "nineteen that hold nothing."
            )
    finally:
        stop_daemon(socket, env)


def saved_composition_version() -> int:
    """The version the app writes into `window.toml`, read from the app's own constant.

    Restated here once and derived rather than typed, because typing it is what broke this
    check. The fixture below was written when the format was at 1, `284b505` moved it to 2 nine
    commits later, and nothing connected the two - so from that day the app refused the fixture,
    opened as a first launch, and this check staged nothing while looking like it staged
    something (kan a_2HSZuuZp4).
    """
    source = REPO / "crates/muster-core/src/composition/saved.rs"
    found = re.search(r"^const VERSION: i64 = (\d+);", source.read_text(), re.MULTILINE)
    if not found:
        raise Failure(
            f"no `const VERSION` in {source}, so this check cannot write a saved arrangement the "
            "app will read. Impact: the fixture below would be refused and the check would stage "
            "nothing. If the constant moved or was renamed, point this at its new home."
        )
    return int(found.group(1))


def check_a_broken_config_opens_the_roster() -> None:
    """A window that comes back with the roster closed still shows what is wrong with it.

    The launch-ordering bug of 2026-08-17, and the reason this check exists rather than a
    unit one. Both layers were tested and correct alone: `Problems::has_error` had its cases
    and `restore_presentation` had its own reasoning. Nothing owned the ORDER, so an error
    raised during startup read `session.presentation` to decide whether to open the roster,
    and `open()` replaced the whole of that a moment later with the saved `window.toml`. A
    window that came back with the roster closed and a broken config therefore opened
    nothing, silently - which is the exact failure the feature exists to prevent - and
    twenty-one tests were green over it.

    Both halves have to be staged for it to bite, which is why nothing smaller catches it: a
    config that will not parse, AND a saved presentation with the roster closed. So this
    writes its own home with both.
    """
    root = ROOT / "problems"
    shutil.rmtree(root, ignore_errors=True)
    for directory in ("home", "home/.muster", "home/.muster/state", "config/herdr", "state",
                      "data", "cache"):
        (root / directory).mkdir(parents=True, exist_ok=True)

    # Unreadable in a way the parser names, rather than unreadable as a file: the point is a
    # refusal that reaches the roster, and a missing file is not a refusal at all. The shell
    # block goes in too, on the same terms as every other check here - a login shell under a
    # scratch HOME takes the pane down with it.
    # Before the [shell] table, not after it: a bare key following a table header belongs to
    # that table, so appending this would refuse the file for an unknown key in [shell]
    # rather than for the value it names - the check would still pass, about something else.
    (root / "home/.muster/config.toml").write_text(
        'resize_step = "20"\n\n' + _ISOLATED_MUSTER_CONFIG
    )
    # The other half: a window remembered with the roster put away. Without this the roster
    # is open anyway and the bug cannot show.
    #
    # A version the app will accept, because a saved arrangement it refuses is one it ignores:
    # the window then opens as a first launch, whose roster is open, and the half this check is
    # staging is gone. It fails loudly when that happens - `composition.restore.failed` is not
    # in the expected list below and must never be added, because a run that declares it is a
    # run asserting nothing.
    (root / "home/.muster/state/window.toml").write_text(
        f"version = {saved_composition_version()}\n"
        "focused = 0\n\n[window]\nsidebar = false\nfont_size_offset = 0\n"
    )

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
        records = launch(env, [], "problems", settle=20.0)
        # The refusal is the fixture here, so it is declared. Anything else wrong is a real
        # finding and still fails.
        expect_nothing_wrong(records, expected=("config.refused", "config.unreadable"))
        opened = expect(
            records,
            "problems.sidebar.opened",
            "a broken config raised no roster, so the one thing that tells somebody their "
            "settings were refused never appeared - which is what shipped on 2026-08-17",
        )
        if "impact" not in opened:
            raise Failure(f"the record does not say what it cost: {opened}")
        # And the window is still a window. Opening the roster over a refused config must not
        # come at the price of the panes, which is the other way this could be "fixed".
        ready = expect(records, "app.ready", "the app never finished launching")
        if ready.get("typeable") != "true":
            raise Failure(
                "a refused config left a window with nothing to type into - the settings "
                "are meant to be ignored, not the session"
            )
    finally:
        stop_daemon(socket, env)


def end_what_the_last_run_left() -> None:
    """Ends any daemon still listening under ROOT from an earlier run.

    A run that was interrupted, or that crashed outside a `finally`, leaves one. It is
    harmless where it sits and becomes unreachable the moment ROOT is deleted, so this is the
    last moment anything can ask it to stop.
    """
    for socket in sockets_under(ROOT):
        stop_daemon(socket)


def leaked_daemons() -> int:
    """How many daemons this run left behind, named rather than counted.

    Checked here rather than left to `./dev --doctor`, because a leak that only shows up in a
    diagnostic somebody runs when already suspicious is a leak nobody finds. Four runs left
    eight strays before anything noticed (a_2I7ASgulK).

    A socket that still answers is the test. A daemon stopped cleanly takes its socket file
    with it, so a file left behind is either a live daemon or the litter of a killed one, and
    the ping is what tells those apart.
    """
    alive = []
    for socket in sockets_under(ROOT):
        try:
            Client(socket, ROOT, timeout=2.0).request("ping")
            alive.append(socket)
        except Exception:
            continue
    if not alive:
        return 0
    print(
        f"\n  FAIL  the tier left {len(alive)} daemon(s) running\n"
        + "".join(f"    {socket}\n" for socket in alive)
        + "    Each one holds a pane and outlives this run. The next run deletes these socket\n"
        + "    files, and a daemon whose socket is gone can only be ended with a signal - so\n"
        + "    strays accumulate silently. A check that starts a daemon has to end it in a\n"
        + "    `finally`, through stop_daemon() above.",
        file=sys.stderr,
    )
    return 1


def main() -> int:
    if not APP.exists():
        print(f"smoke: {APP} is missing. Run `./dev -b` first.", file=sys.stderr)
        return 2

    # Before the delete, not after, and that ordering is the third of this tier's three
    # daemon-leak faults (a_2I7ASgulK). Removing ROOT takes the previous run's socket files
    # with it, and a daemon whose socket path is gone cannot be reached through the API at
    # all - `./dev --doctor` reports it as unreachable and the only way left to end one is a
    # signal. Every check ends its own daemon in a `finally`, which is the mechanism; this is
    # the recovery for the run that was interrupted and never reached one.
    end_what_the_last_run_left()
    shutil.rmtree(ROOT, ignore_errors=True)
    ROOT.mkdir(parents=True, exist_ok=True)
    daemon = IsolatedDaemon(ROOT / "d", herdr_bin=pinned_herdr())
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
                "the app as it ships paints every pane",
                lambda: check_the_bundle_paints_panes(daemon, pane),
            ),
            (
                "an agent's state reaches the window",
                lambda: check_agent_state_reaches_the_app(daemon, pane),
            ),
            ("a pane that does not exist says why", lambda: check_bad_pane(daemon)),
            ("a bare launch opens a usable window", lambda: check_bare_launch(daemon)),
            ("a clean machine gets a daemon and a workspace", check_cold_start),
            (
                "a pane can drive the window it is drawn in",
                check_a_pane_can_drive_its_own_window,
            ),
            (
                "a refused config opens the roster it would have had nowhere to appear in",
                check_a_broken_config_opens_the_roster,
            ),
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

    failures += leaked_daemons()

    if failures:
        print(
            f"\nsmoke: {failures} failure(s).\n"
            "A failed check is a wiring failure - the app started but did not connect "
            "something it must. The full log for each is under "
            f"{ROOT}/<check>.jsonl. A leaked daemon is named above and is a fault in this "
            "script rather than in the app.",
            file=sys.stderr,
        )
        return 1
    print(
        "\nsmoke: the app launches, connects, paints, renders a split tab as splits, shows "
        "what its agents are doing, lists the panes nothing is showing, comes up on a machine "
        "with no daemon by starting one, hands every pane a `muster` that answers for the "
        "window it is drawn in, does all of it assembled into a bundle with launchd's PATH, "
        "and leaves no daemon running afterwards."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
