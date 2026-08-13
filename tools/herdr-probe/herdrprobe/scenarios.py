"""The scenarios.

Each one drives a real daemon, writes raw transcripts, and records machine-checkable
facts. Nothing here asserts: the probe observes and saves, and the findings doc reads
the saved evidence. A scenario that discovers herdr disagreeing with our
architecture is doing its job.
"""

from __future__ import annotations

import base64
import json
import re
import time
from pathlib import Path

from .panestream import MODE_SEQUENCES, PaneStream
from .recorder import Recorder, RecordingClient

# Fields that only a live session can have. If session.snapshot carries these it is
# live structure; if only layout.export has the tree, the mirror bootstraps elsewhere.
LIVE_ONLY_PANE_FIELDS = ["agent_status", "focused", "foreground_cwd", "scroll", "revision"]

SUBSCRIPTIONS = [
    {"type": "pane.created"}, {"type": "pane.closed"}, {"type": "pane.updated"},
    {"type": "pane.focused"}, {"type": "pane.exited"}, {"type": "pane.agent_detected"},
    {"type": "tab.created"}, {"type": "tab.focused"}, {"type": "workspace.created"},
    {"type": "workspace.focused"},
]

# Every subscription that takes no parameters, which is every structural change a
# mirror has to apply. Kept separate from SUBSCRIPTIONS above so that widening it does
# not silently rewrite what the older scenarios recorded; those transcripts are cited
# by the findings doc and should change only when someone means to re-record them.
#
# The three parameterized subscriptions - pane.agent_status_changed, pane.output_matched,
# pane.scroll_changed - are deliberately absent: they need a pane id, and they are the
# ones that come back with dotted names (see docs/observations/herdr-0.8.0.md section 6).
STRUCTURE_SUBSCRIPTIONS = [
    {"type": name} for name in (
        "workspace.created", "workspace.updated", "workspace.metadata_updated",
        "workspace.renamed", "workspace.moved", "workspace.reordered",
        "workspace.closed", "workspace.focused",
        "worktree.created", "worktree.opened", "worktree.removed",
        "tab.created", "tab.closed", "tab.focused", "tab.renamed", "tab.moved",
        "pane.created", "pane.closed", "pane.updated", "pane.focused", "pane.moved",
        "pane.exited", "pane.agent_detected",
        "layout.updated",
    )
]


def _new_workspace(client, cwd="/tmp", label=None):
    return client.request("workspace.create", {"cwd": cwd, "focus": True, "label": label})


def _panes(client):
    return client.request("session.snapshot")["snapshot"]["panes"]


def _status(client):
    return {p["pane_id"]: p.get("agent_status") for p in _panes(client)}


# --------------------------------------------------------------------------- 1

def snapshot(daemon, rec: Recorder) -> None:
    """Fact 1: is session.snapshot live structure, or a restore tree?"""
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    client.request("pane.split", {"direction": "right", "target_pane_id": "w1:p1", "cwd": "/tmp"})
    client.request("tab.create", {"workspace_id": "w1"})
    time.sleep(0.5)

    snap = client.request("session.snapshot")["snapshot"]
    export = client.request("layout.export", {})
    rec.write_json("session.snapshot.json", snap)
    rec.write_json("layout.export.json", export)

    pane = snap["panes"][0]
    rec.fact("snapshot_top_level_keys", sorted(snap.keys()))
    rec.fact("snapshot_pane_keys", sorted(pane.keys()))
    rec.fact("snapshot_live_fields_present", {f: (f in pane) for f in LIVE_ONLY_PANE_FIELDS})
    rec.fact("snapshot_counts", {k: len(snap[k]) for k in ("workspaces", "tabs", "panes", "agents", "layouts")})

    export_text = json.dumps(export)
    rec.fact("export_contains_live_fields", {f: (f'"{f}"' in export_text) for f in LIVE_ONLY_PANE_FIELDS})
    rec.fact("export_top_level_keys", sorted(export.keys()) if isinstance(export, dict) else None)
    rec.note(f"snapshot panes={len(snap['panes'])} tabs={len(snap['tabs'])} agents={len(snap['agents'])}")
    rec.note(f"live fields on pane: {[f for f in LIVE_ONLY_PANE_FIELDS if f in pane]}")

    # Does subscribing alone bootstrap the mirror, or is a snapshot required first?
    with daemon.client().subscribe(SUBSCRIPTIONS) as stream:
        time.sleep(1.0)
        replay = stream.snapshot()
    rec.write_json("subscribe-replay.json", replay)
    rec.fact("subscribe_replays_existing_state", len(replay) > 0)
    rec.fact("subscribe_replay_event_kinds", sorted({e.get("event") for e in replay if "event" in e}))
    rec.note(f"events.subscribe replayed {len(replay)} events for an already-populated session")


# --------------------------------------------------------------------------- 2

def frames(daemon, rec: Recorder) -> None:
    """Fact 2: what the pane channel carries, and whether mode changes survive it."""
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    stream = PaneStream(daemon, "w1:p1", "control", cols=80, rows=24)
    try:
        stream.wait_for_frames(1, timeout=5.0)
        stream.wait_quiet()
        attach_frames = stream.snapshot()
        first = attach_frames[0]
        rec.fact("attach_first_frame_full", first.get("full"))
        rec.fact("attach_first_frame_seq", first.get("seq"))
        rec.fact("attach_first_frame_bytes", len(base64.b64decode(first["bytes"])))
        rec.fact("attach_frame_geometry", [first.get("width"), first.get("height")])
        rec.fact("frame_envelope_keys", sorted(first.keys() - {"_t_ms"}))
        rec.note(f"attach: {len(attach_frames)} frame(s), first full={first.get('full')} "
                 f"{first.get('width')}x{first.get('height')} {len(base64.b64decode(first['bytes']))}B")

        before = len(stream.snapshot())
        escaped = "".join(s.decode("latin1") for s in MODE_SEQUENCES.values()).replace("\x1b", "\\033")
        stream.send_input_text(f"printf '%s'\n" % escaped)
        time.sleep(1.5)
        after = stream.snapshot()[before:]
        emitted = b"".join(base64.b64decode(f["bytes"]) for f in after if f.get("bytes"))

        survived = {name: (seq in emitted) for name, seq in MODE_SEQUENCES.items()}
        rec.fact("mode_sequences_survive_frame_stream", survived)
        rec.fact("mode_sequences_any_survived", any(survived.values()))
        rec.note(f"mode sequences surviving into frames: "
                 f"{[k for k, v in survived.items() if v] or 'none'}")

        seqs = [f["seq"] for f in stream.snapshot() if f.get("type") == "terminal.frame"]
        rec.fact("frame_seq_monotonic", seqs == sorted(seqs))
        rec.fact("frame_seq_range", [seqs[0], seqs[-1]] if seqs else None)
        rec.fact("frame_count", len(seqs))
        rec.fact("full_frame_count", sum(1 for f in stream.snapshot() if f.get("full")))

        all_frames = stream.snapshot()
        rec.write_text("frames.ndjson", "".join(json.dumps(f, sort_keys=True) + "\n" for f in all_frames))
        rec.write_bytes("frame-001-attach.ansi", base64.b64decode(all_frames[0]["bytes"]))
        if after:
            rec.write_bytes("frames-after-mode-set.ansi", emitted)
        rec.write_text("stderr.txt", "\n".join(stream.stderr) + "\n")

        # A second viewer: does the daemon fan the same screen out to both?
        observer = PaneStream(daemon, "w1:p1", "observe", cols=80, rows=24)
        try:
            observer.wait_for_frames(1, timeout=5.0)
            observer.wait_quiet()
            obs = observer.snapshot()
            rec.fact("observer_gets_full_frame_on_attach", bool(obs) and obs[0].get("full"))
            rec.fact("observer_frame_geometry", [obs[0].get("width"), obs[0].get("height")] if obs else None)
            rec.write_text("observer-frames.ndjson",
                           "".join(json.dumps(f, sort_keys=True) + "\n" for f in obs))
            rec.note(f"observer attach: {len(obs)} frame(s), first full={obs[0].get('full') if obs else None}")
        finally:
            observer.close()
    finally:
        stream.close()


