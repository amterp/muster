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
    rec.note(f"with no viewer attached the pane's PTY is {rec._facts['pty_size_no_viewer']}")

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
        rec.note(f"controller at 100x30 -> PTY {rec._facts['pty_size_under_controller']}, "
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
                     f"PTY stayed {rec._facts['pty_size_with_observer_attached']}")
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
                 f"PTY {rec._facts['pty_size_after_resize']}")
    finally:
        control.close()

    # The one that matters for "sessions survive anything Muster does": when the
    # controller goes away, does the pane go back to the size everyone else uses?
    time.sleep(1.5)
    pane4, layout4 = dims()
    rec.write_json("pane-after-detach.json", {"pane": pane4, "layout": layout4})
    after_detach = _pty_size(client)
    rec.fact("pty_size_after_controller_detached", after_detach)
    rec.fact("geometry_hold_released_on_detach", after_detach == rec._facts["pty_size_no_viewer"])
    rec.note(f"after the controller detached the PTY is {after_detach} "
             f"(was {rec._facts['pty_size_no_viewer']} before any viewer attached)")


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


ALL = {
    "snapshot": snapshot,
    "frames": frames,
    "agent-states": agent_states,
    "geometry": geometry,
    "input-path": input_path,
}
