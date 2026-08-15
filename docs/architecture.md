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
nothing herdr-shaped escapes it.

**Reaching a remote daemon is a transport concern and stops there.** A remote herdr speaks the same socket a local
one does, so an SSH master forwards that socket onto a path on this machine and the adapter is handed a path like
any other - the client, the snapshot, the subscription, the agent watchers and the server-side encoder are
unchanged. The evidence that this is safe is `observations/herdr-0.8.0.md` section 8: the same recordings against a
Linux daemon differ in nothing. The data plane cannot use the trick, because herdr publishes a pane's frames through
a CLI over stdio rather than through a socket method - so a pane's bridge runs that command over the same master,
which is also what keeps a remote pane as cheap as a local one. Reimplementing that stream was rejected: it is
bincode over herdr's internal types with no published schema, which is the byte-level protocol emulation
`testing.md` deletes. A tunnel that drops is reopened onto the same path, so recovery is the adapter's ordinary
reconnect rather than a mechanism of its own.

## The vocabulary

The backend contract speaks Muster's terms, not any backend's. Nouns: backend session (one daemon connection),
workspace, tab (the unit that owns one pane tree - trees hang off tabs, not workspaces), layout (a tab's tree, as
proportions rather than cells), pane, pane channel (the output stream feeding a surface), agent state.

**A layout is proportions, never geometry.** A backend sizes panes for a viewport of its own - herdr's is a fixed
54x23 whether a client is attached or not (`observations/herdr-0.8.0.md` section 13) - so the cell rectangles it
publishes describe nobody's window. What crosses the seam is the tree and its ratios; the shell lays that out at
whatever size it has, and the pane's own geometry follows from the controller as below. Verbs, as intents: attach, split, close, focus, resize, send input,
scroll, spawn. Small on purpose - everything the view needs, nothing any particular backend happens to offer. The
contract corpus at this seam is the executable form of this vocabulary and the definition any replacement backend
(fork or wholesale) must satisfy.

Agent states are working / blocked / idle / done / **unknown** - five, not four; unknown renders as itself, never as
success. State is daemon truth, but one of the five is computed from a client-side input, so the vocabulary has to
carry that input.

`done` is not stored anywhere: it is `idle` on a pane that has not been *seen*, and seen-ness is written only when an
agent completes - a working or blocked pane going idle. **Muster derives it rather than reading it.** herdr derives
one too, from whether the pane's tab is active and whether the foreground client's window has OS focus, and its JSON
API has no method for the second (`observations/herdr-0.8.0.md` section 3). A daemon asked to decide this for a
window it cannot see is answering from a client that never reported, so its `done` is normalized back to `idle` on
the way in and Muster's own answer replaces it. Two writers for one field is the failure named below; of the two,
only one can see the window.

Muster's rule is the same shape with inputs it actually has. A pane is seen when it is on screen in a window that has
the OS's focus, and the shell reports that focus across the seam because nothing else can observe it. A completion on
a seen pane is `idle`; anywhere else it is `done`, until somebody looks - gaining focus and bringing a pane on screen
both settle it. Looking away does not un-see what was already seen. `pane.focus` is still written to the daemon,
because it activates the pane's tab and other clients read that.

What this cannot answer, stated rather than hidden: ours is the only focus we can observe, so `done` means "nobody
*we know of* saw it". A second Muster window, or a herdr TUI open beside us, is outside it.

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
- **Composition is resolved against the mirror, never patched by events.** It names daemon things - a tab, a
  pane - and those go away without asking: a tab closed from another client, a pane whose program exited. Every
  such way ends in a window that ignores the keyboard and cannot say why, so composition is brought back into line
  with a daemon's mirror whenever that daemon's structure moves. A region whose tab is gone closes; view-local
  focus falls to a pane that exists.
- **A pane is named by its daemon and its id.** Two daemons hand out the same ids - `w1:p1` means something on each -
  so a bare pane id stops being an answer the moment a window shows more than one. Every message that names a pane
  says both, and anything keyed by pane alone that spans regions is a bug waiting for a second daemon. The empty
  string means "the one this window's keyboard is on", which is what a keybinding means and what every menu item
  sends. One place is left genuinely ambiguous and says so: a command line carrying only a pane id, at the moment
  before any daemon is being followed.
- **Health is per connection, and so is what a window says about it.** A laptop and a devenv have two answers and one
  title bar. The unhappiest is what shows, named - reporting one state for the window would let a dropped VPN read as
  though every session had gone.