# --------------------------------------------------------------------------- 3

def agent_states(daemon, rec: Recorder) -> None:
    """Fact 3: the five states, and what actually gates idle vs done."""
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    reportable, rejected = [], []
    for state in ("idle", "working", "blocked", "done", "unknown"):
        ok, _ = client.try_request(
            "pane.report_agent",
            {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": state},
        )
        (reportable if ok else rejected).append(state)
    rec.fact("states_a_client_may_report", reportable)
    rec.fact("states_rejected_when_reported", rejected)
    rec.note(f"reportable states: {reportable}; rejected: {rejected}")

    # A pane in the ACTIVE tab: completion should read idle, because an active tab
    # with no client counts as seen.
    client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "working"})
    time.sleep(0.4)
    working_status = _status(client)["w1:p1"]
    client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "idle"})
    time.sleep(0.6)
    active_tab_completion = _status(client)["w1:p1"]
    rec.fact("status_while_working", working_status)
    rec.fact("completion_in_active_tab", active_tab_completion)
    rec.note(f"completion while pane is in the ACTIVE tab -> {active_tab_completion}")

    # Same transition with the pane's tab in the background: this is where done lives.
    client.request("tab.create", {"workspace_id": "w1", "focus": True})
    time.sleep(0.4)
    with daemon.client().subscribe(
        SUBSCRIPTIONS + [{"type": "pane.agent_status_changed", "pane_id": "w1:p1"}]
    ) as stream:
        client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "working"})
        time.sleep(0.4)
        client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "idle"})
        time.sleep(0.8)
        background_completion = _status(client)["w1:p1"]

        client.request("tab.focus", {"tab_id": "w1:t1"})
        time.sleep(0.8)
        after_focus = _status(client)["w1:p1"]
        events = stream.snapshot()

    rec.fact("completion_in_background_tab", background_completion)
    rec.fact("status_after_focusing_its_tab", after_focus)
    rec.fact("done_is_derived_not_reported", "done" in rejected and background_completion == "done")
    rec.note(f"completion while pane's tab is in the BACKGROUND -> {background_completion}")
    rec.note(f"after focusing that tab -> {after_focus}")

    rec.write_text("events.ndjson", "".join(json.dumps(e, sort_keys=True) + "\n" for e in events))

    # Event names are not spelled consistently: most arrive snake_cased
    # (pane_created, tab_focused), but the three subscriptions that take parameters
    # keep their dotted subscription name. An adapter has to accept both.
    kinds = sorted({e["event"] for e in events if "event" in e})
    rec.fact("event_kinds_delivered", kinds)
    rec.fact("event_kinds_dotted", [k for k in kinds if "." in k])
    rec.fact("event_kinds_snake_cased", [k for k in kinds if "." not in k])

    status_events = [e for e in events if e.get("event") in
                     ("pane.agent_status_changed", "pane_agent_status_changed")]
    rec.fact("agent_status_change_events_seen", len(status_events))
    rec.fact("agent_status_event_sequence",
             [e["data"].get("agent_status") for e in status_events if isinstance(e.get("data"), dict)])
    rec.fact("agent_detected_event_seen",
             any(e.get("event") in ("pane_agent_detected", "pane.agent_detected") for e in events))
    rec.note(f"status-change events pushed: "
             f"{[e['data'].get('agent_status') for e in status_events if isinstance(e.get('data'), dict)]}")

    # Does pane.focus alone move seen-ness, or must the pane's TAB become active?
    client.request("tab.create", {"workspace_id": "w1", "focus": True})
    time.sleep(0.3)
    client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "working"})
    time.sleep(0.3)
    client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "idle"})
    time.sleep(0.6)
    before_pane_focus = _status(client)["w1:p1"]
    ok, _ = client.try_request("pane.focus", {"pane_id": "w1:p1"})
    time.sleep(0.6)
    rec.fact("pane_focus_supported", ok)
    rec.fact("status_before_pane_focus", before_pane_focus)
    rec.fact("status_after_pane_focus", _status(client)["w1:p1"])
    rec.note(f"pane.focus on a done pane: {before_pane_focus} -> {_status(client)['w1:p1']}")


# --------------------------------------------------------------------------- 4

def _pty_size(client, pane_id="w1:p1"):
    """Ask the pane's own shell what size its PTY is - the only unarguable answer.

    pane.layout reports herdr's notional TUI rect, which is a different number from
    the size the inner program actually sees.
    """
    client.request("pane.send_text", {"pane_id": pane_id, "text": "stty size\n"})
    time.sleep(0.8)
    text = client.request("pane.read", {"pane_id": pane_id, "source": "visible", "strip_ansi": True})["read"]["text"]
    matches = re.findall(r"^\s*(\d+)\s+(\d+)\s*$", text, re.M)
    return [int(matches[-1][1]), int(matches[-1][0])] if matches else None  # [cols, rows]


def geometry(daemon, rec: Recorder) -> None:
    """Fact 4: does a controlling client hold the pane's geometry?"""
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    def dims():
        pane = [p for p in _panes(client) if p["pane_id"] == "w1:p1"][0]
        layout = client.request("pane.layout", {"pane_id": "w1:p1"})
        return pane, layout

    pane0, layout0 = dims()
    rec.write_json("pane-before-attach.json", {"pane": pane0, "layout": layout0})
    rec.fact("pane_keys_carrying_size", [k for k in pane0 if any(t in k for t in ("col", "row", "width", "height", "size"))])
    rec.fact("layout_rect_no_viewer", layout0["layout"]["panes"][0]["rect"])
    rec.fact("pty_size_no_viewer", _pty_size(client))
    rec.note(f"with no viewer attached the pane's PTY is {rec.recall('pty_size_no_viewer')}")

    control = PaneStream(daemon, "w1:p1", "control", cols=100, rows=30)
    try:
        control.wait_for_frames(1, timeout=5.0)
        control.wait_quiet()
        pane1, layout1 = dims()
        rec.write_json("pane-during-control-100x30.json", {"pane": pane1, "layout": layout1})
        frame1 = control.snapshot()[0]
        rec.fact("controller_requested", [100, 30])
        rec.fact("controller_frame_geometry", [frame1.get("width"), frame1.get("height")])
        rec.fact("pty_size_under_controller", _pty_size(client))
        rec.fact("layout_rect_under_controller", layout1["layout"]["panes"][0]["rect"])
        rec.note(f"controller at 100x30 -> PTY {rec.recall('pty_size_under_controller')}, "
                 f"pane.layout rect unchanged at {layout1['layout']['panes'][0]['rect']}")

        observer = PaneStream(daemon, "w1:p1", "observe", cols=80, rows=24)
        try:
            observer.wait_for_frames(1, timeout=5.0)
            observer.wait_quiet()
            obs_frame = observer.snapshot()[0]
            pane2, layout2 = dims()
            rec.write_json("pane-during-control-and-observe.json", {"pane": pane2, "layout": layout2})
            rec.fact("observer_requested", [80, 24])
            rec.fact("observer_frame_geometry", [obs_frame.get("width"), obs_frame.get("height")])
            rec.fact("observer_forced_to_controller_size",
                     [obs_frame.get("width"), obs_frame.get("height")] == [frame1.get("width"), frame1.get("height")])
            rec.fact("pty_size_with_observer_attached", _pty_size(client))
            rec.note(f"controller asked 100x30 and got {frame1.get('width')}x{frame1.get('height')}; "
                     f"observer asked 80x24 and got {obs_frame.get('width')}x{obs_frame.get('height')}; "
                     f"PTY stayed {rec.recall('pty_size_with_observer_attached')}")
        finally:
            observer.close()

        # Resize through the control stream: does pane truth follow the controller?
        control.resize(120, 40)
        time.sleep(1.0)
        pane3, layout3 = dims()
        resized = [f for f in control.snapshot() if f.get("width") == 120]
        rec.write_json("pane-after-resize-120x40.json", {"pane": pane3, "layout": layout3})
        rec.fact("resize_intent_took_effect", bool(resized))
        rec.fact("geometry_after_resize", [resized[-1].get("width"), resized[-1].get("height")] if resized else None)
        rec.fact("pty_size_after_resize", _pty_size(client))
        rec.note(f"terminal.resize 120x40 -> frames at "
                 f"{resized[-1].get('width') if resized else None}x{resized[-1].get('height') if resized else None}, "
                 f"PTY {rec.recall('pty_size_after_resize')}")
    finally:
        control.close()

    # The one that matters for "sessions survive anything Muster does": when the
    # controller goes away, does the pane go back to the size everyone else uses?
    # Sampled twice, because "it had not reverted yet" and "it never reverts" are
    # different claims and only the second one is worth acting on.
    time.sleep(1.5)
    pane4, layout4 = dims()
    rec.write_json("pane-after-detach.json", {"pane": pane4, "layout": layout4})
    settled = rec.recall("pty_size_no_viewer")
    soon = _pty_size(client)
    time.sleep(8.0)
    later = _pty_size(client)
    rec.fact("pty_size_1s_after_detach", soon)
    rec.fact("pty_size_10s_after_detach", later)
    rec.fact("geometry_hold_released_on_detach", later == settled)
    rec.note(f"after the controller detached the PTY is {soon}, still {later} ten seconds later "
             f"(it was {settled} before any viewer attached)")


