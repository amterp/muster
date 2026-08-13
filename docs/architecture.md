# Architecture

How Muster is shaped, and why. This document constrains the load-bearing structure: layers, seams, ownership of
truth, and how traffic flows. It deliberately does not choose module layouts, type names, wire framings, or
concurrency mechanisms - implementing agents research those at build time and will make better local decisions than
this document could. When a decision here proves wrong in practice, change it: update this document in the same
change, and put the why in the commit.

The desiderata in `README.md` are the requirements. `origin.md` holds the founding history and `glossary.md` the
terms; large decisions are recorded as MIPs in `mip/`, routine rationale lives in commit messages, and open
questions live on the kan board. This document is the bridge between the desiderata and code.

## The shape

    ┌─ native shell (per-OS; macOS first) ────────────────────────────────┐
    │  windows · split chrome · key capture · sidebar · notifications     │
    │  renderer surfaces (libghostty), one per visible pane               │
    └──────┬───────────────────────────────────────────────▲──────────────┘
           │ key/mouse/scroll/resize events, actions       │ pane frames
    ┌──────▼───────────────────────────────┐               │ (data plane:
    │  core (headless view-model)          │               │  output only,
    │  mirror · dispatcher · keymap ·      │               │  one channel
    │  attention · config                  │               │  per visible
    └──────┬───────────────────────────────┘               │  pane)
           │ Muster vocabulary (control plane:             │
           │ events, state, intents, input)                │
    ┌──────▼───────────────────────────────────────────────┴──────────────┐
    │  backend adapters (herdr today)                                     │
    └──────▲──────────────────────────────▲───────────────────────────────┘
           │                              │
     herdr daemon (local)          herdr daemon (remote, SSH)

Three layers, two seams.

**Native shell** (per-OS; macOS first). Windows, split chrome, key capture, sidebar, notifications, and one renderer
surface per visible pane. Deliberately thin: it wires OS events into the core and renders what the core says,
nothing more. Its failure modes should be wiring failures - that is what makes it testable by a small smoke layer
(see `testing.md`).

**Core** (headless, OS-free, Rust). The view-model and the only place decisions live: the mirror of each daemon's
state, the action dispatcher, keymap policy, attention routing, configuration. The core never touches an OS API, a
real clock, or a socket directly - those arrive through injected edges.

Rust rather than the shell's language, decided in `mip/0001-portable-core.md`. The short version: a boundary
between shell and core has to exist the moment a second platform appears, because a Linux or Windows shell will not
be Swift either. Putting a portable core on one side of it means macOS pays a well-supported FFI direction and every
other platform pays nothing.

**Backend adapters** (herdr today). Translate the Muster vocabulary to a concrete backend. One adapter per backend;
nothing herdr-shaped escapes it. Control-plane transports: local socket for local daemons, SSH for remote ones.

## The vocabulary

The backend contract speaks Muster's terms, not any backend's. Nouns: backend session (one daemon connection),
workspace, tab (the unit that owns one pane tree - trees hang off tabs, not workspaces), pane, pane channel (the
output stream feeding a surface), agent state. Verbs, as intents: attach, split, close, focus, resize, send input,
scroll, spawn. Small on purpose - everything the view needs, nothing any particular backend happens to offer. The
contract corpus at this seam is the executable form of this vocabulary and the definition any replacement backend
(fork or wholesale) must satisfy.

Agent states are working / blocked / idle / done / **unknown** - five, not four; unknown renders as itself, never as
success. State is daemon truth, but one of the five is computed from a client-side input, so the vocabulary has to
carry that input.

`done` is not stored: it is `idle` on a pane that has not been *seen*, and seen-ness is written only when an agent
completes - a working or blocked pane going idle. At that moment the daemon asks two questions: is the pane's **tab**
the active one, and does the foreground client's **window** have OS focus. Pane focus is not consulted. So Muster
feeds seen-ness by driving the daemon's active tab, which `pane.focus` does as a side effect, and by reporting
window focus - and the second has no method on herdr's JSON API today (see `observations/herdr-0.8.0.md`, and the
kan card tracking the upstream ask). Until it does, a Muster window that loses OS focus while its active tab holds a
running agent will mark that agent seen when nobody saw it.

