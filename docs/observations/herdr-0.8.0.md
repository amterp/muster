# herdr 0.8.0 on the wire

What a real herdr daemon does, as opposed to what its source reads like it does.
Recorded 2026-08-11 against herdr 0.8.0, protocol 19, macOS/arm64.

Every verdict here points at a file under `corpus/herdr-0.8.0/`. Re-record with
`tools/herdr-probe/probe`; source citations are against the `v0.8.0` tag at
`~/src/herdr`.

Four of the five sections were the load-bearing claims `architecture.md` rested on
without ever having watched them. Two survived unchanged, two survived with their
mechanism wrong, and the protocol itself turned out to work differently from what
either document assumed.

## 1. `session.snapshot` is live structure - confirmed

`session.snapshot` returns the running session: workspaces, tabs, panes, agents,
layouts, and the three focus cursors (`focused_workspace_id`, `focused_tab_id`,
`focused_pane_id`). Each pane carries `agent_status`, `focused`, `cwd`,
`foreground_cwd`, `scroll`, and `revision`.

`layout.export` returns `{layout, type}` and none of those live fields. It is a
restore tree, as `origin.md` already corrected itself to say.

So the mirror bootstraps from `session.snapshot`. One wrinkle: `events.subscribe`
also replays current state as synthetic events - subscribing to an already-populated
session produced 9 events describing what already existed, not just what changed
afterwards. A client that snapshots and subscribes will see every entity twice.
Convergent application handles that, which is what `architecture.md` already
requires, but it is a property to rely on rather than a coincidence.

Evidence: `corpus/herdr-0.8.0/snapshot/`.

## 2. Frame diffs consume mode changes - confirmed

Six mode-setting sequences were emitted inside a pane while its stream was recorded:
bracketed paste (2004), alt screen (1049), kitty keyboard (`CSI > 1 u`), mouse
tracking (1000 and SGR 1006), and focus reporting (1004). **None reached the frame
stream.** The daemon's VT consumes them.

This is the fact the whole control/data-plane split rests on, and it holds: a surface
fed by this stream cannot know the pane's modes, so it cannot encode input.

What the stream does carry, on attach: one frame, `full: true`, `seq: 1`, 35,605
bytes for an 80x24 screen. The payload is a re-encoded screen - synchronized-output
guard, cursor hide, clear, then absolute positioning and SGR per cell
(`\x1b[1;1H\x1b[0;39;49ms`). Not the inner program's output at any point.

The envelope is `{type, seq, encoding, full, width, height, bytes}`, base64 in
`bytes`. Frames carry a monotonic `seq`, so **the data plane does have sequence
numbers** even though the control plane does not.

Evidence: `corpus/herdr-0.8.0/frames/`, raw payload in `frame-001-attach.ansi`.

## 3. Five states, and `done` is derived - confirmed, mechanism wrong

The schema's `AgentStatus` has five values: `idle`, `working`, `blocked`, `done`,
`unknown`. What a client may *report* is `PaneAgentState`, which has four -
`pane.report_agent` rejects `done` with `unknown variant 'done'`.

`done` is not a state the daemon stores. It is `idle` plus not-seen:

```rust
// src/app/api_helpers.rs:104
(AgentState::Idle, false) => AgentStatus::Done,
(AgentState::Idle, true)  => AgentStatus::Idle,
```

Seen-ness is written on a completion transition (`working` or `blocked` into `idle`)
and nowhere else:

```rust
// src/app/actions.rs:3078, :59
if change.state != AgentState::Idle { pane.seen = true }
else if is_completion_transition(change) { pane.seen = suppress_active_tab_notifications }

fn active_tab_suppresses_notifications(is_active_tab, outer_terminal_focus) -> bool {
    is_active_tab && outer_terminal_focus != Some(false)
}
```

Observed, and matching that reading exactly:

| Completion transition happens... | resulting status |
|---|---|
| with the pane's tab active | `idle` |
| with the pane's tab in the background | `done` |
| then that tab is focused | `idle` |

Pushed as events in order: `working`, `done`, `idle`. The daemon publishes the derived
value, so a client never computes it.

**The correction.** `architecture.md` says seen-ness is fed by reporting *pane* focus,
and that the vocabulary needs a "pane was seen" intent. Neither matches. Seen-ness is
gated on two things, and pane focus is not one of them:

1. whether the pane's **tab** is the daemon's active tab, and
2. the foreground client's **`outer_terminal_focus`** - whether the client's host
   window has OS focus.

A client feeds the second one by sending the DEC focus-reporting sequences in its raw
input stream (`\x1b[I` gained, `\x1b[O` lost, `src/raw_input.rs:802`), or the
equivalent protocol message on the client socket (`src/protocol/wire.rs:333`).

There is no way to do it over the public JSON API. `pane.mark_seen`, `client.focus`,
and `client.outer_focus` are all unknown methods, and sending `\x1b[I` through
`terminal.input` on a control stream left the status at `done` - those bytes go to the
pane's program, not to the client-focus machinery.

`pane.focus` does move a `done` pane to `idle`, because focusing a pane in another tab
activates that tab. That covers the common case and is what Muster should call. It
does not cover a window that loses OS focus while its active tab holds a running
agent, which is exactly when "did the user see it" stops being guessable from
daemon-side state.