# --------------------------------------------------------------------------- 5

KEY_VOCABULARY = [
    "a", "A", "enter", "return", "tab", "escape", "esc", "space", "backspace",
    "up", "down", "left", "right", "home", "end", "pageup", "pagedown",
    "f1", "f12", "ctrl+c", "ctrl+a", "alt+b", "shift+enter", "ctrl+shift+k",
    "ctrl+alt+delete", "cmd+k", "super+k", "insert", "delete", "€", "é",
]


def input_path(daemon, rec: Recorder) -> None:
    """The input half: what a client can express, and where encoding can live."""
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    accepted, refused = [], {}
    for key in KEY_VOCABULARY:
        ok, err = client.try_request("pane.send_keys", {"pane_id": "w1:p1", "keys": [key]})
        if ok:
            accepted.append(key)
        else:
            refused[key] = err[:160]
    rec.fact("send_keys_accepted", accepted)
    rec.fact("send_keys_refused", refused)
    rec.note(f"pane.send_keys accepted {len(accepted)}/{len(KEY_VOCABULARY)} probed key names")

    # Is any pane terminal mode readable through the API? If not, an adapter cannot
    # encode input itself and the encoding must stay daemon-side.
    pane = [p for p in _panes(client) if p["pane_id"] == "w1:p1"][0]
    mode_words = ("mode", "kitty", "bracket", "paste", "mouse", "application", "cursor_key")
    rec.fact("pane_fields_mentioning_modes", [k for k in pane if any(w in k.lower() for w in mode_words)])
    ok, info = client.try_request("pane.process_info", {"pane_id": "w1:p1"})
    rec.write_json("pane-process-info.json", info if ok else {"error": info})
    rec.fact("process_info_mentions_modes",
             ok and any(w in json.dumps(info).lower() for w in mode_words))

    # Raw bytes on the control stream: this is the channel that could carry fully
    # encoded input, and the one herdr's own TUI uses.
    stream = PaneStream(daemon, "w1:p1", "control", cols=80, rows=24)
    try:
        stream.wait_for_frames(1, timeout=5.0)
        stream.send_input_text("stty raw -echo; cat -v\n")
        time.sleep(1.0)
        probes = {
            "plain_a": b"a",
            "ctrl_c": b"\x03",
            "esc": b"\x1b",
            "alt_b": b"\x1bb",
            "utf8_e_acute": "é".encode(),
            "kitty_csi_u_a": b"\x1b[97u",
            "bracketed_paste": b"\x1b[200~pasted\x1b[201~",
        }
        received = {}
        for name, payload in probes.items():
            stream.send_input_bytes(payload)
            time.sleep(0.5)
            read = client.request("pane.read", {"pane_id": "w1:p1", "source": "visible", "strip_ansi": True})
            received[name] = read["read"]["text"][-200:]
        rec.write_json("raw-input-echo.json", received)
        rec.fact("raw_bytes_accepted_on_control_stream", True)
        rec.note("sent raw byte probes through terminal.input; echo captured in raw-input-echo.json")

        # The focus-reporting sequences herdr parses from client input (CSI I / CSI O)
        # are how seen-ness is fed. Does driving them through this stream move it?
        client.request("tab.create", {"workspace_id": "w1", "focus": True})
        time.sleep(0.3)
        client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "working"})
        time.sleep(0.3)
        client.request("pane.report_agent", {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "idle"})
        time.sleep(0.6)
        before = _status(client)["w1:p1"]
        stream.send_input_bytes(b"\x1b[I")  # OuterFocusGained
        time.sleep(0.8)
        after_gained = _status(client)["w1:p1"]
        rec.fact("status_before_focus_gained_sequence", before)
        rec.fact("status_after_focus_gained_sequence", after_gained)
        rec.fact("csi_i_moves_seenness", before != after_gained)
        rec.note(f"CSI I (focus gained) on the control stream: {before} -> {after_gained}")
    finally:
        stream.close()

    # Whatever the API exposes for a client's own window focus, by name.
    methods = ["client.window_title.set", "client.focus", "client.outer_focus", "pane.mark_seen"]
    availability = {}
    for method in methods:
        ok, err = client.try_request(method, {})
        availability[method] = "accepted" if ok else ("unknown_method" if "unknown" in str(err).lower() else "exists")
    rec.fact("focus_related_method_probe", availability)


# --------------------------------------------------------------------------- 5b

# Each probe is a sequence a client would only send if it believed the pane's program
# had the matching mode on. The pane runs a plain shell, which has none of them. So a
# daemon that re-encodes for the pane's real modes must rewrite every one of these,
# and a daemon that passes bytes through must deliver them untouched.
ENCODING_PROBES: list[tuple[str, bytes, str, str]] = [
    ("plain_a", b"a", "a", "a"),
    ("kitty_csi_u_a", b"\x1b[97u", "^[[97u", "a"),
    ("bracketed_paste", b"\x1b[200~x\x1b[201~", "^[[200~x^[[201~", "x"),
    ("ss3_cursor_up", b"\x1bOA", "^[OA", "^[[A"),
    ("csi_cursor_up", b"\x1b[A", "^[[A", "^[[A"),
]