## Control plane, data plane

Two kinds of traffic, opposite needs, different paths. The split is between *output* and *everything else* - not
between "bytes" and "control":

- **Output rides the data plane.** Each visible pane has its own channel from adapter to surface, bypassing the
  core. With herdr the channel carries server-rendered frame diffs of the pane's screen - not the raw program
  output - so the daemon's render cost scales with *visible* panes: hidden panes detach their channels, and
  revealing a pane costs one full repaint. Detaching costs no state: herdr analyzes a pane's screen whether or not
  anyone is watching it, so a hidden pane keeps reporting its agent state. The core never sits in this path;
  per-byte work in the core is a defect (desiderata: fast is a feature).
- **Everything else rides the control plane, through the core** - daemon events (structure, agent states, bells,
  titles), intents, configuration, and *input*. Input is the awkward one, because nobody in this picture is in a
  position to encode it well. Key encoding needs the pane's terminal modes (kitty keyboard, bracketed paste,
  application cursor keys); those modes live in the daemon's VT, they are not replayed in the frame stream, and
  herdr exposes none of them on its API. The daemon does not encode either: `terminal.input` on a control stream
  is a raw write to the pane's PTY (`observations/herdr-0.8.0.md` section 5). So Muster encodes, blind. The shell
  reports key, mouse and text events with full fidelity, the core routes them, and the core encodes with
  libghostty-vt - the same engine the daemon's own VT runs - against a **declared mode profile** standing in for
  state we cannot read. That profile is the one place the guess lives, and the seam that gets fed from truth the
  day herdr publishes its `InputState`. herdr's named-key API is not an option: it has no navigation cluster.
- **The control plane is not one connection.** herdr answers a request and closes the socket, so each intent costs a
  connect; only subscriptions are long-lived. Nothing may assume a persistent request/response channel, and the
  per-intent connect is a cost the perf budget has to carry.
- **Scroll belongs to the daemon.** A frame stream has no history, so surfaces hold no scrollback and never handle
  the wheel: scroll is an intent, answered by the daemon repainting the viewport - or, when the pane's program is
  reporting mouse, by the daemon encoding a wheel event for it. `terminal.scroll` is the one input-shaped thing
  herdr answers against a pane's real modes, and it is the shape the rest of input should eventually take. Mouse
  buttons and motion have no such command, and Muster does not send them: an SGR click encoded blind is garbage
  on the program's stdin, where a mis-encoded key is merely a wrong key.

## The shell/core seam

The shell and the core are different languages in one process, so the boundary between them is real and has to be
narrow. It is one C ABI symbol - `muster_dispatch(request_bytes) -> response_bytes` - carrying protobuf-encoded
messages in the vocabulary above, plus a callback the shell registers so the core can wake it unasked (an agent
changed state, a notification is due). `include/muster.h` is the whole contract and is hand-written, because a shell
on another platform implements against it and should be able to read it without building anything. Details and
alternatives in `mip/0001-portable-core.md`.

Two properties keep this from being a bottleneck or a maintenance tax. **It carries events, never bytes**: the data
plane runs adapter to surface and never enters the core, so this seam sees keystrokes and daemon events, not output.
And **the schema is generated on both sides** from `proto/muster.proto` and committed on neither, so a shell and a
core that disagree is not a state the repo can hold.

The core answers every request, including the ones it refuses. A shell cannot otherwise tell "the core said no" from
"the core is gone", and those want opposite reactions - so a refusal is a `Failure` carrying prose written for
whoever finds it in a log, not an error code to branch on.

Backpressure has no design yet, and the starting answer is a property of this architecture rather than a mechanism:
because view = f(daemon state), a queued update can be **coalesced** rather than dropped or blocked on. That is what
lets this seam afford a bounded queue when there is finally state worth queueing.

