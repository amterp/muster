"""Translate Antigravity hook and status JSON into the daemon's report arguments."""

import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile


STATES = {
    "working": "working",
    "tool_use": "working",
    "reviewing": "working",
    "idle": "idle",
}


def send(daemon, cwd, arguments, state=None):
    agent = ["--agent", "agy"] if state is None else ["--agent", "agy", "--state", state]
    return subprocess.run(
        [daemon, "report", *agent, *arguments],
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=3,
        check=False,
    ).returncode in (0, 4)


def hook_report(daemon, payload, seen):
    conversation = payload.get("conversationId")
    if not conversation or conversation != os.environ.get("ANTIGRAVITY_CONVERSATION_ID"):
        return

    event = payload.get("hookEventName")
    changed = seen.get("conversation") != conversation
    args = ["--session-id", conversation, "--context-session", conversation] if changed else []
    state = None
    if event == "PreInvocation":
        state = "working"
    elif event == "Stop":
        if changed or payload.get("fullyIdle") is not True or payload.get("continuationActive") is not False:
            return
        state = "idle"
    elif event != "SessionStart":
        return

    if (args or state) and send(daemon, payload["cwd"], args, state):
        seen["conversation"] = conversation
        seen.pop("status", None)


def status_report(daemon, payload, seen):
    conversation = payload.get("conversation_id")
    known = seen.get("conversation")
    if known and conversation != known:
        return
    state = STATES.get(payload.get("agent_state"))
    if payload.get("tool_confirmation_pending") is True:
        state = "blocked"
    if not state:
        seen.pop("status", None)
        return

    args = []
    model = payload.get("model")
    if isinstance(model, dict) and isinstance(model.get("display_name"), str):
        effort = model.get("effort")
        args.extend(("--model", model["display_name"], "--effort", effort if isinstance(effort, str) else ""))

    window = payload.get("context_window")
    if known and isinstance(window, dict):
        used = window.get("used_percentage")
        if type(used) in (float, int) and math.isfinite(used) and 0 <= used <= 100:
            args.extend(("--context-session", known, "--context-used", str(used)))
    title = payload.get("conversation_title")
    if known and isinstance(title, str) and title:
        args.extend(("--session-name", title))

    status = [state, args]
    if seen.get("status") != status and send(daemon, payload["cwd"], args, state):
        seen["status"] = status


def foreground_group(mode):
    tty = os.open("/dev/tty", os.O_RDONLY | os.O_NOCTTY)
    try:
        foreground = os.tcgetpgrp(tty)
        # agy runs each hook in its own process group. Hooks retain their ancestry for the
        # daemon's attribution. Detached status reports can only use the foreground group.
        return foreground if mode == "hook" or os.getpgrp() == foreground else None
    finally:
        os.close(tty)


def report(mode, daemon, socket, pane, payload):
    cwd = payload.get("cwd")
    if not isinstance(cwd, str) or not os.path.isabs(cwd):
        return
    group = foreground_group(mode)
    if group is None:
        return

    directory = Path(tempfile.gettempdir()) / f"muster-agy-{os.getuid()}"
    directory.mkdir(mode=0o700, exist_ok=True)
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        return
    key = hashlib.sha256(f"{socket}\0{pane}".encode()).hexdigest()
    fd = os.open(directory / key, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "r+") as cache:
        lock = fcntl.LOCK_EX if mode == "hook" else fcntl.LOCK_EX | fcntl.LOCK_NB
        fcntl.flock(cache, lock)
        try:
            seen = json.load(cache)
        except ValueError:
            seen = {}
        if not isinstance(seen, dict) or seen.get("group") != group:
            seen = {"group": group}
        previous = seen.copy()

        if mode == "hook":
            hook_report(daemon, payload, seen)
        else:
            status_report(daemon, payload, seen)
        if seen != previous:
            cache.seek(0)
            json.dump(seen, cache)
            cache.truncate()


def main():
    mode = sys.argv[1]
    daemon = os.environ.get("MUSTER_DAEMON")
    socket = os.environ.get("MUSTER_DAEMON_SOCKET")
    pane = os.environ.get("MUSTER_PANE")
    if mode == "hook":
        print("{}")
    if not (daemon and socket and pane):
        return
    try:
        payload = json.load(sys.stdin)
        if isinstance(payload, dict):
            report(mode, daemon, socket, pane, payload)
    except (OSError, ValueError, TypeError, OverflowError, subprocess.TimeoutExpired):
        pass


if __name__ == "__main__":
    main()