Evidence: `corpus/herdr-0.8.0/agent-states/`.

## 4. A controlling client holds the pane's geometry - confirmed, and it does not let go

The pane's real PTY size, read by asking the pane's own shell for `stty size`:

| State | PTY (cols x rows) |
|---|---|
| no viewer attached | 53 x 23 |
| `session control --cols 100 --rows 30` | 100 x 30 |
| plus `session observe --cols 80 --rows 24` | 100 x 30 |
| controller sends `terminal.resize 120x40` | 120 x 40 |
| controller detaches | **120 x 40** |

Three things follow.

The controlling client drives the real PTY, so the card's `direct_attach_resize_locks`
premise holds and `architecture.md`'s "geometry follows the controller" is right.

Observers are not forced to the controller's size. The observer asked for 80x24 and
got 80x24 frames while the PTY stayed 100x30 - the daemon re-renders the screen into
each viewer's requested viewport. So `architecture.md` is wrong that "other clients
view it at that size", and wrong that concurrent TUI viewing is degraded by geometry.
It is degraded by re-rendering a larger screen into a smaller viewport, which is a
different and milder problem.

**The hold does not release on detach.** After the controlling stream closed, the PTY
stayed at 120x40 rather than returning to 53x23. A Muster that quits leaves every pane
it touched sized to a window that no longer exists, and the herdr TUI inherits that.
`architecture.md` claims the hold releases; it does not. This is the one finding here
that looks like a defect rather than a design, and it sits directly against "sessions
outlive everything".

Also worth not tripping over: `pane.layout` reports a rect of 54x23 at (26,1)
throughout, unchanged by any of the above. That is the pane's position in herdr's own
TUI layout, not its PTY size. Nothing in the pane object exposes the PTY dimensions.

Evidence: `corpus/herdr-0.8.0/geometry/`.

## 5. The input path - partly settled

Enough to decide where encoding lives; not enough to close the kill-criterion card.

`pane.send_keys` takes named keys and accepted 24 of 31 probed names. It refused
`home`, `end`, `pageup`, `pagedown`, `insert`, `delete`, and `ctrl+alt+delete` with
`invalid_key`. A semantic key API missing the whole navigation cluster cannot carry a
terminal's keyboard.

`terminal.input` on a control stream accepts raw bytes (base64, or `text`). That is
the channel herdr's own TUI uses: it forwards what its host terminal gave it, and the
server parses those bytes in `raw_input.rs` and re-encodes them for the pane's current
modes.

No pane terminal mode is readable anywhere in the API - not on the pane object, not in
`pane.process_info`. Combined with section 2, that settles it: **an adapter cannot
encode input itself, and encoding must stay daemon-side.** Muster reports bytes, herdr
encodes. This is the same answer `architecture.md` reached, by a firmer route.

Still open, and needing the Swift surface: IME composition, AltGr, dead keys, and
whether byte-level reporting preserves enough for the kitty protocol in practice.

Evidence: `corpus/herdr-0.8.0/input-path/`.

## 6. Protocol facts nothing had written down

**One request per connection.** The API socket answers a request and closes. Three
pings on one connection get one response and then `EPIPE`. Every intent Muster
dispatches pays a `connect()`, which is a latency and a rate question the perf card
needs to answer, and it rules out modeling the control plane as one long-lived
request/response channel.

**Subscriptions are the exception.** `events.subscribe` answers
`{"type":"subscription_started"}` and then streams `{event, data}` frames on the same
connection until either side hangs up.

**Event names are spelled two ways.** Most arrive snake_cased (`pane_created`,
`tab_focused`, `workspace_focused`). The three subscriptions that take parameters keep
their dotted subscription name in the event: `pane.agent_status_changed`,
`pane.output_matched`, `pane.scroll_changed`. An adapter must accept both, and this
cost one silently-empty result during recording before it was caught.

**There are per-entity revision counters.** Panes carry `revision`, agents carry
`state_change_seq`, frames carry `seq`. `architecture.md` says herdr "exposes no event
sequence numbers and no replay, so a gap cannot be detected, only survived". The
no-replay half is right. The no-sequence-numbers half is not, and whether these
counters are enough to detect a gap is worth settling before the reconciliation
cadence gets chosen.

## Unresolved

**Screen-detection manifests did not fire in a headless daemon.** A pane running a
binary named `claude` was detected as a claude agent and reported `idle`, but a
local override manifest at `<config>/agent-detection/claude.toml` - which herdr
confirmed it had loaded, `local override active` - never moved the state off `idle`
regardless of what the pane painted. Custom agent ids cannot be added at all: the
override path is keyed on herdr's known-agent enum (`src/detect/manifest.rs:1097`).

Untested and load-bearing if true: whether screen detection needs a client actually
viewing the pane. If it does, hidden panes stop updating their agent states, and
"hidden panes detach their channels" in `architecture.md` becomes expensive. The probe
attaches control streams, so this is cheap to answer next.

The fake agent works around it by reporting its lifecycle through the API instead,
which is deterministic and exercises the same seen-ness machinery.