The same schema is the CLI and the agent-facing API. "One action path" stops being a discipline and becomes
codegen - a surface that cannot express an action is a missing message, visible at build time.

## Ownership of truth

- **Daemons own structure**: workspaces, tabs, pane trees, panes, scrollback, agent states, process lifetimes.
  View = f(daemon state).
- **The core owns a mirror**: a derived, disposable cache of daemon structure, bootstrapped from an authoritative
  snapshot plus event subscription, rebuilt after any gap, never patched across one.
- **The core owns composition**: which daemons are attached, and which (daemon, workspace, tab) shows in which
  window region. Mixing is at tab granularity: a region displays one tab's pane tree, rendered from daemon truth;
  regions from different daemons sit side by side. Muster does not own an outer split tree over panes - that would
  make it a multiplexer (non-goal) - and a pane can never move between daemons: the process lives where it lives.
- **Cursors are written, not read.** Daemon focus (focused workspace, tab, pane) is a single value per daemon,
  shared with every client including the herdr TUI. Muster's input routing - which pane its keyboard feeds - is
  view-local. Interacting writes daemon focus (which also feeds seen-ness); Muster never *routes* input by reading
  it, so another client moving daemon focus never yanks Muster's keyboard.
- **Geometry follows the controller.** Pane cell dimensions are daemon truth; the shell converts pixels to cells
  and sends resize intents. While Muster controls a pane, the pane's PTY is held at Muster's geometry. Other clients
  are not dragged to that size - the daemon re-renders the screen into each viewer's own requested viewport - so
  concurrent TUI viewing is degraded by seeing a larger screen reflowed into a smaller window, not by resizing.
  The hold does **not** release when Muster detaches: a pane keeps the last geometry its controller set. Leaving a
  user's panes sized to a window that no longer exists is the sharp edge of "sessions outlive everything", and
  restoring geometry on detach is Muster's job until herdr does it.
- **The shell owns nothing.** Surfaces are disposable renders of a pane channel. A surface attaching to a live pane
  starts with a full repaint and never assumes it saw the start of the stream. Closing a window destroys surfaces
  and touches no session.

## Event model

State changes only by applying events and intents in one place, in one order per daemon connection; rendering reads
the result. Pane content is not state in this sense - it is a stream the surface renders. Two constraints carry the
model:

- **Application is convergent.** herdr offers no event replay, and subscribing replays the current session as
  synthetic events, so a client sees every existing entity twice and cannot ask for what it missed. Every event is
  therefore applied idempotently and carries absolute values, never deltas, so snapshot-plus-events converges
  regardless of what raced the bootstrap.
- **Gaps are detectable for agent state, and not for structure.** Measured rather than assumed
  (`observations/herdr-0.8.0.md` section 10). An agent's `state_change_seq` is stamped from one session-wide
  counter, so a client that remembers the highest value it has seen can tell that transitions happened while it was
  not listening, including on panes it has never heard of. A pane's `revision` cannot be used this way: it tracks
  terminal titles and metadata tokens, and does not move when an agent changes state. Nothing reports a pane created
  and closed inside a gap, so structure has no evidence-based detector and periodic reconciliation against a fresh
  snapshot is the only one. Cadence is a separate decision.
- **Removal has two spellings.** A pane a client closes emits `pane_closed`; a pane whose program ends emits
  `pane_exited` and no `pane_closed`. Both must drop the pane, or an exited pane renders forever.
- **Cross-daemon order is core order.** Streams from different daemons have no mutual order. Composition and
  attention are ordered by the core's own application sequence, and nothing may depend on cross-daemon event order
  for correctness.

Rendering is driven by diffs scoped to what changed: an agent-state change costs that change, not a walk of every
pane (desiderata: fast is a feature, the per-event half).

## Attention routing

Attention is computed in the core from control-plane events - agent-state transitions, bells, title changes, output
activity all arrive there, so the data-plane bypass costs nothing here. The core owns the unread and urgency
ordering; the shell only delivers notifications and renders indicators. Activating a notification dispatches an
ordinary focus intent through the one action path - which may change composition first, because the pane that asked
may not be visible in any window. Surfacing the hidden is part of the feature, and the core owns it.

