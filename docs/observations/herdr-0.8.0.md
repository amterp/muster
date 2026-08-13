# herdr 0.8.0 on the wire

What a real herdr daemon does, as opposed to what its source reads like it does.
Recorded 2026-08-11 against herdr 0.8.0, protocol 19, macOS/arm64.

Every verdict here points at a file under `corpus/herdr-0.8.0/`, and section 8 re-runs
the lot against Linux. Re-record with `tools/herdr-probe/probe`; source citations are
against the `v0.8.0` tag at `~/src/herdr`.

Four sections were the load-bearing claims `architecture.md` rested on without ever
having watched them. Two survived unchanged, two survived with their mechanism wrong,
and the protocol itself turned out to work differently from what either document
assumed.

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
| ten seconds later | **120 x 40** |

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
stayed at 120x40 rather than returning to 53x23, and was still 120x40 ten seconds
later. A Muster that quits leaves every pane it touched sized to a window that no
longer exists, and the herdr TUI inherits that. `architecture.md` claimed the hold
releases; it does not. This is the one finding here that looks like a defect rather
than a design, and it sits directly against "sessions outlive everything".

Also worth not tripping over: `pane.layout` reports a rect of 54x23 at (26,1)
throughout, unchanged by any of the above. That is the pane's position in herdr's own
TUI layout, not its PTY size. Nothing in the pane object exposes the PTY dimensions.

Evidence: `corpus/herdr-0.8.0/geometry/`.

## 5. The input path - overturned

Recorded 2026-08-11 as "encoding stays daemon-side". Re-recorded 2026-08-13, and it is
the other way around: **`terminal.input` on a control stream is a raw write to the pane's
PTY.** herdr does not parse those bytes and does not re-encode them.

`cat -v` running in a pane with no kitty keyboard, no bracketed paste and no application
cursor mode, fed each sequence through `terminal.input`:

| Sent | Printed | A re-encode would print |
|---|---|---|
| `a` | `a` | `a` |
| `\x1b[97u` | `^[[97u` | `a` |
| `\x1b[200~x\x1b[201~` | `^[[200~x^[[201~` | `x` |
| `\x1bOA` | `^[OA` | `^[[A` |
| `\x1b[A` | `^[[A` | `^[[A` |

The bytes arrive exactly as sent.

The re-encoding in `raw_input.rs` is real, and belongs to a different connection. Input
forks on the client's mode (`src/server/headless.rs:2852`):

- `ClientConnectionMode::App` - herdr's own TUI. Bytes go through `client.raw_input.push`,
  become structured events, and the runtime re-encodes them for the pane's modes with
  libghostty-vt's key encoder (`src/ghostty/mod.rs:2547`).
- `ClientConnectionMode::TerminalAttach` - what `herdr terminal session control <pane>`
  is, and the only input channel the JSON API offers. Bytes go to
  `apply_terminal_attach_input` (`:403`), which is `runtime.try_send_bytes` and nothing
  else.

Reading the first branch and taking it for both is how this got recorded backwards.

`pane.send_keys` is unchanged, and still cannot stand in: it accepted 24 of 31 probed key
names, refusing `home`, `end`, `pageup`, `pagedown`, `insert`, `delete` and
`ctrl+alt+delete` with `invalid_key`. A semantic key API missing the whole navigation
cluster cannot carry a terminal's keyboard.

**So encoding is the client's job, and the client cannot see what to encode for.** No pane
terminal mode is readable anywhere in the API - not on `PaneInfo`, not in
`pane.process_info` - and section 2 showed the frame stream consumes the sequences that
set them. Muster has to encode against a guess: application cursor keys decide `\x1bOA`
against `\x1b[A`, kitty flags decide whether shift+enter is distinguishable at all,
bracketed paste decides whether paste markers reach the program as text, and mouse
reporting decides whether an SGR click is input or garbage.

Two things bound the damage.

**`terminal.scroll` is the counter-example, and the shape the ask should take.** It is
structured rather than bytes, and the server answers it against the pane's real state
(`apply_terminal_attach_scroll`, `:340`): it reads `wheel_routing()` and either encodes an
SGR wheel event for a mouse-reporting program, sends alternate-scroll keys, or scrolls its
own scrollback. A control-stream client gets the wheel right while knowing nothing. Keys
and mouse buttons have no equivalent.

**herdr already holds the state Muster is missing, in one serializable struct.**
`crate::pane::InputState` (`src/pane/terminal.rs:113`) carries `application_cursor`,
`bracketed_paste`, `alternate_screen`, `focus_reporting`, `mouse_protocol_mode`,
`mouse_protocol_encoding` and `modify_other_keys`; it derives `Serialize`; and the runtime
hands it out as `input_state()`. Kitty flags are tracked alongside it
(`src/pane/kitty_keyboard.rs`). Publishing that struct on the pane object, or taking
structured key events the way `terminal.scroll` already takes scrolls, is a smaller change
than it sounds - which is what the upstream ask should say.

Still open, and needing the Swift surface: IME composition, AltGr, dead keys, and how much
the guess costs against an agent that negotiates the kitty protocol.

Evidence: `corpus/herdr-0.8.0/input-encoding/` for this section,
`corpus/herdr-0.8.0/input-path/` for the `send_keys` vocabulary and the mode-exposure
probes. The `raw-input-echo.json` in `input-path/` supports no claim about encoding: its
`stty raw -echo; cat -v` never ran - leftover bytes from the `send_keys` probe corrupted
the command line - so what it captured is a cooked-mode shell echo.

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

## 7. Screen detection does not need a viewer - confirmed

The question worth asking, because `architecture.md` has hidden panes detach their
channels: does herdr still analyze a pane nobody is watching?

It does. A pane running a screen-detected agent with no client attached tracked every
state change:

| Screen painted | no viewer attached | control stream attached |
|---|---|---|
| `working` | 2.09s | 0.26s |
| `idle` | 0.78s | 0.52s |
| `blocked` | 0.26s | 0.26s |
| `idle` | 0.26s | 0.26s |

Detection is unconditional; the only difference is that the first transition after an
agent starts is slower, and that happens with or without a viewer. So hidden panes may
detach their channels without going blind, and the data plane stays free to scale with
visible panes.

Worth recording how this was nearly got wrong: earlier attempts polled for about a
second and concluded detection was broken, then that it required a viewer. Both
readings came from a poll shorter than the first transition. The scenario now settles
on a state rather than sampling once.

Agent identity comes from the pane's foreground process name, and override manifests
live at `<config>/agent-detection/<agent>.toml` keyed on herdr's known-agent enum
(`src/detect/manifest.rs:1097`). A fixture cannot introduce its own agent id, so
`fake-agent/` borrows `claude`'s and replaces its rules.

Evidence: `corpus/herdr-0.8.0/detection/`.

## 8. Linux behaves the same as macOS

Every scenario above was re-run against the Linux daemon in the devenv container, over
SSH, and diffed:

```
$ tools/herdr-probe/diff-corpus corpus/herdr-0.8.0 corpus/herdr-0.8.0-linux
0 difference(s) across 6 shared scenario(s); 5 volatile fact(s) not compared
```

Not one recorded fact differs. The attach frame is the same 35,605 bytes, the PTY
walks the same 53x23 to 100x30 to 120x40, `done` derives the same way, and
`pane.send_keys` refuses the same seven key names. The five facts not compared are
timings and frame counts, which are not expected to match.

So the remote path is the same path, and "local and remote in one window" costs the
adapter nothing beyond the transport. Re-run this on every herdr upgrade: the day it
stops printing zero is the day the remote path needs its own handling.