def input_encoding(daemon, rec: Recorder) -> None:
    """Does the control stream re-encode input for the pane's modes, or pass bytes through?

    The whole input architecture turns on this. If herdr re-encodes, a client reports
    whatever its host terminal gave it and stays out of the modes business. If it does
    not, the client must encode for modes herdr exposes nowhere.

    The earlier input-path scenario tried to answer this and could not: its `stty raw
    -echo; cat -v` line was corrupted by leftover bytes from the send_keys vocabulary
    probe, so every echo it recorded is a cooked-mode shell echo rather than the bytes
    the program received. This scenario keeps the pane clean and reads `cat -v`.
    """
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    stream = PaneStream(daemon, "w1:p1", "control", cols=80, rows=24)
    try:
        stream.wait_for_frames(1, timeout=5.0)

        # cat -v renders every byte it receives as printable text: ESC as ^[, and
        # nothing else touched. Raw mode with echo off keeps the line discipline from
        # adding a second, differently-mangled copy of the same bytes.
        stream.send_input_text("stty raw -echo; cat -v\n")
        time.sleep(1.5)
        screen_before = _read_visible(client, "w1:p1")
        rec.write_text("screen-before-probes.txt", screen_before)
        # A shell that never ran cat -v would echo the probes itself, which looks
        # similar enough to mislead a reader of the transcript.
        started = "command not found" not in screen_before
        rec.fact("cat_v_started", started)
        if not started:
            rec.note("cat -v did not start; the transcript below is a shell echo and proves nothing")

        results = {}
        for name, payload, if_raw, if_reencoded in ENCODING_PROBES:
            stream.send_input_bytes(payload)
            # LF alone, so cat -v ends the line without printing a ^M of its own.
            stream.send_input_bytes(b"\n")
            time.sleep(0.6)
            line = _read_visible(client, "w1:p1").splitlines()[-1].strip()
            results[name] = {
                "sent": payload.decode("latin-1"),
                "received": line,
                "raw_passthrough_would_print": if_raw,
                "reencode_would_print": if_reencoded,
                "verdict": ("passthrough" if line == if_raw
                            else "reencoded" if line == if_reencoded
                            else "neither"),
            }
            rec.note(f"{name}: cat -v printed {line!r} ({results[name]['verdict']})")

        rec.write_json("cat-v-probes.json", results)
        rec.write_text("screen-after-probes.txt", _read_visible(client, "w1:p1"))
        verdicts = {name: r["verdict"] for name, r in results.items()}
        rec.fact("probe_verdicts", verdicts)
        # plain_a is the control: it reads the same either way, so it says the channel
        # works without saying anything about encoding.
        decisive = {n: v for n, v in verdicts.items() if n != "plain_a"}
        rec.fact("control_stream_reencodes_input", all(v == "reencoded" for v in decisive.values()))
        rec.fact("control_stream_passes_bytes_through", all(v == "passthrough" for v in decisive.values()))
    finally:
        stream.close()


def _read_visible(client, pane_id: str) -> str:
    read = client.request("pane.read", {"pane_id": pane_id, "source": "visible", "strip_ansi": True})
    return read["read"]["text"]


# --------------------------------------------------------------------------- 5c

# Text chosen for the ways a grid reader goes wrong rather than for looking realistic:
# a wide character occupies two cells and leaves a spacer, a combining mark belongs to
# the cell before it, and an emoji cluster is several codepoints that must land in one
# cell rather than several.
FIDELITY_PAYLOAD = [
    "ascii: hello world",
    "wide: NIHAO -> 你好世界",
    "combining: é and é should look alike",
    "box: ┌─┬─┐",
    "emoji: \U0001f44d and \U0001f468‍\U0001f469‍\U0001f467",
]


def frame_fidelity(daemon, rec: Recorder) -> None:
    """Do herdr's frames, replayed into libghostty-vt, reproduce herdr's own screen?

    The grid harness reads a screen out of the frame stream and calls it what the user
    sees. That is only true if replaying the frames lands where the daemon's own
    terminal did, and the daemon can be asked directly: pane.read returns its rendering
    of the same screen at the same moment. Recording both gives the test two independent
    oracles instead of a snapshot that only proves the pipeline ran.
    """
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    stream = PaneStream(daemon, "w1:p1", "control", cols=80, rows=24)
    try:
        stream.wait_for_frames(1, timeout=5.0)
        stream.wait_quiet(quiet_for=0.5, timeout=3.0)

        # clear first, so the prompt and the command line do not sit above the payload
        # differently on a re-record.
        stream.send_input_text("clear\n")
        time.sleep(0.8)
        for line in FIDELITY_PAYLOAD:
            # ensure_ascii=False, or json quotes the payload into \uXXXX escapes and the
            # shell prints the escape text - a screen with no wide characters on it,
            # which is the one thing this scenario exists to capture.
            stream.send_input_text(f"printf '%s\\n' {json.dumps(line, ensure_ascii=False)}\n")
            time.sleep(0.4)
        stream.wait_quiet(quiet_for=0.8, timeout=5.0)

        frames = stream.snapshot()
        with (rec.dir / "frames.ndjson").open("w") as f:
            for frame in frames:
                f.write(json.dumps({k: v for k, v in frame.items() if k != "_t_ms"}) + "\n")

        # herdr's own rendering of the screen those frames describe.
        rec.write_text("herdr-screen.txt", _read_visible(client, "w1:p1"))

        rec.fact("frame_count", len(frames))
        rec.fact("full_frame_count", sum(1 for f in frames if f.get("full")))
        rec.fact("payload_lines", FIDELITY_PAYLOAD)
        rec.note(f"painted {len(FIDELITY_PAYLOAD)} lines, captured {len(frames)} frame(s) "
                 f"and herdr's own screen text")
    finally:
        stream.close()


def detection(daemon, rec: Recorder) -> None:
    """Does screen-based agent detection need a client viewing the pane?

    It decides how much the data plane may detach. If detection only runs for panes
    someone is watching, then hidden panes freeze their agent states, and agent states
    are the whole reason Muster exists.
    """
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.4)

    client.request("pane.send_text", {"pane_id": "w1:p1", "text": f"exec {daemon.screen_agent}\n"})
    time.sleep(1.5)
    agents = client.request("session.snapshot")["snapshot"]["agents"]
    rec.fact("agent_detected_from_process_name", [a.get("agent") for a in agents])
    rec.fact("override_manifest_loaded",
             "local override" in daemon.cli("server", "agent-manifests", check=False).stdout)

    def settle(command: str, want: str, timeout: float = 30.0):
        """Drive the screen and time how long the daemon takes to agree."""
        client.request("pane.send_text", {"pane_id": "w1:p1", "text": command + "\n"})
        start = time.monotonic()
        while time.monotonic() - start < timeout:
            live = client.request("session.snapshot")["snapshot"]["agents"]
            if live and live[0].get("agent_status") == want:
                return round(time.monotonic() - start, 2)
            time.sleep(0.25)
        return None

    cycle = [("working", "working"), ("idle", "idle"), ("blocked", "blocked"), ("idle", "idle")]
    unviewed = [[cmd, settle(cmd, want)] for cmd, want in cycle]
    rec.fact("detection_latency_no_viewer_s", unviewed)
    rec.fact("detection_works_with_no_viewer", all(v is not None for _, v in unviewed))
    rec.note(f"with no viewer attached, detection settled in {[v for _, v in unviewed]}s")

    stream = PaneStream(daemon, "w1:p1", "control", cols=80, rows=24)
    try:
        stream.wait_for_frames(1, timeout=5.0)
        time.sleep(1.0)
        viewed = [[cmd, settle(cmd, want)] for cmd, want in cycle]
    finally:
        stream.close()
    rec.fact("detection_latency_with_viewer_s", viewed)
    rec.fact("detection_works_with_viewer", all(v is not None for _, v in viewed))
    rec.note(f"with a control stream attached, detection settled in {[v for _, v in viewed]}s")


# --------------------------------------------------------------------------- 9

def _wait_for_kind(stream, *kinds, timeout=6.0):
    return stream.wait_for(lambda e: e.get("event") in kinds, timeout=timeout)


def _tree_depth(node) -> int:
    if not isinstance(node, dict) or node.get("type") != "split":
        return 0
    return 1 + max(_tree_depth(node.get("first")), _tree_depth(node.get("second")))