## Input precedence

A keystroke resolves in fixed order: first the Muster keymap - if the chord is bound to an action, dispatch it and
stop; otherwise it is reported, with full fidelity, toward the focused pane via the control plane. The wheel is the
standing exception: scroll always becomes an intent (see data plane). The keymap is data in the config file, not
code.

## One action path

Every operation - keybinding, menu, CLI, socket API - dispatches the same action into the same core dispatcher;
invocation surfaces carry no logic of their own (desiderata: parity by construction). The Muster CLI talks to the
running app over a local IPC endpoint and covers view-layer operations; backend-level operations remain the backend
CLI's job (herdr already has a good one).

## The renderer seam

The renderer gets the same treatment as the backend: a narrow contract in Muster's terms - create a surface in a
region, run a pane channel into it, resize it, read its grid (the test oracle) - and nothing libghostty-shaped
escapes the seam. Today the only way to feed an embedded ghostty surface is the command it spawns; the embedding
header has no byte-feed API. The pane channel is therefore delivered by a bridge subprocess the surface runs. That
is a fact about current libghostty, not a choice - re-verify on upgrades, and revisit if upstream grows a direct
feed.

## Degradation

Health is per-connection *and* per-channel, and it is state, not an error path:

- **connected**: live control plane, live pane channels.
- **stale**: the control plane is silent or wedged (SSH up, daemon unresponsive), or a pane channel dropped. Render
  the last mirror and last frames, marked stale. Pane-channel recovery is a forced full repaint; control-plane
  recovery is a fresh snapshot. The two recover independently.
- **disconnected**: render the labeled last mirror; reconnect resyncs everything.

Liveness needs an active probe - the control plane is legitimately silent when nothing happens - and how it probes
is an implementation choice. Version skew between Muster and a daemon is detected at attach and surfaced plainly.
Sessions survive anything Muster does: a broken Muster must never strand a session (see also geometry, above).

## The diagnostic log

One run, one file, every process. The app names it, and every bridge it spawns inherits the path, so a keystroke
leaving the app and arriving at a daemon reads as consecutive lines rather than as a correlation exercise across
clocks. Records are one JSON object per line - time, level, process, pid, a dotted event name, then fields - which
makes the log greppable by hand and an assertion surface for tests: a launch smoke test can assert that
`channel.connected` appeared and that no `error` record did.

Events are named for the question they answer, not for the code that emitted them. The load-bearing ones are the
boundaries where a process can silently stop mattering: the control socket binding, a bridge dialing back, the first
frame painted, and the reason a pane's stream ended.

On by default in debug builds and opt-in in release, because a terminal multiplexer's logs are unusually sensitive.
What the user typed is recorded only under a separate switch again: by default a keystroke record carries its shape -
which key, how many bytes - and not its content. The default must stay the one that cannot leak a password into a
file destined for a bug report.

Where the file lives is an OS question and therefore the shell's; nothing in the core knows the path.

## Seams and test hooks

The injected edges, matching `testing.md`: the clock, and the renderer seam (tests feed pane channels through
libghostty-vt and assert the resulting grid). The backend connection is deliberately *not* one of them - tests
spawn a real, version-pinned herdr rather than a stand-in, so the adapter is judged against the daemon itself.
What is injectable there is narrower and lives in the code's own shape: the event parser takes a reader rather
than a socket, so a recorded stream can be cut anywhere, and the connection loop takes a socket path, so a killed
daemon is the disconnect case. The perf harness measures at the same edges, at 1 and 15 panes (desiderata budgets).

## Deliberately open

Left to implementing agents with better information at build time:

- The concurrency mechanism, as long as the event-model property holds.
- Wire framing of the app's CLI and IPC endpoint.
- Reconciliation cadence and the liveness probe.
- Per-view tab selection: default to the daemon's focused tab, or remember per view.

Project-level undecideds - the language split, optimistic UI, reproducible presentation
state - are tracked on the kan board.
