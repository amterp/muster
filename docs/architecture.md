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

**Core** (headless, OS-free). The view-model and the only place decisions live: the mirror of each daemon's state,
the action dispatcher, keymap policy, attention routing, configuration. The core never touches an OS API, a real
clock, or a socket directly - those arrive through injected edges.

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
success. One transition has a client-side input: herdr distinguishes idle from done partly by whether a pane has
been *seen*, and only backend-recognized focus counts as seeing. Muster therefore reports focus to the daemon as
panes gain focus in its windows - otherwise every pane it renders reads as unseen forever. State is daemon truth,
but seen-ness is fed by clients, so the vocabulary carries a "pane was seen" intent.

## Control plane, data plane

Two kinds of traffic, opposite needs, different paths. The split is between *output* and *everything else* - not
between "bytes" and "control":

- **Output rides the data plane.** Each visible pane has its own channel from adapter to surface, bypassing the
  core. With herdr the channel carries server-rendered frame diffs of the pane's screen - not the raw program
  output - so the daemon's render cost scales with *visible* panes: hidden panes detach their channels, and
  revealing a pane costs one full repaint. The core never sits in this path; per-byte work in the core is a defect
  (desiderata: fast is a feature).
- **Everything else rides the control plane, through the core** - daemon events (structure, agent states, bells,
  titles), intents, configuration, and *input*. Key encoding needs the pane's terminal modes (kitty keyboard,
  bracketed paste, mouse reporting), and those modes live in the daemon's VT; they are not replayed in the frame
  stream. So surfaces do not encode input: the shell reports key, mouse, and text events with full fidelity, the
  core routes them, and encoding happens at the adapter seam or in the daemon - whichever stage 1 of the founding
  plan (`origin.md`) proves out. This mirrors herdr's own client design: report maximal key information, encode
  where the modes live.
- **Scroll belongs to the daemon.** A frame stream has no history, so surfaces hold no scrollback and never handle
  the wheel: scroll is an intent, answered by the daemon repainting the viewport.

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
  and sends resize intents. While Muster controls a pane the daemon holds it at Muster's geometry and other clients
  view it at that size; the hold releases when Muster detaches. Sessions always survive Muster - but concurrent TUI
  *viewing* of a Muster-controlled pane is degraded by design, and the fallback guarantee is scoped accordingly.
- **The shell owns nothing.** Surfaces are disposable renders of a pane channel. A surface attaching to a live pane
  starts with a full repaint and never assumes it saw the start of the stream. Closing a window destroys surfaces
  and touches no session.

## Event model

State changes only by applying events and intents in one place, in one order per daemon connection; rendering reads
the result. Pane content is not state in this sense - it is a stream the surface renders. Two constraints carry the
model:

- **Application is convergent.** herdr exposes no event sequence numbers and no replay, so a gap cannot be
  detected, only survived: every event is applied idempotently and carries absolute values, never deltas, so that
  snapshot-plus-events converges regardless of what raced the bootstrap. Periodic reconciliation against a fresh
  snapshot is the only true gap detector; its cadence is an implementation choice.
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

## Seams and test hooks

The injected edges, matching `testing.md`: the backend connection (a fake daemon speaks the same
vocabulary), the clock, and the renderer seam (tests feed pane channels through libghostty-vt and assert the
resulting grid). The contract corpus sits at the adapter seam; it audits the fake against a real, version-pinned
herdr. The perf harness measures at the same edges, at 1 and 15 panes (desiderata budgets).

## Deliberately open

Left to implementing agents with better information at build time:

- The concurrency mechanism, as long as the event-model property holds.
- Wire framing of the app's CLI and IPC endpoint.
- Reconciliation cadence and the liveness probe.
- Per-view tab selection: default to the daemon's focused tab, or remember per view.

Project-level undecideds - the language split, where input encoding lands, optimistic UI, reproducible presentation
state - are tracked on the kan board.