- **Cursors are written, not read.** Daemon focus (focused workspace, tab, pane) is a single value per daemon,
  shared with every client including the herdr TUI. Muster's input routing - which pane its keyboard feeds - is
  view-local. Interacting writes daemon focus (which activates the pane's tab, for the clients that read it); Muster
  never *routes* input by reading
  it, so another client moving daemon focus never yanks Muster's keyboard. Both halves are one action to whoever
  clicked, so one request does both: the keyboard moves whatever the daemon answers, and a refused write is worth a
  log line rather than undoing a focus move the user watched happen.
- **A tree that disagrees with its tab is not an arrangement.** A tab's pane list and its pane tree arrive as
  separate events with no order between them, so the tree can name fewer panes than the tab holds or more, and a
  subscription that has just bootstrapped replays layout events - walking a tab backwards through arrangements it
  had minutes ago. Both measured against herdr 0.8.0. The pane list decides what exists; the tree decides only
  order, and one that disagrees is withheld rather than repaired. Repairing means inventing a place for a pane
  nobody placed; publishing it drops every pane it omits, and a dropped pane costs its surface and the bridge that
  feeds it. Withheld is a state the shell already answers correctly, by leaving what it is showing alone.
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
- **Gaps are quantifiable for agent state in arrears, and never for structure.** Measured rather than assumed
  (`observations/herdr-0.8.0.md` section 10). An agent's `state_change_seq` is stamped from one session-wide
  counter, so comparing two of them says how many transitions ran in between, including on panes the client has
  never heard of. In arrears because the stamp reaches a client in exactly one place - a snapshot's agent list - and
  is on no event: what a client learns, it learns at the moment it re-snapshots, which is the moment the snapshot
  has already made it correct. So the number is not a consistency signal but an attention one, and Muster reports it
  as such: an agent may have asked for the user while nobody was listening. A pane's `revision` answers none of
  this - it tracks terminal titles and metadata tokens and does not move when an agent changes state. Nothing at all
  reports a pane created and closed inside a gap, so structure has no evidence-based detector and periodic
  reconciliation against a fresh snapshot is the only one. Cadence is a separate decision.
- **Removal has two spellings.** A pane a client closes emits `pane_closed`; a pane whose program ends emits
  `pane_exited` and no `pane_closed`. Both must drop the pane, or an exited pane renders forever.
- **Agent state has one writer.** herdr carries `agent_status` on its pane payloads as well as on its agent events,
  and the payloads are replayed as of when a subscription opened - so letting structure write that field means a
  reconnect can roll a working agent back with nothing following to correct it. The agent channel owns the field;
  structure sets it only for a pane it is seeing for the first time.
- **Agent state costs a connection per pane.** `pane.agent_status_changed` takes a `pane_id` and no session-wide
  subscription carries the same information (`observations/herdr-0.8.0.md` section 11), so an overview of N panes is
  N held-open connections plus one for structure. Muster subscribes for every pane the mirror holds rather than only
  the attached one, because showing them all is the point and measuring the cheap arrangement would tell us nothing
  about the one that ships. Measured at fifteen panes, that is one thread and ~48 KB each, idle at zero CPU - so the
  arrangement stays, and the upstream ask is a courtesy rather than a need. What it does cost is two descriptors per
  pane against the 256 a GUI-launched process inherits, so the shell must raise its own soft limit at startup.
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

**Everything the daemons hold is published as well as everything on screen.** A window shows what fits in it, and the
pane most likely to have finished unnoticed is the one no region is showing - so a roster travels beside the view:
every pane every attached daemon holds, ordered and named for a reader, each row saying whether anything is showing
it. Order and label are decisions and live in the core, so the sidebar, the CLI and an agent all get the same answer.
Structure only, like the view: what an agent is doing keeps its own per-pane message, because a roster is stable and
a state blinks.

**Moving the keyboard comes in two kinds.** Next and previous walk reading order across every region and wrap, so
between them they reach every pane - that is the guarantee. The four directions are geometric: the core lays the whole
window out from the ratios it already publishes plus the region weights, and picks the pane actually in that
direction, requiring it to overlap the source across the direction of travel. They do not wrap, because reachability
is already covered and predictability is worth more. Asking the daemon was rejected: `BackendChannel::submit` is
write-only by design, and every future backend would owe us a read to answer a question about an arrangement Muster
already holds. A tree walk was rejected too - on a perpendicular split it has to pick a child by position in the tree
rather than by where it is, so in any asymmetric arrangement it lands somewhere the user did not point at.

**The arrangement over regions is Muster's, and only Muster's.** Each region carries a weight and the window divides
its width by their sum, so equal shares are what a window that has never been dragged looks like. A weight per region
rather than a ratio per boundary, because regions are a list and not a tree - owning a tree over them is what would
make Muster a multiplexer. Dragging the line between two regions moves only that pair's share of itself, so nothing
further along the window moves, and it is the one drag that settles in the core rather than being asked of a daemon:
no daemon knows the other one exists. It is also the one share that is clamped, because nothing sits behind it to
refuse an impossible one, and a region dragged to nothing would leave no divider to grab.

**A focus request surfaces the pane it names.** Naming a pane no region is showing retargets a region onto its tab
rather than being refused - a list of panes that cannot be reached is a display, not routing. The region chosen is
one already on that pane's daemon, preferring the focused one; a region on another daemon is never taken, because a
window showing a laptop beside a devenv is the arrangement this project exists for. Only a daemon with no region at
all gets a new one.

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

On macOS a keybinding *is* a menu item, because that is where the platform dispatches a key equivalent. Matching
chords before the pane sees them would take shortcuts the user rebound in System Settings and make them mean
something else, and would hide from every menu what the app can do. So the menu is where Muster's own actions live,
and each item does nothing but dispatch.

Nothing renders an intent optimistically. A split, a close, a focus and a divider drag are all requests, and what
came of them arrives as the next published view - so a window can never show an arrangement no daemon agreed to. The
one thing an intent may settle locally is where Muster's own keyboard lands, because that is Muster's state and not
the daemon's: a split hands back the pane it made, and that pane takes the keyboard, because that is what pressing
the key meant.

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

**A fourth state, and the one that is easy to get wrong: the daemon answered, and the session it describes is not
the one we knew.** After a daemon restart the pane ids and the tree come back but every terminal is new
(`observations/herdr-0.8.0.md` section 12); after a config change or a crash the session may be empty entirely.
Every test for "connected" passes in both cases, and rendering an empty session as though the user closed
everything is the worst available answer. This is a distinct state with a distinct response - say what was there,
offer to rebuild it - and it belongs to Muster because no daemon can know what a window was showing.

## Durability

What survives what. Written down because "sessions outlive everything" reads as one guarantee and is really four,
and because the layer that can honestly answer each is different.

| | what is lost | who can help |
|---|---|---|
| Muster quits or crashes | nothing | nobody needs to; the daemon owns the PTYs |
| the connection drops (VPN, lid, SSH) | nothing; the view goes stale and resyncs | the degradation model above |
| the daemon restarts | every process; scrollback | herdr restores the tree and cwds |
| the machine reboots | the same, plus the daemon must come back | as above |
| the machine is gone | local work only | remote daemons keep running |

Two things follow.

**Persist intent, never observation.** The mirror is an observation - these panes exist, this agent is blocked,
focus is here - and it is explicitly disposable. A restore description is an intent: make a right-split with these
two directories. Writing down observations would tempt a restore to reinstate things that are meaningless after a
restart (agent status, scroll offsets, revisions), so the mirror is deliberately **not** serializable and gains no
persistence hooks. What gets written down is what someone would ask for again.

This also resolves an apparent gap in "view = f(daemon state)": restoring looks like it needs an inverse, and does
not. A restore is a sequence of ordinary intents - create a workspace, apply a layout, spawn - so it flows through
the one action path, which makes it scriptable, agent-drivable, and testable with no new mechanism. `layout.apply`
is the primitive, and it is additive rather than reconciling, so whatever calls it must apply into something fresh.

**Muster's own durable state is composition, and only composition.** Which daemons are attached, and which (daemon,
workspace, tab) shows in which window region. Everything else it holds is derived. That is a few hundred bytes, and
its smallness is the point: the shell owns nothing, so there is nearly nothing to save.

But it is the one piece nobody else can save. A herdr daemon's export is scoped to itself and structurally cannot
describe a workspace spanning a laptop and a devenv, because neither daemon knows the other exists. Muster is the
only layer that sees across them, which makes cross-daemon composition the part of durability that is genuinely
ours - and it follows that restore is per-daemon and partial by nature, since after a reboot the local daemon comes
back fresh while the remote one never noticed.

What Muster must not do here: keep its own session store, or infer an agent's resume token by reading its output.
The first is the multiplexer non-goal and the second is the agent-framework one. Reporting a session reference the
harness hands over is metadata about a pane and is fine; herdr already has `pane.report_agent_session` for it, and
that is where a real "resume this agent" story lives - in the harness's own session, not in the terminal.

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