def lifecycle(daemon, rec: Recorder) -> None:
    """What a mirror has to survive: things being destroyed, and nesting.

    Every earlier scenario recorded a session that only ever grew, so the corpus has
    no removal in it at all - and removal is the half of convergent application that
    is easy to get wrong. This also records the first layout deeper than one split,
    because rendering native splits cannot be designed against a single-level tree.

    Also settles what the per-entity counters actually track, which decides whether a
    client can detect a gap in the event stream or only survive one.
    """
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.5)

    with daemon.client().subscribe(STRUCTURE_SUBSCRIPTIONS) as stream:
        # The replay arrives on the drain thread, so counting it needs a beat. Reading
        # immediately reports zero and looks like herdr not replaying at all.
        time.sleep(1.0)
        replayed = stream.snapshot()

        # Two levels, mixed directions: root = split(right, p1, split(down, p2, p3)).
        client.request("pane.split", {"direction": "right", "target_pane_id": "w1:p1", "cwd": "/tmp"})
        time.sleep(0.4)
        client.request("pane.split", {"direction": "down", "target_pane_id": "w1:p2", "cwd": "/tmp"})
        time.sleep(0.6)

        nested_snapshot = client.request("session.snapshot")["snapshot"]
        nested_export = client.request("layout.export", {})
        rec.write_json("nested-session.snapshot.json", nested_snapshot)
        rec.write_json("nested-layout.export.json", nested_export)

        root = nested_export.get("layout", {}).get("root")
        tab_layout = next((l for l in nested_snapshot["layouts"] if l["tab_id"] == "w1:t1"), {})
        rec.fact("nested_tree_depth", _tree_depth(root))
        rec.fact("nested_tree_directions", sorted(_directions(root)))
        rec.fact("nested_snapshot_split_count", len(tab_layout.get("splits", [])))
        rec.fact("nested_snapshot_split_keys",
                 sorted(tab_layout["splits"][0].keys()) if tab_layout.get("splits") else [])
        rec.fact("nested_snapshot_pane_rects",
                 {p["pane_id"]: p["rect"] for p in tab_layout.get("panes", [])})
        rec.note(f"three panes, two levels: export tree depth {_tree_depth(root)}, "
                 f"snapshot lists {len(tab_layout.get('splits', []))} split border(s)")

        # Now take things away, which is the part nothing has ever recorded.
        client.request("tab.create", {"workspace_id": "w1", "focus": False})
        time.sleep(0.4)
        client.request("pane.close", {"pane_id": "w1:p3"})
        closed = _wait_for_kind(stream, "pane_closed", "pane.closed")
        time.sleep(0.3)
        client.request("tab.close", {"tab_id": "w1:t2"})
        _wait_for_kind(stream, "tab_closed", "tab.closed")
        time.sleep(0.3)

        client.request("workspace.create", {"cwd": "/tmp", "focus": False, "label": "doomed"})
        time.sleep(0.4)
        client.request("workspace.close", {"workspace_id": "w2"})
        _wait_for_kind(stream, "workspace_closed", "workspace.closed")
        time.sleep(0.3)

        # Does an agent state change reach a client that did not name the pane?
        #
        # pane.agent_status_changed is one of the three subscriptions that require a
        # pane_id, so watching every pane's agent means one subscription per pane - unless
        # the unparameterized pane.updated also fires. Which of those is true decides
        # whether the sidebar costs one connection or N.
        rounds = []
        for state in ("working", "idle", "blocked"):
            before_agent = len(stream.snapshot())
            client.request("pane.report_agent",
                           {"pane_id": "w1:p1", "source": "probe", "agent": "probe",
                            "state": state})
            time.sleep(1.2)
            rounds.append([state, [e.get("event") for e in stream.snapshot()[before_agent:]]])
        # The first round also announces the agent itself, which is a one-off. What
        # matters is the rounds after it, when the agent is already known and only its
        # state is moving.
        settled = [events for _, events in rounds[1:]]
        rec.fact("agent_state_change_events_without_naming_the_pane", rounds)
        rec.fact("agent_state_visible_to_unparameterized_subscriber",
                 any(events for events in settled))
        rec.note(f"agent state changes, watched without naming the pane: {rounds}")

        # Compare against a subscriber that does name it, on the same daemon.
        with daemon.client().subscribe(
            [{"type": "pane.agent_status_changed", "pane_id": "w1:p1"}]
        ) as named:
            time.sleep(0.4)
            base = len(named.snapshot())
            client.request("pane.report_agent",
                           {"pane_id": "w1:p1", "source": "probe", "agent": "probe",
                            "state": "idle"})
            time.sleep(1.2)
            named_events = [e.get("event") for e in named.snapshot()[base:]]
        rec.fact("agent_state_change_events_when_naming_the_pane", named_events)
        rec.note(f"the same change, watched by a subscriber that named the pane: "
                 f"{named_events or 'NOTHING ARRIVED'}")

        # A pane whose program ends on its own, rather than one a client closed.
        client.request("pane.send_text", {"pane_id": "w1:p2", "text": "exit\n"})
        exited = _wait_for_kind(stream, "pane_exited", "pane.exited", timeout=8.0)
        time.sleep(0.6)

        events = stream.snapshot()

    rec.write_text("events.ndjson", "".join(json.dumps(e, sort_keys=True) + "\n" for e in events))
    kinds = sorted({e["event"] for e in events if "event" in e})
    rec.fact("subscribe_replayed_event_count", len(replayed))
    rec.fact("subscribe_replayed_kinds", sorted({e["event"] for e in replayed if "event" in e}))
    rec.fact("lifecycle_event_kinds_delivered", kinds)
    rec.fact("removal_kinds_seen", [k for k in kinds if "clos" in k or "exit" in k])
    # The shape decides how a mirror applies a removal: an id is enough to drop an
    # entry, a whole entity is not, and guessing wrong is a pane that never disappears.
    rec.fact("pane_closed_payload_keys", sorted(closed["data"].keys()) if closed else None)
    rec.fact("pane_exited_payload_keys", sorted(exited["data"].keys()) if exited else None)
    rec.fact("pane_exited_payload", exited["data"] if exited else None)

    # The trap: a pane whose program ended and a pane a client closed are announced by
    # different events. A mirror that only handles pane_closed keeps the exited pane
    # forever, which is a dead surface the user cannot get rid of.
    order = [e["event"] for e in events if "event" in e]
    exited_at = order.index("pane_exited") if "pane_exited" in order else None
    rec.fact("pane_exit_also_emits_pane_closed",
             "pane_closed" in order[exited_at + 1:] if exited_at is not None else None)

    # The sharpest trap of the three. Closing a tab or a workspace announces only itself:
    # the panes underneath it are simply gone, with no pane event of any kind. A mirror
    # that removes only what it is told about leaks them, and they are panes the user
    # cannot see any way to close.
    created = [e["data"]["pane"]["pane_id"] for e in events
               if e.get("event") == "pane_created" and isinstance(e["data"].get("pane"), dict)]
    announced = [e["data"]["pane_id"] for e in events
                 if e.get("event") in ("pane_closed", "pane_exited")]
    surviving = [p["pane_id"] for p in _panes(client)]
    rec.fact("panes_created", created)
    rec.fact("panes_whose_removal_was_announced", announced)
    rec.fact("panes_still_alive", surviving)
    rec.fact("panes_removed_with_no_pane_event",
             sorted(set(created) - set(announced) - set(surviving)))
    rec.note(f"created {len(created)} pane(s), {len(announced)} announced their removal, "
             f"{len(set(created) - set(announced) - set(surviving))} vanished with a parent")

    layouts = [e for e in events if e.get("event") == "layout_updated"]
    rec.fact("layout_updated_count", len(layouts))
    rec.fact("layout_updated_payload_keys",
             sorted(layouts[0]["data"]["layout"].keys()) if layouts else None)
    rec.fact("layout_updated_is_whole_tab_absolute",
             bool(layouts) and {"panes", "splits", "tab_id", "focused_pane_id"}
             <= set(layouts[0]["data"]["layout"].keys()))
    # Which structural changes did NOT get one. A mirror that renders geometry only on
    # layout_updated goes stale exactly here.
    followed = {order[i]: order[i + 1] == "layout_updated"
                for i in range(len(order) - 1)
                if order[i] in ("pane_created", "pane_closed", "pane_exited",
                                "tab_created", "tab_closed", "workspace_closed")}
    rec.fact("structural_change_followed_by_layout_updated", followed)

    rec.note(f"kinds delivered: {kinds}")
    rec.note(f"removals: {[k for k in kinds if 'clos' in k or 'exit' in k] or 'NONE'}")
    rec.note(f"subscribe replayed {len(replayed)} event(s) for a one-pane session")
    rec.note(f"pane_exited followed by a pane_closed: "
             f"{order[exited_at + 1:] if exited_at is not None else 'n/a'}")

    _counters(client, rec)


