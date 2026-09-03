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

Settled 2026-08-15, and not by herdr. Muster derives `done` itself, from the transitions
the agent channel already delivers and a window focus the shell reports across its own
seam - and normalizes herdr's answer back to `idle` on the way in, so there is one writer
rather than two disagreeing. `pane.focus` is still called, for the tab activation other
clients read. See `architecture.md` (agent state) and `corpus/conformance/attention.json`.
What that leaves open is only what no client can answer: a second Muster window, or a
herdr TUI beside us, is focus we cannot observe.

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

Three things bound the damage.

**The daemon will encode for us, on a different channel.** `pane.send_input`
(`{pane_id, text, keys}`) encodes text with bracketed paste applied from the pane's real
`input_state()` (`src/app/api_helpers.rs:25`) and named keys with the same ghostty
encoder herdr's TUI uses (`:37`). So mode-aware encoding is not missing from herdr - it
is missing from the *control stream*, which is the channel a client holds open. Two
things keep this from simply being the answer: the JSON socket is one request per
connection, so every keystroke would pay a `connect()` on the latency path, and the key
vocabulary cannot name the navigation cluster (section 5's `send_keys` result). It is a
usable fallback for the cases where the guess is known wrong - paste above all - rather
than a replacement for local encoding.

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

**Measured, 2026-08-13, driving a real pane through Muster.** The guess costs less than
feared and exactly one thing more than expected.

| Case | Result |
| --- | --- |
| Printable text, enter, ctrl+C, tab | works |
| shift+enter in Claude Code | works - `CSI 27;2;13~` is understood without kitty negotiation |
| Dead keys (`option+e`, `e` → `é`) | works |
| Arrows in `vim` | works |
| Arrows in `less` | was broken; now correct, see below |
| Paste | was unfenced; now correct, see below |

`less` is the case that fails, and it fails loudly rather than subtly. It calls terminfo's
`smkx` (`\E[?1h\E=`) on startup, which turns application cursor mode on, and then decodes
arrows as `kcuu1`/`kcud1`, which `xterm-256color` defines as `\EOA`/`\EOB`. Sent `\E[B`
instead, it echoes `ESC [ B` into its status line and rings the bell. Verified in a local
pty, no daemon involved: `\E[B` produces a 38-byte response containing `\x07`, `\EOB`
produces 99 bytes of scrolled content and no bell.

`vim` survives the same mode because its own key tables accept both forms, which is why a
single-program check would have concluded the loss was theoretical. It is not: the split is
between programs that trust terminfo and programs that hedge, and the ones that trust
terminfo are the ones a guess breaks.

**Both losses are recovered, by `pane.send_input`.** The earlier reading of that API said
its key vocabulary could not name the navigation cluster. That was drawn from a
`ctrl+alt+delete` rejection and was too broad: `up`, `down`, `left` and `right` are all
accepted. What is genuinely missing is `pageup`, `pagedown`, `home`, `end`, `insert` and
`delete` - `parse_key_combo` (`src/config/keybinds.rs:1201`) never got string aliases for
those `KeyCode`s, though herdr's terminal handles them everywhere else. Keys may also
carry modifiers as `+`-joined strings, so `shift+up` and `ctrl+left` are expressible.

So Muster routes bare arrows and paste through `pane.send_input` and encodes everything
else locally. `less` scrolls. A multi-line paste arrives as text to edit rather than as
commands to run.

The cost was the objection to doing this, and it does not survive measurement: **0.09ms
median, 1.6ms worst over 20 samples**, against the 33ms a key repeat allows. A raw client
pays one connection per request where the CLI pays two - `cli::send_request` opens a
separate connection to `ping` for a version check first (`src/cli.rs:759`).

One thing this arrangement has to get right that a single channel would not: ordering.
Control-stream bytes reach the PTY through the bridge, a routed key reaches it directly,
and the two race - so Muster serializes, and a routed key completes its round trip before
the next byte goes out.

Still open, and needing more of the Swift surface: full IME composition with a candidate
window, AltGr on a non-US layout, and mouse.

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
--- corpus/herdr-0.8.0 (Darwin/arm64)
+++ corpus/herdr-0.8.0-linux (Linux/aarch64)

0 difference(s) across 15 shared scenario(s); 13 volatile fact(s) not compared; 4 not comparable across platforms
```

Not one recorded fact differs. The attach frame is the same 35,605 bytes, the PTY
walks the same 53x23 to 100x30 to 120x40, `done` derives the same way, and
`pane.send_keys` refuses the same seven key names. The thirteen facts not compared are
timings, frame counts, and two kinds of value that are about the machine rather than
about the daemon: the opaque terminal ids stamped per run, and a home directory. What is
being asked of those - were the terminals reused, did the working directories survive -
is recorded separately as a boolean, and those are compared.

The four beside them are a weaker exemption and are worth naming, because a reader
should know what this section does not cover. Three are `arranging`'s move payloads,
which carry a whole layout tree worth comparing and are skipped because a `cwd` and a
`terminal_id` sit inside the same blob. The fourth is where a line too long for the pane
wraps in `read-depth`, measured from where the shell's echo of the command began - so it
reports the width of a prompt as much as the daemon's wrapping. All four are compared
when both recordings come from the same platform.

So the remote path is the same path, and "local and remote in one window" costs the
adapter nothing beyond the transport. `./dev --ssh` re-runs this against a scratch
recording on every invocation, so the day it stops printing zero is the day the remote
path needs its own handling.

Three corrections to what this section said when it was first written, each found by
re-running it after the scenario set grew - from six to eleven, and then to fifteen.

The first is that it was six, and the claim was quietly narrower than it read. The
five scenarios added since - input encoding, layout reconstruction, lifecycle,
durability, and the removal counters - had never been run against Linux at all, and
three of them had no Linux recording to diff against.

The second is a bug in the probe rather than in herdr, and it is the more interesting
one. The remote fixture emptied the session file every time it started the daemon, so
that each scenario began with no leftovers. The durability scenario stops the daemon
mid-run to see what survives a restart - and against the remote fixture that stop and
start emptied the session, so it recorded `session_survives_daemon_restart: false` and
eleven facts that followed from it. Read at face value that is a platform difference
worth designing around; it was an artifact of the measurement. Emptying the session now
happens once per scenario rather than on every start, and a stop over there stops the
daemon rather than only the tunnel, so the event being measured is the event the
scenario names. Linux survives a restart exactly as macOS does, keeping pane ids and
losing terminals.

The third repeats the first, which is the point of writing it down again. Four more
scenarios arrived - `arranging`, `naming`, `read-depth`, `split-sides` - and this section
went on claiming eleven while `./dev --ssh` had been failing on the new four since they
landed. Every difference turned out to be the machine rather than the daemon, which is
what the paragraph above about the four exempted facts now records. One of them was a bug
in the probe again: a recording stamped whichever machine ran it, so the whole Linux
corpus claimed to be `Darwin/arm64`, and nothing caught it because nothing read the
stamp. The diff reads it now, which is what lets it tell a fact two platforms cannot
share from one that merely moved.

## 9. Input-to-glyph is 1.4 ms, and its tail is a render throttle

Added 2026-08-13, against the same daemon. The kill criterion this answers is
`a_26BIX28HG`: double emulation puts a second terminal and a socket between a keystroke
and a glyph, and if that is felt, the architecture is wrong.

Measured with `tools/latency.py`: `cat` on both sides, one byte in, wait for it to come
back, at a fast typist's cadence. The daemon side writes to a pane's control stream and
reads frames off it; the floor writes to a plain PTY and reads the kernel's echo.

| path | min | median | p95 | max |
|---|---|---|---|---|
| plain pty (the floor) | 0.03 | 0.07 | 0.22 | 1.33 |
| daemon: stream responded | 0.49 | 1.03 | 2.61 | 21.96 |
| daemon: glyph painted | 0.49 | 1.41 | 22.60 | 23.27 |

**The daemon answers in about a millisecond.** Every keystroke produces a frame almost
immediately - that column has no tail worth the name.

**The p95 is a second frame, not slow work.** In 23 of 60 samples the frame that arrived
first did not yet carry the byte, and the glyph waited for the next render: the PTY echo
lost a race against a render that was already in flight. `App::can_render_now`
(`src/app/runtime.rs:519`) gates rendering on `MIN_RENDER_INTERVAL`, 16 ms
(`src/app/mod.rs:35`), so the loser of that race waits out the window. That is the whole
shape of the distribution - roughly 1.4 ms or roughly 20 ms, with almost nothing between.

**None of it is Muster's.** The measurement runs no Muster code: it is herdr answering
its own control stream. One `render_and_stream` serves every render target, so herdr's
own TUI is behind the same gate. Muster adds frame decode and VT parse on top, budgeted
at 5.96 and 3.72 ns/byte by `./dev --perf` - about 0.35 ms for a 35 KB full repaint and
microseconds for the diffs that follow.

So the kill criterion passes, and it passes for a reason worth keeping straight: double
emulation is cheap, and what latency there is belongs to a daemon-side throttle that is
tunable rather than to the architecture. It also sets a ceiling Muster cannot beat by
being fast, which is what makes it worth an upstream conversation rather than local
optimization.

Evidence: `tools/latency.py`, run with `./dev --latency`.

One methodological note, recorded because it nearly became a wrong answer. The first
version of this measurement read frames on a second thread and reported a 20 ms median
with a 30 ms tail. The measuring loop held Python's GIL and starved the reader, so the
number described the instrument. Single-threaded blocking reads moved the median from
20 ms to 1.4 ms. Any future timing harness here reads on the thread that is timing.

## 10. Removals, and what the counters actually count

Added 2026-08-13, for the mirror (`a_26DAm1Zt0`). Every earlier scenario recorded a
session that only ever grew, so nothing here had ever watched herdr take something
away - and removal is the half of convergent application that is easy to get wrong.

Evidence: `corpus/herdr-0.8.0/lifecycle/`, recorded with
`tools/herdr-probe/probe lifecycle`.

### A pane whose program exits is announced differently from a pane a client closed

`pane.close` emits `pane_closed`. A pane whose program ends emits `pane_exited` and
**never** a `pane_closed` afterwards - the recorded stream goes straight from
`pane_exited` to `layout_updated`. Both payloads are the same three fields,
`{type, pane_id, workspace_id}`: an id, not the entity, which is all a mirror needs to
drop an entry.

A mirror that keys removal on `pane_closed` alone therefore keeps every exited pane
forever. That is a surface rendering a dead PTY that the user cannot get rid of, and it
would have looked like a Muster bug rather than a missing event name.

### `layout_updated` is the geometry, and it does not fire for tabs

It carries a whole tab's layout - `area`, `panes` with rects, `splits`, `focused_pane_id`,
`tab_id`, `workspace_id`, `zoomed` - in absolute values, so applying one twice is
applying it once. That makes it the right thing to render splits from.

What it does not do is fire for everything. In the recording it followed every
`pane_created`, `pane_closed` and `pane_exited`, and followed **none** of `tab_created`,
`tab_closed` or `workspace_closed`. A client that treats it as "geometry changed" and
refreshes nothing else shows a closed tab's panes until something touches a pane.

Its `splits` are herdr's `SplitBorder` (`src/layout.rs:119`) - boundaries for mouse-drag
resize, with no parent or child links. The BSP tree itself is only in `layout.export`,
which carries none of the live fields. Recorded at three panes and two levels
(`nested-layout.export.json`, `nested-session.snapshot.json`), which is the first
layout in the corpus deeper than a single split.

### The counters answer a narrower question than architecture.md assumed

`architecture.md` said gaps "cannot be detected, only survived", then corrected itself to
note the per-entity counters and left open whether they are enough. Measured, they are
enough for one of the two kinds of change:

| counter | bumped by | use |
|---|---|---|
| pane `revision` | terminal title, metadata tokens | nothing a mirror wants |
| agent `state_change_seq` | any agent state transition, session-wide | detects missed transitions |

A pane's `revision` did not move when its agent went idle to working, and did move when
its title changed (0 -> 0 -> 1). Reading the source says why: it is bumped in exactly
three places, all of them title or metadata-token
(`src/terminal/state.rs:198`, `src/app/actions.rs:1083`, `src/app/api/panes.rs:1411`).
It is not a general "this pane changed" counter and cannot be used as one.

`state_change_seq` is better than per-entity. It is stamped from one session-wide
counter (`src/app/actions.rs:2973`), so panes get interleaved values - the recording
walks `{p1: 2}`, `{p1: 2, p5: 4}`, `{p1: 6, p5: 4}`. Comparing two of them tells a client
that transitions ran in between, **including on panes it has never heard of**.

Two of them, and not a running count, because of where the stamp is: searching the
schema for `state_change_seq` returns exactly one hit, `success_response/$defs/AgentInfo`
- the `agents[]` of a `session.snapshot`. It is on no event. `PaneAgentStatusChangedEvent`
carries `agent`, `agent_status`, `display_agent`, `pane_id`, `state_labels`, `title` and
`workspace_id`, and `PaneInfo` does not carry it either.

So the counter is a between-snapshots measure rather than a live one. A client learns
what it missed at the moment it re-snapshots, which is the moment it stops needing to
know for correctness - the snapshot already fixed the mirror. What survives is an
attention fact: an agent may have gone blocked and back while nobody was listening, and
that is a notification the user never got.

So: agent-state gaps are quantifiable in arrears, structural gaps are not detectable at
all. Nothing reports that a pane was created and closed inside a gap, which leaves
periodic re-snapshot as the only detector for structure. That is the fact the
reconciliation cadence gets chosen against.

### Subscribing still replays

Confirmed again on a smaller session: 7 events for one workspace, one tab, one pane -
`workspace_created`, `workspace_focused`, `tab_created`, `tab_focused`, `pane_created`,
`pane_focused`, `layout_updated`. Section 1 saw 9 for a three-pane session. The replay is
the current session described as synthetic creation events, so snapshot-then-subscribe
sees everything twice and convergent application is not optional.

## 11. Agent state is only delivered to a subscriber that names the pane

Added 2026-08-13, and it constrains the founding desideratum rather than an
implementation detail. Evidence: `corpus/herdr-0.8.0/lifecycle/`, facts
`agent_state_change_events_without_naming_the_pane` and
`agent_state_change_events_when_naming_the_pane`.

Of herdr's 27 subscription types, 24 take no parameters. Three do, and
`pane.agent_status_changed` is one of them - it requires a `pane_id`.

The question was whether some unparameterized subscription carries the same information,
`pane.updated` being the obvious candidate. Measured against one daemon, with a client
subscribed to all 24 unparameterized types:

| driven | what that client received |
|---|---|
| agent -> working | `pane_agent_detected` (once, naming the agent, not its state) |
| agent -> idle | nothing |
| agent -> blocked | nothing |

A second client on the same daemon, subscribed only to
`{"type": "pane.agent_status_changed", "pane_id": "w1:p1"}`, received
`pane.agent_status_changed` for the same transition.

So there is no session-wide agent-state event. "Every pane shows working / blocked / done
/ idle at a glance" costs one subscription per pane, and each is a held-open connection,
started when the pane appears and dropped when it goes. At fifteen panes that is fifteen
connections plus one for structure, and the alternative is polling `session.snapshot`,
which pays a connect per poll and picks its own staleness.

Worth an upstream ask, and the strongest one Muster has found: an unparameterized
`pane.agent_status_changed` would cost herdr one subscription kind and would save every
client that wants an overview from opening a connection per pane. `pane_updated` already
exists and is unparameterized, which suggests the plumbing is there.

Not a blocker for one pane, which is what the app shows today. It is a blocker for the
sidebar, and the number of connections it implies should be settled before that is built.

### What sixteen of them cost, measured

Added 2026-08-15, macOS arm64, release build, growing one session from 1 pane to 15 with a
subscription open. Regenerate with:

```
MUSTER_HERDR=deps/herdr/0.8.0/herdr cargo run --release -p muster-perf --example watcher-cost
```

|             | 1 pane  | 15 panes   | per added pane |
|-------------|---------|------------|----------------|
| RSS         | 2128 KB | 2784 KB    | 46-51 KB       |
| threads     | 3       | 17         | 1              |
| descriptors | 12      | 40         | 2              |

Idle CPU over three seconds holding fifteen watchers: 0.0 ms. Threads parked in a blocking
read cost nothing, so scheduling is not the problem here.

Two descriptors per watcher rather than one, because `watch()` clones the stream so
`Watcher::drop` can shut it down and unpark the thread. The clone is what makes that safe -
the descriptor stays valid for as long as the watcher holds it - so this is the price of a
shutdown that cannot race, not an oversight.

**The upstream ask is worth less than this section claimed.** Under a megabyte and no
measurable CPU at a full window does not justify asking another project to change its API.
An unparameterized `pane.agent_status_changed` would still save every client a connection
per pane, and it is still the cheapest thing herdr could do for clients like this one - but
Muster ships the sidebar without it, and should say so if it asks.

**Thread-per-watcher stays.** One thread and ~48 KB per pane, idle at zero, against a
rewrite that would add a polling dependency or a hand-rolled `poll` to reclaim 700 KB. Not
a trade worth making at this cardinality. Revisit if a window ever holds panes by the
hundred.

**Descriptors are the resource with a ceiling, and it is lower than it looks.** A
developer's shell reports a soft `RLIMIT_NOFILE` in the millions, so measuring from one is
misleading: a GUI-launched app inherits launchd's limit, which defaults to 256 soft and
unlimited hard (`launchctl limit maxfiles`). Fifteen panes on each of two daemons is 60
descriptors in watchers alone, before the structure subscriptions, the control socket per
visible pane, and the bridge subprocess behind each. Muster should raise its own soft limit
at startup rather than meet that ceiling as a pane which silently fails to open.

## 12. A daemon restart keeps the shape and loses the processes

Added 2026-08-13. `architecture.md` promised sessions outlive quitting the app, dropping
the VPN and closing the lid - every one of which is a case where the daemon itself
survives. Nobody had watched what happens when it does not, which is the ordinary
consequence of a crash, an update, or a reboot.

Evidence: `corpus/herdr-0.8.0/durability/`, recorded with
`tools/herdr-probe/probe durability`. A graceful `herdr server stop` was used, so a crash
or a power cut is strictly worse than this and loses at least as much.

| | survives a daemon restart |
|---|---|
| workspaces, tabs, pane tree | yes |
| pane ids | yes - `w1:p1`, `w1:p2`, `w1:p3` came back unchanged |
| per-pane cwd | yes |
| terminal ids | **no** - every one changed |
| scrollback | **no** |

herdr persists and restores the session's *shape*. It does not, and cannot, keep the
processes: `terminal_id` changed on all three panes (`term_658f1ed496e411` became
`term_658f1ed77653f1`), and a marker echoed into a pane before the restart was gone
afterwards. So a pane comes back in the right place, in the right directory, running a
fresh shell.

This is better than assumed and it is the shape a durability story should take. What is
lost is exactly what nothing can save - a live process - and what is kept is exactly what
is cheap to write down. It also means Muster inherits most of this for free rather than
needing a persistence layer of its own, which would have made it the multiplexer it says
it is not.

### `layout.apply` builds, it does not restore

The counterpart to `layout.export` accepts an exported `root` and rebuilds the tree with
the recorded cwds. But it is **additive**: applying a two-pane export to a session that
already had three panes produced five, not three. It creates a layout rather than
reconciling one.

That is the right primitive and the wrong verb for a restore button. Anything built on it
has to decide what it is applying *into* - a fresh workspace, most likely - because
running it twice against the same session silently doubles the panes.

## 13. The tree can be rebuilt from rects, and the rects are about nobody's window

Section 10 recorded the first layout deeper than one split and said so as a limitation:
three panes, two levels, one direction, nested only ever to the right. Anything designed
against that would have been designed against a shape too small to be wrong in an
interesting way. So this builds five panes at three levels, with both directions and a
split under each side of the root, and records the three descriptions of it in one run.

    right( down(p1, right(p3, p4)), down(p2, p5) )

**Every split has a border covering exactly the panes beneath it.** For each split in
`layout.export`'s tree, the union of its descendants' rects appears in the snapshot's
`splits` with the same direction and the same ratio, for all four. So the tree is
recoverable from `layout_updated` alone, which is the event that already fires on every
pane change. Nothing has to ask `layout.export` for structure, which matters because that
call carries none of the live fields and would be a second request per change.

**One shape makes it ambiguous, and it is recorded too.** Splitting a pane along the axis
it already sits on gives `columns(columns(p3, p6), p4)`, where the inner split and its own
first child both start at the outer split's corner and both span its height. Two rectangles
then answer "what is the first child here", and only the larger one is right. Every tree
above has exactly one candidate at every node, so none of them can tell a reconstruction
that picks wrong - which is not a hypothetical: the implementation was broken deliberately
and the corpus passed, and this recording is what closed that.

**The border ids spell the same paths, and so does the resize API.** They are named
`split_<n>_<turns>`, where the turns are `root` at the top and a string of `0`s and `1`s
below it: the split at `first`-then-`second` is `split_2_01`. Those paths agree with the
export tree's own, for every border. And `layout.set_split_ratio` takes `path: [bool]`,
which is that same address. herdr thinks in tree paths, so a client that rebuilds the tree
has also derived the name it needs to move a divider back.

**The rects describe herdr's own terminal, not a viewer's.** The area is 54x23 at (26,1)
with nothing attached, which is a herdr window minus its sidebar. Attaching a control
stream at 200x50 did not move it, and neither did detaching. So the cell numbers are a
fixed fiction to any client rendering at its own size: what transfers is the structure and
the ratios, and the numbers are useful only for deciding which node contains which.

**It survives the budget.** Splitting the roomiest pane repeatedly to sixteen panes, every
subtree's covering rect stayed distinct and every border still matched. The smallest rect
dimension at sixteen panes was 5 cells, so the reconstruction is not near the edge where
two siblings collapse onto one rect and become indistinguishable. Recorded step by step,
because the fixed area above means this could have failed and the failure would have
arrived as a scrambled window at some pane count nobody had tried.

**A divider someone else drags announces itself.** `layout.set_split_ratio` emits
`layout_updated`, and the new ratio is in the snapshot afterwards. A view that follows this
event follows a drag made in herdr's own TUI.

**Zoom is a flag and changes nothing else.** With a pane zoomed, all five panes are still
listed with their ordinary unzoomed rects, and only `zoomed` moves. A client that renders
the rects it is given paints every pane while the daemon is showing one, so honoring the
flag is not optional.

**Closing collapses the parent.** A `pane_closed` is followed by `layout_updated`, and the
split that held the closed pane is gone from the tree rather than left with one side.

**A tree is not evidence about now, and the reason is the replay.** Everything above reads a
tab that has settled. A subscription is handed a sequence, and opening one against a tab that
settled minutes ago replays how it got there: a three-pane tab arrived as one pane, then two,
then three, each as an ordinary `layout_updated` with nothing marking it historical. So a
client applying them in order walks the tab backwards through arrangements it has outgrown,
and one that reads a tree as evidence that a pane is gone moves the keyboard off that pane and
keeps it there - the arrangement arriving next makes the wrong answer valid. The pane list is
the thing that stayed true throughout, which is why Muster's composition takes it as the
authority and the tree as an ordering only (`corpus/conformance/composition.json`, "a tree
that has not caught up does not move the keyboard").

**A split seen by a subscription that is already open publishes one arrangement, not two.**
Splitting a settled two-pane tab broadcast the three-pane tree and nothing before it. Worth
recording because the opposite was assumed from a test log: the shorter-timescale transient
that reading suggested is not a separate behaviour, it is the replay above seen through a
subscription that had just bootstrapped. The rule the core applies is unchanged and still
needed - it is the replay that needs it - but a client does not have to recognise an
intermediate arrangement during a split. The pair in section 14 is the exception and a
different mechanism: two requests, two real arrangements, both true when they were sent.

Evidence: `corpus/herdr-0.8.0/layout/`, recorded with `tools/herdr-probe/probe layout`. The
last two are `corpus/herdr-0.8.0/layout-replay/`, recorded with
`tools/herdr-probe/probe layout-replay` - a scenario of its own because it builds a tab rather
than reading the fixed one above. `bootstrap.events.ndjson` is the replay verbatim.

## 14. There is no splitting leftward, and the pair that builds one is announced twice

`SplitDirection` is `right` and `down`, and a split always puts the new pane on the
`second` side. So the four-way splitting people arrive expecting has no direct request
behind it: leftward is a rightward split followed by `pane.swap` of the pair. The
question is not whether that ends in the right arrangement - it does - but what a client
watching is shown while it happens.

**Both arrangements are published, 100 ms apart.** The pair issued back to back with no
wait between them produced `pane_created`, then a `layout_updated` placing the new pane
on the right, then a second `layout_updated` placing it on the left. Arrival gaps
measured by a watcher rather than read off the daemon: 0.0 ms from the creation to the
first layout, and **108.3 ms** from the first layout to the second. At sixty frames a
second that is six frames of the pane sitting where nobody asked for it, then a jump.

**That hundred milliseconds is the broadcast, not the work.** The same recording times
the requests: `pane.split` answered in 13.8 ms and `pane.swap` in **1.5 ms** after it, so
the pair is finished about 15 ms after it is asked for. What takes another hundred is
herdr getting round to telling a *subscriber* - and the caller does not need telling,
because `PaneSwapResult` carries the whole settled layout, in the same
`PaneLayoutSnapshot` shape `layout_updated` uses. A client that reads its own answer is a
hundred milliseconds ahead of one that waits to be told twice.

So what a viewer sees is not decided by the daemon. Both arrangements still reach the
subscription, and applying them in order is what draws the wrong one; Muster takes the
arrangement from the answer and drops the broadcast it overtook (`mirror/state.rs`,
`settle`). The upstream ask stands and would delete all of it - a four-way
`SplitDirection`, or a request that places a new pane on a named side.

**A swap moves daemon focus to the source pane.** Not to the pane that was focused, and
not to whatever ends up where it was: `focused_pane_id` in the result is the
`source_pane_id` that was asked for. It shows up when the split before it carried
`focus: true`, which Muster's does - the intermediate arrangement names the *new* pane as
focused and the settled one names the pane that was split, so the two layouts differ in
their cursor as well as their shape. Worth knowing because it defeats the obvious way to
recognize the intermediate: comparing whole layouts calls those two different things when
the only difference that matters is the arrangement.

**A swap it cannot do is an error, not a soft no.** `pane.swap` names four refusal
reasons in its result (`no_neighbor`, `same_pane`, `not_found`, `cross_tab`), but a call
naming no target at all raises `invalid_pane_swap` instead. Worth knowing because this
one arrives *after* a split that already succeeded: a caller treating it as a plain
failure has created a pane it never undoes, on the side nobody asked for.

**Its result nests, like the rest of them.** The settled layout comes back under a `swap`
key rather than at the top level, the same shape section 6 recorded for every other
result. Reading `changed` off the top level gets `null`, which reads exactly like a swap
that did nothing.

Evidence: `corpus/herdr-0.8.0/split-sides/`, recorded with
`tools/herdr-probe/probe split-sides` - `wire.ndjson` for the request timings and
`FACTS.json` for the broadcast gap. The focus behaviour was measured live against the
pinned daemon while building the leftward split, 2026-08-15, and is held by
`crates/muster-herdr/tests/split_sides.rs`.

## 15. A tab closes when its last pane goes, and says nothing

Closing or exiting the only pane in a tab removes the tab, and the only event that fires
is about the pane. `tab.list` stops reporting the tab from the same moment, so the daemon
is unambiguous about the outcome - it simply never announces it.

Measured both spellings, since removal has two (section 10). `pane.close` on a tab's only
pane emits `pane_closed` and nothing else; a shell that runs `exit` emits `pane_exited`
and nothing else. In a tab holding two panes, neither removal touches the tab, so this is
about emptiness rather than about closing:

    tabs before          w1:t1, w1:t2, w1:t3
    close w1:p2          → pane_closed          tabs: w1:t1, w1:t3
    exit in w1:p4        → pane_exited          tabs: w1:t1, w1:t3   (t3 still holds p3)
    exit in w1:p3        → pane_exited          tabs: w1:t1

Subscribing to `tab.closed` throughout returns nothing. `workspace.closed` is not the
answer either: the workspace outlives its tabs.

**The consequence for a client is a tab it holds forever.** Nothing on a healthy
connection ever revisits it - only a reconnect re-snapshots - so a mirror waiting to be
told keeps a tab nobody can reach, which in Muster showed up as a sidebar caption over no
rows and a region still pointed at a tab that is gone.

**Inferring it is safe, and this is the measurement that says so.** A tab is never
legitimately empty, because on creation the pane arrives *before* the tab:

    tab.create           → pane_created w1:p3, then tab_created w1:t3, then layout_updated
    pane.split           → pane_created w1:p4, then layout_updated

So there is no moment where a real tab is waiting for its first pane, and "this tab holds
no panes" means only one thing. Muster removes the tab when its last pane goes
(`mirror/state.rs`, `remove_pane`) rather than waiting for an announcement that never
comes. The upstream ask is still worth making - a client should not have to derive a
tab's lifetime from its contents.

Evidence: recorded live against the pinned daemon while fixing the sidebar caption bug,
2026-08-15.

## 16. The two names a pane has, and which of them a client is told about

A pane carries a `label` somebody chose and a `terminal_title` its program set, and
herdr keeps them apart: setting one never touches the other. That separation is what
lets a sidebar row show a durable name and a changing status at once, so it is worth
saying which is which.

    label                    "🔥 payments spike"     set by pane.rename, kept by herdr
    terminal_title           "✢ first working build" whatever the program last wrote
    terminal_title_stripped  "first working build"   the same, minus one activity glyph

Stripping removes a single leading braille spinner or Claude activity glyph when a
space follows it, and answers nothing at all when what is left is empty
(`~/src/herdr/src/terminal/title.rs`). Both title fields are absent until a program
writes one; `label` is absent until somebody sets one, in 1942 recorded pane objects
across this corpus, so "has a name" needs no sentinel.

**A changed title is announced, and a rotating spinner is free.** One OSC 2 write
produced exactly one `pane_updated`, carrying the new title on the event itself rather
than only in an answer to a later question. Five spinner glyphs rotated in front of an
unchanged title produced none, because herdr compares the *stripped* title before it
emits (`~/src/herdr/src/app/terminal_titles.rs:46`). This matters for the budget: a
harness rewriting its spinner several times a second costs a client nothing, and only
the text a person would read reaches the wire. The headless server runs the same sync
loop as the TUI (`~/src/herdr/src/server/headless.rs:574`), so none of this depends on
anybody having herdr's own interface open.

**A rename is not announced at all.** `pane.rename` emitted no event of any kind - not
`pane_updated`, and there is no `pane.renamed` topic in the schema to subscribe to. The
name became visible to a subscriber only when a later title change happened to carry it
along. `pane.rename` does answer with the whole pane payload including the new label, so
a client learns about its own rename from the reply, which is the same shape as a split
answering with its settled layout (section 14). What no client can currently learn is
somebody *else's* rename. Tabs do not have this problem: `tab.rename` emits `tab_renamed`
with the new label.

**A tab cannot be un-named.** `tab.rename` declares `label` a required string where
`pane.rename` declares it nullable, and `Tab::set_custom_name` wraps whatever it is given
in `Some` (`~/src/herdr/src/workspace/tab.rs:204`). Passing `null` is a schema violation
and passing `""` sets an empty name that survives a daemon restart:

    tab.list                 label "1"          herdr's own position number
    tab.rename "release"     label "release"
    tab.rename ""            label ""           and there is no way back to "1"

**The replay is a log of past events, not a statement of the present, and it is drained
after the snapshot.** Subscribing afresh to a session whose pane was both named and titled
replayed a `pane_created` holding `revision: 0`, `agent_status: "unknown"`, and no `label`
or `terminal_title` key at all - the payload as of when the pane was made. A client that
snapshots on connect therefore applies the snapshot first and the history afterwards, which
is backwards, and this goes wrong in two ways rather than one.

Absent fields are the first. A `pane_created` may not clear a name or a title already held,
because absent there means "not recorded" rather than "not set" - the same fence
`agent_status` already sits behind (section 10, and the event model in `architecture.md`).

Stale values are the second, and they need a different answer per field, because **herdr
orders the title and does not order the name**. `revision` counts changed *stripped* titles
and nothing else: not an agent changing state (section 10), and measured here, not a rename
either - renaming a pane twice left it where it was. So an older payload can be recognised
and dropped for the title, and cannot be for the name. With no ordering and no announcement,
a `label` on an event carries no information a snapshot does not carry better, and the only
safe rule is that a structure event never writes the name of a pane already held. The cost
is one round trip of latency on somebody else's rename, which was nearly the situation
anyway.

**A restart keeps the name and loses the title**, which is the asymmetry the whole
feature rests on. After `server.stop` and a fresh daemon, `label` came back on the pane
and `terminal_title` was null: herdr wrote the name down, and the process that would set
a title again is new. A name is durable identity, a title is volatile status.

## 17. A pane's history reads back as rows, and stops at a thousand of them

Recorded in `corpus/herdr-0.8.0/read-depth/`, driving an 80x24 pane through three
thousand numbered rows. Two questions, both of which decide whether a client can offer
find-in-pane at all: how much of the history a client can get at, and whether what comes
back can be turned into a position to scroll to.

**A read's lines are grid rows, exactly.** `pane.read --source recent` ends with the rows
`--source visible` returns, character for character, and scrolling up a hundred rows put
exactly the hundred-rows-earlier slice of that same read on screen. So the *n*th line
from the end of a read is `offset_from_bottom` *n*, with no fudge factor - which makes
landing on a match arithmetic rather than a search of its own.

    recent (last 1000 rows)   … ruler-02978 … ruler-03000
    visible                     ruler-02978 … ruler-03000     the read's own tail
    scroll up 100 → visible     ruler-02878 … ruler-02900     recent[-124:-100]

**Relative scroll is exact and there is no absolute one.** Asking `terminal.scroll` for a
hundred rows up moved `offset_from_bottom` to exactly 100. That is the whole mechanism
available: `pane.scroll` does not exist as a method, so a client that knows which row it
wants must read the current offset and scroll by the difference. Two round trips on two
channels, and wrong if the pane prints in between.

**The read stops at a thousand rows, and asking for more is not an error.** `lines` is
clamped rather than refused - `read_terminal_snapshot` does `lines.min(1000)`
(`~/src/herdr/src/app/api_helpers.rs:120`) - so 1001, 2000 and 100000 all returned 1000
rows of a pane reporting `max_offset_from_bottom: 2978`. Two thirds of that pane's
history is unreachable through this API, and nothing in the reply distinguishes "you
asked for more than I give" from "that is all there is".

    lines: 80 → 80      lines: 999  → 999      lines: 2000   → 1000
    lines: 500 → 500    lines: 1000 → 1000     lines: 100000 → 1000

**`truncated` answers a different question than the cap, and it is the useful one.** It
was `true` for every read of this pane including `lines: 80`, so it means "there is more
history than this" rather than "you hit the ceiling" - which is exactly what a client
needs to say honestly what it searched, and is not a way to detect the clamp.

**There is no paging, and asking for it succeeds.** `PaneReadParams` has no offset or
cursor (`~/src/herdr/src/api/schema/panes.rs:252-263`), and an `offset` key sent anyway
came back a normal success carrying the identical thousand rows - herdr ignores
parameters it does not know (section 6). A client that assumed the key worked would page
through the same screenful forever, with nothing anywhere reporting a problem.

**Nothing on the API searches.** No `pane.search` and no `pane.find`; the refusal for an
unknown method enumerates every method herdr has, so this is a complete answer rather
than three guesses. `pane.read` and the `pane.output_matched` event are the entire
surface area for looking at what a pane has printed.

**The daemon is not short of the data.** herdr's own `snapshot_history` reads
`recent_unwrapped_ansi(usize::MAX)` (`~/src/herdr/src/pane.rs:2651-2654`) when it writes
a pane down for durability. The whole history is there; only the API declines to hand it
over.

**Choosing a source costs something either way, and a long line is where it shows.** A
167-character line came back from `recent` as the rows it occupies - 80, 29, 80, 80, 7,
counting the echoed command that wrapped too - and from `recent_unwrapped` as one line of
167. So `recent` is the source whose positions are computable and which cannot match a
needle spanning a wrap, and `recent_unwrapped` is the reverse. There is no source that is
both, and a client that read both would have two answers to how many matches there are.

**A read's last row is the pane's last printed row, not its bottom row**, and the two are
only the same on a full screen - which is the screen the alignment above was measured on.
Added 2026-09-03. herdr trims the blank remainder of the viewport off the bottom of what
it returns: a pane holding 3009 rows whose screen had just been erased and given two lines
answered a 1000-row read with 978, and 2 of its 24 viewport rows were printed. So "the nth
line from the end is `offset_from_bottom` n" needs the trimmed rows added back, and their
count is exact rather than a guess - the read's window minus what came back, where the
window is the pane's whole height for an untruncated read and a thousand for a truncated
one.

    pane holds 3009 rows, viewport 24, screen just erased and given two lines
    read 1000 -> 978 rows          visible -> 2 rows of 24        22 trimmed either way

**A pane on the alternate screen holds nothing behind its screen, and says so like a pane
that has no history.** `max_offset_from_bottom` drops to 0 the moment a program enables
mode 1049, `recent` returns the visible rows, and `truncated` is `false` - the same answer
a fresh pane gives. So a client searching an agent pane covers one screen and, taken at
face value, reports a complete search of it. The main screen's history is intact and comes
back when the program leaves. `ESC[3J` - which `clear` sends and plenty of programs send
on their own - leaves the identical shape without any full-screen program involved.

    main screen, 3009 rows    max_offset_from_bottom 2985    recent -> 978 rows
    alternate screen          max_offset_from_bottom 0       recent -> 4 rows, truncated false
    back on the main screen   history intact

This is the one worth knowing before building find on `pane.read`: every agent harness is
a full-screen program, so it is the shape most panes in a Muster window are in.

Evidence: `corpus/herdr-0.8.0/read-depth/` - `FACTS.json` for the verdicts, `wire.ndjson`
for the requests and answers, `recent-tail.txt` and `visible-at-bottom.txt` for the rows
the alignment is read off. Re-record with `tools/herdr-probe/probe read-depth`.

## 18. Waiting for a pane to print needs the connection left open

`pane.wait_for_output` is the only call herdr answers slowly on purpose, and it is the
only one that cares how the caller holds the socket. Muster's client sent its request line
and then half-closed the write side, on the belief that the daemon waits for end-of-write
before answering. It does not - it reads exactly one line - and given a half-closed socket
this call reads the caller as gone and hangs up without answering.

    ping, session.snapshot, pane.read, pane.split     answered either way
    pane.wait_for_output, write side half-closed      nothing, in under 1 ms
    pane.wait_for_output, write side left open        output_matched, as documented

The failure is the shape of a success. A hang-up is indistinguishable from a read timeout
at the socket, so Muster saw `TimedOut` roughly a millisecond after asking for a
five-second wait - which reads as "the daemon is wedged" rather than "this call needs a
connection nobody has closed".

**A timeout is a clean refusal.** Asking for a pattern nothing prints returned
`code: "timeout"`, `message: "timed out waiting for output match"`, after the
`timeout_ms` asked for. That is what lets a caller tell "nothing printed" from "the daemon
went away", and the two want opposite responses: send anyway, or stop.

**`match` is a substring or a regex, and there is no "anything at all".** So "the shell
has drawn its prompt" is spelled as the regex `\S` against `source: visible`. A prompt
configured to draw nothing visible is therefore a timeout rather than a match, which is a
real configuration and the reason the timeout has to be survivable.

**There is no way to spawn a command with a pane.** `pane.split` takes `cwd`, `direction`,
`env`, `focus`, `ratio`, `target_pane_id` and `workspace_id` - no command, and neither do
`tab.create` or `workspace.create`. A pane runs the daemon's `default_shell` and nothing
else, so "make a pane running this" is necessarily a split, a wait for the prompt, and
then the text - three calls, and the wait is the only thing standing between the text and
a program that has not finished starting.

**What the wait is worth, measured rather than assumed.** A pty buffers, so a plain `sh`
handed input before its prompt appears runs it anyway - which is why removing the wait broke
nothing for a release. What loses the text is a program that takes the terminal in hand as it
starts: `tcsetattr` with `TCSAFLUSH` discards input that arrived and has not been read, and
that is the first thing a full-screen agent harness does. Two panes running such a program,
one equipped by Muster and one handed the same text with nothing waiting first, split cleanly:
the first gets its command and the second loses it, and the second gets it on a second attempt
once the program is reading. `crates/muster-herdr/tests/pane_equipping.rs`.

**The readiness signal is satisfied by the terminal's own echo.** Muster waits for `\S` on the
visible screen, and a terminal echoes what is typed into it - so text sent into a pane puts
non-space there before the program has run a line. Harmless where Muster uses it, since a pane
it has just made has had nothing typed into it, and worth knowing for anything else that waits
this way on a pane already in use.

Pinned by `crates/muster-herdr/tests/client_connection.rs`, which asserts both directions:
the ordinary calls answer with the write side half-closed, and `pane.wait_for_output` does
not. Nothing else in the suite would notice a half-close coming back.

## 19. A resize amount is a share of what the divider divides, and half is as far as one goes

**`pane.resize`'s `amount` is a fraction, added to a split's ratio as it stands.** herdr
documents this nowhere - `PaneResizeParams` (`~/src/herdr/src/api/schema/panes.rs:203-209`)
carries no doc comment, and the only hint upstream is that its own CLI examples pass
`--amount 0.1`. Measured against the pinned daemon, on a fresh two-pane tab sitting at 0.5:

    amount   0.05    0.25    0.5     1.0     10.0
    ratio    0.55    0.75    0.9     0.9     0.9

So a request for 0.05 moves the divider by exactly five percent of the region, and the
arithmetic is `current_ratio + amount` with no scaling in between
(`~/src/herdr/src/layout.rs:235-237`).

**Two ceilings, and they compound.** The amount is clamped to `.abs().min(0.5)` before it is
applied (`~/src/herdr/src/app/api/panes.rs:404-409`), and the ratio it produces is then
clamped to `0.1..0.9` (`layout.rs:209-211`). Half is therefore as far as any single request
travels, and every amount at or above it is indistinguishable - which is why the row above
flattens at 0.9 rather than continuing.

**Which divider moves is the nearest split on the axis.** `resize_focused` picks it by edge
distance from the focused pane's rect, preferring the requested side and falling back to the
other (`layout.rs:213-238`, `nearest_resize_split` at 356-366). The ratio is that split
node's, so it is a share of the whole region only when there is one split on that axis -
which is what bounds how exact a distance in points can be made without Muster holding a
tree of its own.

**Why this was worth measuring rather than reading.** Muster sent this field a count of cells
for a release. Every step a person could write - `"1c"`, `"1px"`, `"10c"` - arrived at or
above 1.0 and landed on 0.9, so the config key had exactly one behaviour and omitting it, at
herdr's own 0.05, was the only usable setting. Nothing compared the two sides: the intent's
doc comment said cells, the conformance case for it named the field `ratio`, and neither was
ever put next to a daemon.

Evidence: `a_resize_amount_is_a_share_of_the_region_and_saturates_above_a_half` in
`crates/muster-herdr/tests/split_sides.rs`, which is the table above, re-derived against
`deps/herdr.pin` on every run of `./dev -t`.

## 20. A pane moved between tabs is announced only to whoever asked for that event by name

**`pane_moved` does not arrive unless it is subscribed to by name.** A client that
subscribed to all sixteen of the other structural kinds, and drove a real `pane.move`,
saw one event: `layout_updated`. Subscribing to `pane.moved` as well, the same move
announced five - `layout_updated`, `pane_moved`, `pane_focused`, `tab_focused`,
`workspace_focused`. This matters more than it reads: Muster's own subscription list did
not name it, so the event it needed most was one no run had ever received, and the
decoder's unknown-kind warning could never have fired for it either. A kind nobody
subscribes to leaves no evidence at all.

**The payload carries the whole pane, already stating the tab it landed in.** Keys are
`pane`, `previous_pane_id`, `previous_tab_id`, `previous_workspace_id`, `type`, and the
nested `pane` is the same shape `pane_created` and `pane_updated` carry - so it reads as
an upsert rather than needing a verb of its own.

**Nothing else states the move.** No `pane_updated` follows one: measured zero across the
whole exchange. And `pane.move`'s answer, which is the other route a client could learn
from, carries its trees under `source_layout` and `target_layout` - where `pane.swap`
answers with a key named plainly `layout`. Muster's adapter looks one level down for
exactly `layout`, so it settles a swap from its own answer and settles nothing from a
move. Both trees are sitting in that reply, under names nothing reads.

**Why that combination is worse than either half.** A tab's tree is withheld while the
panes it names disagree with the panes the mirror holds, which is the right behaviour when
a tree is momentarily ahead of its panes. With the move unread, the disagreement is
permanent rather than momentary: the pane's tab is never updated, so both the tab it left
and the tab it joined stop redrawing rather than showing the move. Muster is what causes
these events - a row dropped on a row in another tab - so this was reachable from the
product's own drag-to-swap.

**A swap within one tab is the control.** It announces `layout_updated` alone, changes no
pane's tab, and answers with a settled `layout` the caller can apply directly. So the tree
is enough for a swap and is not enough for a move, and the difference is exactly the tab.

Evidence: `corpus/herdr-0.8.0/arranging/` - `FACTS.json` for the event kinds, payload keys
and answer shapes, `wire.ndjson` for the requests and their replies, `events.ndjson` for
what each subscriber saw. Re-record with `tools/herdr-probe/probe arranging`.

## 21. A tab moved is announced to nobody but whoever named `tab.moved`, and to them it states the whole new order

**A tab reordered announces exactly one event, and nothing else says it happened.** A client
subscribed to every one of the twenty-four structural kinds saw a single event for a real
`tab.move`: `tab_moved`, with no `layout_updated` and no `tab_focused` beside it. Drop
`tab.moved` from that list and a further move announces **nothing at all**, while the order
changes anyway: the list stood at `w1:t3, w1:t1, w1:t2` and came out `w1:t1, w1:t3, w1:t2`,
with the subscriber told none of it. That is a sharper version of section 20's finding rather
than a repeat of it. A pane carried between tabs at least announced `layout_updated` to an unsubscribed
client, so something arrived and was merely insufficient; a tab moved reaches such a client as
silence, and Muster is exactly that client - `tab.moved` is not in its subscription list, so
no run has ever received one and the decoder's unknown-kind warning could not have fired for
it either.

**The payload states the whole new order, absolutely.** Keys are `insert_index`, `tab_id`,
`tabs`, `type`, `workspace_id`, and `tabs` is an array of the same `TabInfo` a snapshot
carries - in the order they now sit. So there is nothing to compute: a client applies the
sequence it is given, which is what the event model asks for (absolute values, never deltas).
The array is that workspace's tabs rather than the session's; the recording had one workspace,
and herdr builds it from `tab_list_info(ws_idx)` at `src/app/api/tabs.rs:206`.

**A tab keeps its id across a move.** Measured directly: the same three ids before and after,
in a different order. Worth stating because herdr's `tab_id` *looks* positional - `w1:t3` - and
if it were, this would be three renames rather than one reorder and no mirror keyed by id could
follow it. The id is built from a number stored on the tab, and moving the tab carries the
number with it.

**A tab's label is positional, and this event is the only thing that renumbers it.** An unnamed
tab's label is its place, so moving one relabels every unnamed tab in the workspace: `w1:t1`
went from `1` to `2` and `w1:t3` from `3` to `1`, while `w1:t2` kept the name `second` somebody
had given it. Two consequences, and they point in opposite directions. Anything drawing herdr's
labels has no way to keep them current without reading this event. And anything that reads the
event must not take the labels from it: a tab's label has one writer, `tab_renamed`, precisely
because a replayed creation label puts a number back over somebody's name (section 16), and
these are the same numbers. Muster discards an all-digits label and draws its own place, so for
Muster this is a field to ignore rather than a second thing to fix.

**A move that moves nothing announces nothing, and still answers with the list.** Asked to put
a tab at the index it already held, herdr sent no event at all and answered `tab_list` as
usual. So silence is safe to read as "the order did not change", and a caller cannot tell a
declined move from a performed one by looking at its answer - which is the shape of
`a_29e9bxN9r` in a place nobody was looking for it.

**The answer carries the settled order.** `tab.move` answers `tab_list { tabs }`, so a caller
is a round trip ahead of its own subscription the way it is for a swap (section 14) - unlike a
pane move, whose trees arrive under names nothing reads (section 20). Nothing in Muster reorders
tabs today, so this is a fact held in reserve rather than one being acted on.

Evidence: `corpus/herdr-0.8.0/arranging/` - `FACTS.json` for the event kinds, the order before
and after, the payload and the label renumbering, `wire.ndjson` for the requests and their
replies, `events.ndjson` for what each subscriber saw. Re-record with
`tools/herdr-probe/probe arranging`.

## 22. A replay closes a pane before it creates it

**A subscription's replay states past events in an order that is not the order they
happened in.** Measured 2026-08-30 against herdr 0.8.0 on macOS/arm64. A workspace was
created, a pane was split off it, and that pane was closed, all before any subscription
existed. `pane.list` then reported one pane. Subscribing produced this, in this order:

```
pane_created   w1:p1
pane_closed    w1:p2
pane_focused   w1:p1
layout_updated
pane_created   w1:p2
layout_updated
```

`w1:p2` is closed two lines before it is created. A client that applies these in the order
they arrive ends up holding a pane the session does not have, and nothing later corrects it:
the removal arrived first, found nothing to remove, and there is no second one.

**This is what "subscribe first, snapshot second" does not survive.** Muster subscribes
before it snapshots so that no event can fall into the gap between the two, and relies on
upsert to absorb the overlap (`muster-herdr/src/subscription.rs`). Upsert does absorb a
replayed pane that still exists, which is the case section 1 recorded. It cannot absorb one
that does not, because the event carrying a creation looks the same whether or not that
creation has since been undone.

**Unguarded, the pane is drawn until something unrelated asks for a fresh snapshot.** From a
real window on 2026-08-30: the daemon held six panes, the sidebar listed ten, and it stayed
at ten for forty-one consecutive publishes. What ended it was the user clicking one of the
four phantom rows, which herdr refused with `pane_not_found: pane w6:p39 not found`, and a
refusal is one of the few things that re-snapshots. A ghost is worse than a stale row: it
takes a place in the numbered chords, so ⌘1 to ⌘9 name the wrong panes while it is there.

**What Muster does about it.** A removal for a pane the mirror does not hold is remembered
instead of discarded, and a creation for a pane already said to be gone is refused
(`muster-core/src/mirror/state.rs`). A snapshot clears that memory, being the census it
stands in for. Both orders then converge: close-then-create is refused, and create-then-close
removes the pane the ordinary way.

Evidence: not in `corpus/herdr-0.8.0/`, unlike every section above. The finding came from a
hand-run probe against a scratch daemon rather than from `tools/herdr-probe/probe`, and what
pins it instead is a test: `a_pane_closed_before_the_subscription_does_not_come_back` in
`crates/muster-herdr/tests/subscription.rs`, which drives a real daemon and fails without the
guard. Worth folding into the probe next time it is re-recorded.

## 23. A client whose terminal is taken sees its stream end

**A `--takeover` ends the displaced client's stream rather than leaving it reading.**
Measured 2026-09-02 against herdr 0.8.0 on macOS/arm64. One client held a pane through
`herdr terminal session control <pane>` and was painting frames; a second attached to the
same pane with `--takeover`. The first client's stdout reached end of file, three runs out
of three, 258 to 259 ms after the second client was *spawned* - and most of that is a process
starting rather than herdr deciding.

**This is the cheap answer to a question two cards were waiting on.** Only one client may hold
a herdr terminal, which is why a second Muster window opening onto a pane the first is showing
gets `already has an attached client` and renders nothing (`a_2IZ5TL6DQ`). What decides how
expensive it would be to *hand* a pane from one window to another is whether the window that
loses it finds out: a stream that ends is an event the losing window already knows how to
react to - a bridge exiting is the same shape as a bridge whose connection dropped - where a
stream that went quiet would mean rendering what was last painted with nothing saying so, and
a channel of its own to announce the move.

**What it does not make cheap.** Two windows showing one pane *at once* is a different thing
and this says nothing about it: the terminal still has one holder, so that needs a relay
feeding several surfaces rather than a handoff (`a_2IZ6Of6JP`). And a displaced window's
bridge exiting is indistinguishable, from the near side, from one whose pane closed or whose
route dropped - the respawn policy already has to tell those apart, and a handoff would arrive
as a fourth case on the same signal.

Evidence: `a_client_that_loses_its_terminal_sees_its_stream_end` in
`crates/muster-herdr/tests/one_client_per_terminal.rs`, which drives a real daemon and turns
red if a later herdr stops telling the client it displaced. Not in `corpus/herdr-0.8.0/`: the
frame stream is not in the JSON API, so `tools/herdr-probe/probe` has nothing to record it
with.

## 24. A closing stream says which of four things happened, in prose

Recorded 2026-09-01, later than the rest of this file and by hand rather than through the
probe. What prompted it: a client that is refused a terminal and a client that has one taken
away are the same event to Muster - a bridge that stopped - and they call for opposite
answers, so the question was whether herdr says which.

It does. A `terminal.closed` frame carries a `reason` string, and the four Muster can reach
are distinct:

| What happened | `reason` |
|---|---|
| The terminal already had a client | `terminal attach failed: terminal <id> already has an attached client; retry with --takeover` |
| Another client attached with `--takeover` | `terminal attach taken over` |
| This client let go - its stdin reached EOF | `detached` |
| The stream ended with no closing frame at all | none; the bridge supplies its own |

A string and no code, so anything reading these is matching prose and will stop matching when
herdr rewords one. `muster-herdr/src/bridge_report.rs` is the one place that does, an
unrecognised reason falls to "the connection was lost", and
`corpus/conformance/bridge-report.json` fails when a re-pin moves the wording.

**Why the difference is worth having.** Both refusals mean somebody else holds the terminal,
and only one of them means somebody else *wants* it. A refused attach is almost always a
client whose transport died and which has not noticed (section 4, and kan a_2I76eCrjw), so
attaching again with `--takeover` is recovery. A takeover is another live window that just
asked for this pane, so attaching again would be answered the same way by the window on the
other side, and one terminal would be traded back and forth until both gave up.

Evidence: `corpus/herdr-0.8.0/closing-reasons/`, one line per case, recorded against a scratch
daemon with three `terminal session control` clients.