def _directions(node, found=None):
    found = found if found is not None else set()
    if isinstance(node, dict) and node.get("type") == "split":
        found.add(node.get("direction"))
        _directions(node.get("first"), found)
        _directions(node.get("second"), found)
    return found


def _counters(client, rec: Recorder) -> None:
    """Can a client detect that it missed events, or only that it is out of date?

    architecture.md left this open, and it decides whether reconciliation can be
    triggered by evidence or has to be a timer.
    """
    def pane(pane_id="w1:p1"):
        return next(p for p in _panes(client) if p["pane_id"] == pane_id)

    before = pane()["revision"]
    client.request("pane.report_agent",
                   {"pane_id": "w1:p1", "source": "probe", "agent": "probe", "state": "working"})
    time.sleep(0.5)
    after_state = pane()["revision"]

    client.request("pane.send_text",
                   {"pane_id": "w1:p1", "text": "printf '\\033]0;muster-probe\\007'\n"})
    time.sleep(1.0)
    after_title = pane()["revision"]

    rec.fact("pane_revision_start", before)
    rec.fact("pane_revision_bumped_by_agent_state", after_state != before)
    rec.fact("pane_revision_bumped_by_title", after_title != after_state)
    rec.note(f"pane revision: {before} -> {after_state} after an agent state change "
             f"-> {after_title} after a title change")

    # Is the agent counter per-pane or session-global? A global one can be used to
    # notice a transition on a pane this client has never seen.
    client.request("pane.split", {"direction": "right", "target_pane_id": "w1:p1", "cwd": "/tmp"})
    time.sleep(0.5)
    fresh = [p["pane_id"] for p in _panes(client)]
    second = next((p for p in fresh if p != "w1:p1"), None)
    seqs = []
    for pane_id in ("w1:p1", second, "w1:p1"):
        if not pane_id:
            continue
        for state in ("working", "idle"):
            client.request("pane.report_agent",
                           {"pane_id": pane_id, "source": "probe", "agent": "probe", "state": state})
            time.sleep(0.35)
        agents = client.request("session.snapshot")["snapshot"]["agents"]
        seqs.append({a.get("pane_id"): a.get("state_change_seq") for a in agents})

    rec.fact("agent_state_change_seq_progression", seqs)
    final = seqs[-1] if seqs else {}
    values = [v for v in final.values() if v is not None]
    rec.fact("agent_state_change_seq_unique_across_panes", len(set(values)) == len(values))
    rec.fact("agent_state_change_seq_final", final)
    rec.note(f"agent state_change_seq across panes: {final}")


# --------------------------------------------------------------------------- 10

def durability(daemon, rec: Recorder) -> None:
    """What survives a daemon restart, and whether structure can be put back.

    architecture.md promises sessions outlive quitting the app, dropping the VPN and
    closing the lid - all cases where the daemon itself lives. Nothing has ever watched
    what happens when the daemon does not, which is the ordinary consequence of a reboot
    or a crash. This measures the floor rather than assuming it.

    Also checks the restore half: layout.export claims to be a restore tree, and
    layout.apply takes one. Whether the pair actually round-trips decides how much of a
    'reopen my workspace' feature is a call rather than a project.
    """
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    client.request("pane.split", {"direction": "right", "target_pane_id": "w1:p1", "cwd": "/tmp"})
    client.request("tab.create", {"workspace_id": "w1"})
    time.sleep(0.8)

    # Something identifiable in the scrollback, to see whether it comes back.
    marker = "MUSTER-DURABILITY-MARKER"
    client.request("pane.send_text", {"pane_id": "w1:p1", "text": f"echo {marker}\n"})
    time.sleep(1.0)

    before_snapshot = client.request("session.snapshot")["snapshot"]
    before_export = client.request("layout.export", {})
    rec.write_json("before-restart.snapshot.json", before_snapshot)
    rec.write_json("before-restart.layout.json", before_export)
    before_panes = sorted(p["pane_id"] for p in before_snapshot["panes"])
    before_screen = _read_visible(client, "w1:p1")
    rec.fact("panes_before_restart", before_panes)
    rec.fact("marker_on_screen_before_restart", marker in before_screen)
    rec.note(f"before restart: {len(before_panes)} pane(s), marker on screen: {marker in before_screen}")

    # The event under test. `herdr server stop` is the graceful path - a crash or a power
    # cut is strictly worse, so anything lost here is lost there too.
    daemon.stop()
    time.sleep(1.0)
    daemon.start()
    time.sleep(1.5)
    client = RecordingClient(daemon.client(), rec)

    after_snapshot = client.request("session.snapshot")["snapshot"]
    rec.write_json("after-restart.snapshot.json", after_snapshot)
    after_panes = sorted(p["pane_id"] for p in after_snapshot["panes"])
    rec.fact("panes_after_restart", after_panes)
    rec.fact("session_survives_daemon_restart", after_panes == before_panes)
    rec.fact("workspaces_after_restart", [w["workspace_id"] for w in after_snapshot["workspaces"]])

    # The distinction that decides what "survives" is worth: a pane can come back with
    # its id and its cwd while its terminal - and therefore the process in it - is new.
    terminals = lambda snap: {p["pane_id"]: p["terminal_id"] for p in snap["panes"]}
    before_terms, after_terms = terminals(before_snapshot), terminals(after_snapshot)
    rec.fact("terminal_ids_before_restart", before_terms)
    rec.fact("terminal_ids_after_restart", after_terms)
    rec.fact("terminals_survive_daemon_restart", before_terms == after_terms)
    rec.fact("cwds_survive_daemon_restart",
             {p["pane_id"]: p["cwd"] for p in before_snapshot["panes"]}
             == {p["pane_id"]: p["cwd"] for p in after_snapshot["panes"]})
    rec.note(f"terminals reused: {before_terms == after_terms} "
             f"(pane ids reused: {after_panes == before_panes})")
    rec.note(f"after restart: {len(after_panes)} pane(s) - survived: {after_panes == before_panes}")

    if after_panes:
        after_screen = _read_visible(client, after_panes[0])
        rec.fact("scrollback_survives_daemon_restart", marker in after_screen)
        rec.note(f"scrollback marker survived: {marker in after_screen}")
    else:
        rec.fact("scrollback_survives_daemon_restart", False)

    # The restore half: can the exported tree be put back?
    root = before_export.get("layout", {}).get("root")
    rec.fact("export_carries_root", root is not None)
    rec.fact("export_carries_cwd_per_pane", '"cwd"' in json.dumps(before_export))
    if root is None:
        rec.note("no root in the export, so nothing to apply back")
        return

    ok, applied = client.try_request("layout.apply", {"root": root, "focus": True})
    time.sleep(1.2)
    rec.fact("layout_apply_accepted_an_export", ok)
    if not ok:
        rec.fact("layout_apply_error", applied)
        rec.note(f"layout.apply refused the exported tree: {applied}")
        return

    restored = client.request("session.snapshot")["snapshot"]
    rec.write_json("after-apply.snapshot.json", restored)
    # Additive or replacing? A restore that duplicates instead of rebuilding is a very
    # different feature, and the difference is invisible until someone runs it twice.
    rec.fact("layout_apply_is_additive", len(restored["panes"]) > len(after_panes))
    rec.fact("panes_before_apply", after_panes)
    restored_layout = next((l for l in restored["layouts"] if l.get("panes")), {})
    rec.fact("panes_after_apply", sorted(p["pane_id"] for p in restored["panes"]))
    rec.fact("pane_count_after_apply", len(restored["panes"]))
    rec.fact("splits_after_apply", len(restored_layout.get("splits", [])))
    rec.fact("cwds_after_apply", sorted({p.get("cwd") for p in restored["panes"]}))
    rec.note(f"layout.apply rebuilt {len(restored['panes'])} pane(s), "
             f"{len(restored_layout.get('splits', []))} split(s)")


# --------------------------------------------------------------------------- 12


def _rect_key(rect) -> tuple:
    return (rect["x"], rect["y"], rect["width"], rect["height"])


def _union(first, second):
    if first is None or second is None:
        return first or second
    left = min(first["x"], second["x"])
    top = min(first["y"], second["y"])
    right = max(first["x"] + first["width"], second["x"] + second["width"])
    bottom = max(first["y"] + first["height"], second["y"] + second["height"])
    return {"x": left, "y": top, "width": right - left, "height": bottom - top}


def _covered_rect(node, pane_rects):
    """The rect a subtree covers, built only from the pane rects the snapshot lists.

    The question behind this: a client holding rects and no tree wants to know which
    node contains which, and containment is the only relationship rects have. If a
    split's own rect is exactly the union of the panes beneath it, the tree can be
    rebuilt from geometry. If it is not, it cannot.
    """
    if not isinstance(node, dict):
        return None
    if node.get("type") == "pane":
        return pane_rects.get(node.get("pane_id"))
    return _union(_covered_rect(node.get("first"), pane_rects),
                  _covered_rect(node.get("second"), pane_rects))


def _split_nodes(node, path=()):
    """Every split in an exported tree, with the turns that reach it.

    A path of booleans is herdr's own way of naming a divider - layout.set_split_ratio
    takes exactly this - so it is the address a resize would be sent back on.
    """
    if not isinstance(node, dict) or node.get("type") != "split":
        return []
    return ([(list(path), node)]
            + _split_nodes(node.get("first"), path + (False,))
            + _split_nodes(node.get("second"), path + (True,)))


def _tree_shape(node):
    """The tree as one readable line, so a corpus reader can see it without walking JSON."""
    if not isinstance(node, dict):
        return "?"
    if node.get("type") == "pane":
        return node.get("pane_id", "?")
    return (f"{node.get('direction')}({_tree_shape(node.get('first'))}, "
            f"{_tree_shape(node.get('second'))}@{node.get('ratio')})")


def _tab_layout(snapshot, tab_id="w1:t1"):
    return next((l for l in snapshot.get("layouts", []) if l.get("tab_id") == tab_id), {})


def _id_path(split_id):
    """The path a border id spells, if it spells one.

    herdr names them `split_<n>_<turns>`, where the turns are `root` for the top and a
    string of 0s and 1s below it. Read rather than trusted: this exists to be compared
    against the export's own paths, not to be relied on before they agree.
    """
    if not split_id:
        return None
    turns = split_id.rsplit("_", 1)[-1]
    if turns == "root":
        return []
    return [c == "1" for c in turns] if set(turns) <= {"0", "1"} else None


def _crowd(client, until_panes: int = 16):
    """Splits until the pane rects run out of room, and reports where rebuilding stops.

    Muster budgets fifteen panes, and herdr sizes these rects for a fixed area of its
    own. A width that reaches zero is a rect that no longer says which node contains
    which, so this finds the pane count where geometry stops being an answer instead of
    discovering it in a window later.
    """
    steps = []
    last_good = 0
    while True:
        snapshot = client.request("session.snapshot")["snapshot"]
        tab = _tab_layout(snapshot)
        rects = {p["pane_id"]: p["rect"] for p in tab.get("panes", [])}
        export = client.request("layout.export", {})
        root = export.get("layout", {}).get("root")
        borders = {(_rect_key(s["rect"]), s["direction"]): s for s in tab.get("splits", [])}

        matched, distinct = 0, set()
        nodes = _split_nodes(root)
        for _path, node in nodes:
            covered = _covered_rect(node, rects)
            if covered:
                distinct.add((_rect_key(covered), node.get("direction")))
                if borders.get((_rect_key(covered), node.get("direction"))):
                    matched += 1
        # Two subtrees covering one rect in one direction are two nodes a client cannot
        # tell apart, which is the failure this is looking for - and it is not the same
        # as a border simply going missing.
        rebuildable = matched == len(nodes) and len(distinct) == len(nodes)
        steps.append({
            "panes": len(rects),
            "splits": len(nodes),
            "borders_matched": matched,
            "rects_distinct": len(distinct) == len(nodes),
            "smallest_rect": min(
                (min(r["width"], r["height"]) for r in rects.values()), default=None),
            "rebuildable": rebuildable,
        })
        if not rebuildable or len(rects) >= until_panes:
            return {
                "steps": steps,
                "last_good_pane_count": last_good,
                "verdict": ("held to the end" if rebuildable
                            else f"broke at {len(rects)} panes"),
            }
        last_good = len(rects)

        # Split the pane with the most room left, which is what a person does and what
        # keeps the tree from degenerating into one deep spine.
        biggest = max(rects.items(), key=lambda item: item[1]["width"] * item[1]["height"])[0]
        direction = "right" if rects[biggest]["width"] >= rects[biggest]["height"] else "down"
        ok, _ = client.try_request(
            "pane.split", {"direction": direction, "target_pane_id": biggest, "cwd": "/tmp"})
        if not ok:
            steps.append({"panes": len(rects), "split_refused": True})
            return {"steps": steps, "last_good_pane_count": last_good,
                    "verdict": f"herdr refused a split at {len(rects)} panes"}
        time.sleep(0.35)


def layout(daemon, rec: Recorder) -> None:
    """Where a window's splits come from, and how far the rects can be trusted.

    The corpus's deepest tree is three panes at two levels, nested only ever to the
    right, recorded as a side effect of the lifecycle scenario. A reconstruction
    checked against that is checked against nothing: it would pass while getting
    `first`-side nesting, alternating directions and any depth beyond two wrong.

    So this builds five panes at three levels with splits on both sides of the root,
    and records the three things that describe it - the snapshot's rects, the live
    layout_updated events, and layout.export's tree - in one run, so they can be
    compared rather than believed.

    Two other questions decide the design and have no recording at all. Whose window
    the rects describe: herdr computes them for a client's terminal area, and if they
    move when a client attaches at another size then only ratios survive. And which
    changes announce themselves: a divider dragged by another client is a change
    Muster has to follow, and a client that never hears about it renders a stale tree
    that looks perfectly healthy.
    """
    client = RecordingClient(daemon.client(), rec)
    _new_workspace(client)
    time.sleep(0.5)

    with daemon.client().subscribe(STRUCTURE_SUBSCRIPTIONS) as stream:
        time.sleep(0.8)

        # Five panes, three levels, and splits under both sides of the root:
        #   right( down(p1, right(p3, p4)), down(p2, p5) )
        #
        # Splitting a pane puts it on the `first` side and the new pane on `second`, so
        # splitting p1 after it already has a sibling is what produces nesting under
        # `first` - the case the existing recording does not have, and the one a
        # reconstruction written against that recording would get wrong.
        for target, direction in (
            ("w1:p1", "right"),   # root: p1 | p2
            ("w1:p1", "down"),    # p1 becomes p1 / p3, under the root's first child
            ("w1:p3", "right"),   # p3 becomes p3 | p4, three levels deep
            ("w1:p2", "down"),    # p2 becomes p2 / p5, so both root children are splits
        ):
            client.request("pane.split",
                           {"direction": direction, "target_pane_id": target, "cwd": "/tmp"})
            time.sleep(0.4)
        time.sleep(0.6)

        deep_snapshot = client.request("session.snapshot")["snapshot"]
        deep_export = client.request("layout.export", {})
        rec.write_json("deep-session.snapshot.json", deep_snapshot)
        rec.write_json("deep-layout.export.json", deep_export)

        root = deep_export.get("layout", {}).get("root")
        tab = _tab_layout(deep_snapshot)
        pane_rects = {p["pane_id"]: p["rect"] for p in tab.get("panes", [])}
        rec.fact("deep_tree_shape", _tree_shape(root))
        rec.fact("deep_tree_depth", _tree_depth(root))
        rec.fact("deep_tree_directions", sorted(_directions(root)))
        rec.fact("deep_pane_count", len(pane_rects))
        rec.fact("deep_area", tab.get("area"))
        rec.fact("deep_pane_rects", pane_rects)
        rec.fact("deep_split_ids", [s.get("id") for s in tab.get("splits", [])])
        rec.note(f"built {_tree_shape(root)}")

        # Can the tree be rebuilt from rects alone? For every split the export names,
        # is there a border in the snapshot covering exactly the panes beneath it, with
        # the same direction and ratio?
        borders = {(_rect_key(s["rect"]), s["direction"]): s for s in tab.get("splits", [])}
        pairings = []
        for path, node in _split_nodes(root):
            covered = _covered_rect(node, pane_rects)
            border = borders.get((_rect_key(covered), node.get("direction"))) if covered else None
            pairings.append({
                "path": path,
                "direction": node.get("direction"),
                "export_ratio": node.get("ratio"),
                "covered_rect": covered,
                "matched_split_id": border.get("id") if border else None,
                "matched_ratio": border.get("ratio") if border else None,
            })
        rec.fact("split_pairings", pairings)
        rec.fact("every_split_has_a_border_covering_exactly_its_panes",
                 all(p["matched_split_id"] for p in pairings))
        rec.fact("border_ratios_match_the_export",
                 all(p["matched_ratio"] == p["export_ratio"] for p in pairings))
        rec.note("rect union identifies every split border: "
                 f"{all(p['matched_split_id'] for p in pairings)}")

        # A second, independent route to the same tree. The border ids look like
        # `split_2_01`, which reads as a path of first/second turns - so if they agree
        # with the export's paths, geometry is not the only way to rebuild the tree and
        # a disagreement between the two would be a herdr change worth noticing.
        rec.fact("split_id_paths_match_the_export_paths",
                 all(_id_path(p["matched_split_id"]) == p["path"] for p in pairings
                     if p["matched_split_id"]))
        rec.note("split ids spell the same paths as the export tree: "
                 f"{all(_id_path(p['matched_split_id']) == p['path'] for p in pairings if p['matched_split_id'])}")

        # Whose window do these rects describe? Nothing was attached above, so if they
        # move now, they are about a viewer rather than about the session - and a client
        # rendering at its own size can use ratios and nothing else.
        before_attach = _tab_layout(client.request("session.snapshot")["snapshot"])
        with PaneStream(daemon, "w1:p1", cols=200, rows=50) as _viewer:
            time.sleep(1.2)
            during_attach = _tab_layout(client.request("session.snapshot")["snapshot"])
        time.sleep(0.6)
        after_detach = _tab_layout(client.request("session.snapshot")["snapshot"])
        rec.fact("area_with_no_client", before_attach.get("area"))
        rec.fact("area_with_a_200x50_client", during_attach.get("area"))
        rec.fact("area_after_that_client_left", after_detach.get("area"))
        rec.fact("area_follows_an_attached_client",
                 before_attach.get("area") != during_attach.get("area"))
        rec.note(f"area with no client {before_attach.get('area')}, "
                 f"with a 200x50 client {during_attach.get('area')}")

        # A divider moved by somebody else. If this is silent, a client's tree is stale
        # the moment another client drags, with nothing to say so.
        before_ratio = len(stream.snapshot())
        ok, _ = client.try_request("layout.set_split_ratio",
                                   {"tab_id": "w1:t1", "path": [], "ratio": 0.3})
        time.sleep(1.0)
        ratio_events = [e.get("event") for e in stream.snapshot()[before_ratio:]]
        after_ratio = _tab_layout(client.request("session.snapshot")["snapshot"])
        rec.fact("set_split_ratio_accepted", ok)
        rec.fact("set_split_ratio_events", ratio_events)
        rec.fact("root_ratio_after_setting_it_to_0_3",
                 next((s["ratio"] for s in after_ratio.get("splits", [])
                       if s.get("id", "").endswith("root")), None))
        rec.note(f"a divider moved by another client announces: {ratio_events or 'NOTHING'}")

        # Zoom, which is a layout state rather than a change of tree: a view that
        # ignores it renders every pane while the daemon shows one.
        before_zoom = len(stream.snapshot())
        zoom_ok, _ = client.try_request("pane.zoom", {"pane_id": "w1:p4", "mode": "on"})
        time.sleep(1.0)
        zoomed = _tab_layout(client.request("session.snapshot")["snapshot"])
        rec.fact("zoom_accepted", zoom_ok)
        rec.fact("zoom_events", [e.get("event") for e in stream.snapshot()[before_zoom:]])
        rec.fact("zoomed_flag", zoomed.get("zoomed"))
        rec.fact("pane_rects_while_zoomed",
                 {p["pane_id"]: p["rect"] for p in zoomed.get("panes", [])})
        rec.note(f"zoomed={zoomed.get('zoomed')}, "
                 f"{len(zoomed.get('panes', []))} pane(s) still listed")
        client.try_request("pane.zoom", {"pane_id": "w1:p4", "mode": "off"})
        time.sleep(0.8)

        # And the collapse: closing a pane takes its parent split with it, so a client
        # that applies the removal without re-reading the tree renders a divider with
        # one side.
        before_close = len(stream.snapshot())
        client.request("pane.close", {"pane_id": "w1:p4"})
        _wait_for_kind(stream, "pane_closed", "pane.closed")
        time.sleep(0.8)
        collapsed_export = client.request("layout.export", {})
        collapsed = _tab_layout(client.request("session.snapshot")["snapshot"])
        rec.write_json("collapsed-layout.export.json", collapsed_export)
        rec.fact("close_events", [e.get("event") for e in stream.snapshot()[before_close:]])
        rec.fact("collapsed_tree_shape", _tree_shape(collapsed_export.get("layout", {}).get("root")))
        rec.fact("collapsed_split_ids", [s.get("id") for s in collapsed.get("splits", [])])
        rec.note(f"after closing one pane: {_tree_shape(collapsed_export.get('layout', {}).get('root'))}")

        # Last, because it leaves the session crowded: how far rects scale. The area
        # above is fixed at 54x23 whoever is watching, Muster budgets fifteen panes, and
        # a rect that collapses to nothing no longer says which node contains which.
        crowded = _crowd(client)
        rec.fact("crowded", crowded)
        rec.note(f"rects rebuild the tree up to {crowded['last_good_pane_count']} pane(s); "
                 f"{crowded['verdict']}")

        events = stream.snapshot()

    rec.write_text("events.ndjson", "".join(json.dumps(e, sort_keys=True) + "\n" for e in events))
    updates = [e for e in events if e.get("event") == "layout_updated"]
    rec.fact("layout_updated_count", len(updates))
    rec.fact("event_kinds", sorted({e["event"] for e in events if "event" in e}))
    rec.note(f"{len(updates)} layout_updated event(s) across {len(events)} in total")


ALL = {
    "snapshot": snapshot,
    "frames": frames,
    "agent-states": agent_states,
    "detection": detection,
    "geometry": geometry,
    "input-path": input_path,
    "input-encoding": input_encoding,
    "frame-fidelity": frame_fidelity,
    "lifecycle": lifecycle,
    "layout": layout,
    "durability": durability,
}
