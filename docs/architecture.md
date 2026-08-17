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

The macOS shell is AppKit, and SwiftUI appears in one file: the find bar, which was ported from Ghostty's rather than
rebuilt. **Either toolkit is a fair choice for a given piece of the window** - this layer exists to feel like the
platform, and both of them are the platform. A second toolkit in one window does cost integration work, and the find
bar is the record of what it cost: its layer background is forced clear because a GPU surface sits
underneath it, it carries a workaround for a SwiftUI crash on macOS 15, and it reaches its own text field through a
`NotificationCenter` hop because `@FocusState` cannot be read from outside a view body. Weigh that bill when adding
the next SwiftUI view. It is a cost, not a line nobody may cross.

**Core** (headless, OS-free, Rust). The view-model and the only place decisions live: the mirror of each daemon's
state, the action dispatcher, keymap policy, attention routing, configuration. The core never touches an OS API, a
real clock, or a socket directly - those arrive through injected edges.

Rust rather than the shell's language, decided in `mip/0001-portable-core.md`. The short version: a boundary
between shell and core has to exist the moment a second platform appears, because a Linux or Windows shell will not
be Swift either. Putting a portable core on one side of it means macOS pays a well-supported FFI direction and every
other platform pays nothing.

**Backend adapters** (herdr today). Translate the Muster vocabulary to a concrete backend. One adapter per backend;
nothing herdr-shaped escapes it.

**Muster ships its daemon and runs it, and talks to no other.** Not for convenience, though it is convenient: a
person using Muster should not have to learn what herdr is. It is what makes the rest of this document mean
anything. The corpus is recorded from one pinned build, so a Muster attached to some other daemon is a Muster whose
every behaviour is unverified - and the daemon on herdr's default socket is whatever the user last started. So the
bundle carries the binary named in `deps/herdr.pin`, the app finds it beside its own executable rather than on PATH,
and it runs it under a herdr session of its own. A stranger is then not something to detect; it is something that
cannot arise.

Started, never stopped, because sessions outliving the app is the point. What it costs is the escape hatch - a
terminal's `herdr pane list` does not see Muster's panes - and `herdr --session muster` buys it back, since the
session is herdr's own concept rather than one invented here. Naming a `socket` in Muster's config file is the way
to ask for a particular daemon on purpose; nothing else in the environment is read, so an exported
`HERDR_SOCKET_PATH` meant for somebody's own CLI cannot quietly redirect a window.

**The daemon's environment is built, not inherited, and that follows from it being started and never stopped.**
Whatever shell launched Muster is a moment; the daemon is not, and every pane's program is its child - so anything
carried in at birth becomes state handed to every agent, on a process that outlives the app that carried it. An
allowlist rather than a denylist, because a denylist is wrong until somebody notices, and the way you notice here is
an agent behaving strangely for reasons nothing on screen explains. It stays short because a pane runs a shell and a
shell rebuilds its own world from the user's files: what has to survive is only what a shell cannot work out for
itself - where home is, what to run, the machine's locale, and the person's own ssh agent. The launch says in the run
log what it carried and what it dropped, by name and never by value.

Not hypothetical. Launching Muster from inside a coding-agent session put that session's markers and its messaging
credentials into the daemon and from there into every pane, where a fresh agent read them, believed it was a child of
another session, and stopped saving its transcript - and it persisted after that Muster had quit.

**An allowlist can only carry what exists, so a little is supplied.** Muster is meant to be launched from the Dock,
and launchd hands a GUI process `HOME`, `PATH`, `SHELL`, `USER`, `LOGNAME`, `TMPDIR`, `SSH_AUTH_SOCK` and little
else - no locale at all, which is the exact absence `LANG` is on the list to prevent. Nothing looks broken today, and
the reason is a loan rather than an answer: building the renderer derives a locale from the platform and puts it in
the whole process, so the environment Muster reads a moment later has a `LANG` in it that no shell set. That is the
same borrowing as the fonts and colours Muster used to take from a Ghostty config file, and less visible - it depends
on the renderer being built before the daemon is started, and the day a renderer changes, every pane drops to the C
locale in silence. So Muster answers it: the shell reports what the platform says, because only it can ask macOS, and
the core decides whether a daemon gets it - only when nothing in the environment named a locale, since one half
inherited and one half supplied is the split the allowlist already refuses to create. The run log names supplied
variables as a third list beside carried and dropped, so "where did this come from" has an answer.

`TERM` is not one of them, and its absence is the more useful fact. herdr sets a pane's `TERM` itself, so no pane has
ever seen the daemon's; the one thing that reads it is herdr's host-terminal detection, which decides who a
notification is attributed to. Carrying it meant a Muster launched from Ghostty had its daemon posting notifications
as Ghostty, to a terminal that was not there.

**The daemon's config is derived, not borrowed, and it follows from the same fact.** A daemon of Muster's own that
reads a stranger's config file is not a daemon of Muster's own: a `default_shell` somebody set for their own terminal
decided what every Muster pane ran, and - the sharper half - `version_check` and `manifest_check` default to true, so
a daemon pinned by version and checksum took its update policy from a file Muster does not own. Pinning it is what
makes a green suite a claim about anything, and an update check is the one thing that moves it off the pin with
nobody asking. So Muster derives a config from its own file and names it to the daemon with `HERDR_CONFIG_PATH`, the
same shape of answer the renderer already gets. What makes that variable the right lever rather than a private
`XDG_CONFIG_HOME` is that it moves the config file and nothing else: the socket, the session state and the data
directory stay where herdr's own rules put them, so the escape hatch above still works and a daemon holding somebody's
agents is not orphaned by an upgrade.

The cost is one leak, and it is answered rather than accepted. A pane's process inherits the daemon's environment, so
that variable reaches every pane, and `herdr` typed inside one would read Muster's file instead of the person's. Every
pane-creating call therefore carries the user's own path back in its `env`. A parameter rather than a scrub, because
the two fail differently: forgetting to scrub is invisible from outside, while a parameter can be asserted - a
conformance case walks every intent Muster sends and fails any that herdr says could carry an environment and does
not, so a fourth way of making a pane fails the gate rather than leaking quietly. What it cannot cover is a pane
Muster did not make: one herdr restores after a daemon restart is built with no launch environment at all. That is a
stated limit, not a gap to chase.

The guarantee stops at the machine's edge. An SSH endpoint runs a platform this bundle carries no binary for, so a
remote daemon is still whatever is installed over there. Closing that means putting an agent on the far machine on
first connection, the way mutagen does.

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

**Two of them are questions rather than requests, and find is why.** A backend is asked what a pane's history holds
and where its viewport is looking; neither changes anything, so neither is an intent. Find is the only caller, and
the shape it wants is one method: a backend that searches its own scrollback answers directly, and one that does not
reads the history back and matches it in the core. herdr is the second kind and has no search at all
(`observations/herdr-0.8.0.md` section 17), so the day it grows one is a change to one function body. What no backend
gets to decide is what a match *is* - plain substring, ASCII case folding - because the renderer marks the ones on
screen with its own matcher, and two answers to "how many are there" is the one thing a find bar cannot have.

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

**First sight is the exception, and it has to be.** A pane that finished before Muster attached produced no
transition to observe, and a daemon outlives the app - so quitting and coming back is the ordinary case, not a corner
one. There the daemon's own answer is the only evidence there is, and it is real evidence: it knows the pane's tab was
in the background. So a pane arriving already `done` is adopted as unseen, and every observation after that is
Muster's. The same shape the mirror already uses for the field itself: structure sets agent state only for a pane it
is seeing for the first time, and the agent channel owns it from then on.

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
  View = f(daemon state). Owning the scrollback buffer is not the same as deciding how deep it goes: that answer,
  and what a pane runs, are Muster's, translated onward into a config file the daemon reads (the shape, above).
- **The core owns a mirror**: a derived, disposable cache of daemon structure, bootstrapped from an authoritative
  snapshot plus event subscription, rebuilt after any gap, never patched across one.
- **A daemon's answer is daemon truth, on the same terms as its events.** Not a prediction and not a patch: a
  statement about a change the daemon has just made, arriving on the request channel instead of the broadcast one.
  Both may be applied; neither may be assumed, and Muster still never writes anything it was not told. Timing is
  why it is worth having two channels: herdr answers a swap, a resize or a divider position with the settled
  arrangement in about a millisecond and broadcasts the same arrangement about a hundred later, so a mirror that
  waits to be told twice renders the arrangement it is moving away from (`observations/herdr-0.8.0.md`, section
  14). Between the answer and its broadcast the mirror is ahead of its own stream, so it remembers the arrangements
  the tab has passed through and drops each one once when it arrives - matched by shape rather than by whole
  layout, because the cursors beside a tree move on their own terms. Bounded by construction: an entry is spent on
  its first match, so nothing is suppressed indefinitely and a wrong guess costs one frame rather than a stuck
  window. How many may be remembered at once follows from the fastest thing that produces them, which is a dragged
  divider at about a hundred a second - roughly ten in flight, and a bound sized for anything slower is a drag that
  snaps back to where the gesture began.

  Reading an answer costs a reader per shape the daemon states one in, and herdr has two: flat rectangles for
  everything it broadcasts and for most answers, its own exported tree for a divider position. Muster reads both
  and tells them apart by which keys a payload has rather than by which verb answered, so a daemon that starts
  answering with either needs no change.
- **The core owns composition**: which daemons are attached, and which (daemon, workspace, tab) shows in which
  window region. Mixing is at tab granularity: a region displays one tab's pane tree, rendered from daemon truth;
  regions from different daemons sit side by side. Muster does not own an outer split tree over panes - that would
  make it a multiplexer (non-goal) - and a pane can never move between daemons: the process lives where it lives.
- **Composition is resolved against the mirror, never patched by events.** It names daemon things - a tab, a
  pane - and those go away without asking: a tab closed from another client, a pane whose program exited. Every
  such way ends in a window that ignores the keyboard and cannot say why, so composition is brought back into line
  with a daemon's mirror whenever that daemon's structure moves. A region whose tab is gone closes; view-local
  focus falls to a pane that exists.
- **Muster names its own panes.** A name is `p` and nine characters - `p1w3r07bsd` - minted by Muster rather than
  borrowed from the backend, and it is what every message in the schema means by a pane id. The reason is not
  tidiness: Muster has to be able to tell a pane which pane it is, and a backend's id arrives too late for that.
  herdr assigns `w1:p3` in its *answer* to `pane.split`, while the environment a new pane is born with has to be
  sent *with* the request, so there is no moment where Muster holds both. A name Muster mints goes into the request
  that creates the pane, reaches it as `MUSTER_PANE`, and is bound to whatever comes back. The registry that does
  the binding is `crates/muster-core/src/names.rs`, and the adapter translates at the wire - nothing above it
  spells a pane the backend's way.
  A name is unique across every attached machine, which is what makes it an answer on its own: two daemons both
  hand out `w1:p1`, and a caller naming a pane on the devenv has no way to know and no reason to say which machine
  holds it. So a request that names a pane and no daemon finds the daemon from the pane. The empty pane means "the
  one this window's keyboard is on", which is what a keybinding means and what every menu item sends.
  Two handles do still travel in the backend's vocabulary, both marked as such in the schema: `ViewPane`'s
  `backend_pane_id` and `ViewRegion`'s `backend_socket`, for the bridge, which streams frames from the daemon
  directly. Workspaces keep the backend's ids.
- **Muster names its own tabs too, for the other half of the reason.** `t1w3r07bsd`, from the same registry and
  translated at the same wire. Nothing has to tell a tab which tab it is, so there is no reservation before creation
  and no `MUSTER_TAB` in any pane's environment - a tab name is minted the first time a daemon mentions the tab. What
  it buys is only the uniqueness: `w1:t1` is a string both machines hand out, so a window showing a laptop beside a
  devenv could describe a tab and offer no way to act on it. Now a request that names a tab and no daemon finds the
  daemon from the tab, exactly as a pane request does, and `muster tab focus` and `muster tab rename` are sayable.
  Unlike a pane there is no backend spelling beside it in the schema, because nothing outside the core ever names a
  tab to a daemon: no bridge streams one.
  A tab name is written down with the pane names, and for a reason that is about neither: the saved arrangement
  records which tab each region was showing, so a registry that forgot its tabs would fail every region's check on
  reopen and open the window as a first launch, every launch.
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
- **A watcher subscribes and then asks, because the two cannot happen at once.** A per-pane subscription can only be
  opened once the pane is known to exist, and dialing it takes time - so between a pane appearing and its watcher
  being live there is a window, and herdr has no replay for what falls in it. Nothing corrects it afterwards either:
  only a reconnect re-bootstraps, so on a healthy connection the pane keeps its old state indefinitely and looks
  calm. That is this project's founding claim failing silently, at the moment it is most likely to matter - just
  after a split, with something new started in the pane. So each watcher reads its pane's current state once it is
  subscribed, and the read is refused if the subscription moved the pane while the question was in flight: a stream
  is a better authority than an answer to a question asked at the same moment. The read is deliberately not counted
  as a transition, because the daemon counted one Muster never saw - counting it would hide the very gap it
  recovered from. Periodic reconciliation would paper over this rather than close it, and "papering over" is the
  accurate description: the pane would stay wrong until the next sweep.
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

**A pane is named twice over, and the two age differently.** A name somebody typed is durable identity - the daemon
writes it down, so it survives a daemon restart - and it wins over anything derived, because it is the only line
written for that pane rather than worked out from where it sits. Under it, when there is something to say, goes what
the agent calls itself: volatile status, lost on a restart because the process that would set it again is new
(`observations/herdr-0.8.0.md` section 16). Between them a row has a first line stable enough to learn and a second
that changes as work happens, which is what fifteen rows reading `<directory> · claude` could never do.

The second line is drawn only for a pane with a detected harness whose title says something the first line does not,
and that rule is the core's on the same terms as the ordering. A plain shell titles itself too - usually with the
path the row already leads with - so "draw it when it exists" would double the height of the list to repeat it, and
suppressing that by matching shell prompt conventions would be a guess about somebody's dotfiles where a detected
harness is a fact the daemon reports. A row is one line or two and never taller: a height that varied with the length
of what an agent wrote would move the rows below it while somebody was reading them.

Naming is an ordinary intent through the one action path, so a chord, a menu item, the CLI and an agent all reach it,
and nothing is rendered optimistically - a rename is applied from what the daemon answers, the way a split is. That
answer is the only route there is for Muster's own renames: herdr emits no event for one at all, so a rename made by
another client arrives when the connection next re-snapshots. Clearing a name is a null rather than an empty string,
and for a tab it is neither, because herdr has no spelling for it.

**The roster is a tree, because a tab is what a person navigates between.** Daemon, then tab, then pane: a flat list
of panes cannot say which of them sit side by side, and a region shows one tab, so "where has that agent got to" is a
question about tabs. A tab also says whether a region is showing it, which is not the same question as its panes
being on screen: a zoomed tab is on screen while all but one of its panes are not. Naming is the core's, on the same
terms as the ordering, and it drops what a backend's own label repeats - herdr names an unnamed tab after its
position, and Muster has a better position to write one from. Each row carries the tab's Muster name beside its place,
so reading the roster and acting on what it says are the same vocabulary.

**One numbering, and it is on the panes.** Every pane carries a place in a single count that runs across every
attached daemon and every tab, and that number is what the list draws and what ⌘1 to ⌘9 name. It is on panes because
the unit of this product is an agent and an agent is a pane: the rows carrying the states are pane rows, so a number
drawn a level above them would be one thing pointing at another. Numbering panes costs the tab axis nothing, because
focusing a pane surfaces the tab holding it - so the nine chords reach every tab too, through any pane in it. Tabs
keep a place of their own for stepping and for captioning one nobody has named, and no chord names it: two numberings
in one list is worse than either, and whichever thing is numbered, only one may be.

The number is positional and it moves when a pane above it closes. That is the cost of numbering the thing that
churns, and it is the right trade once the order is the user's to arrange: a stable number would keep its value when
you moved the row, which is the opposite of what the gesture asked for. What it does not fix is a number going stale
between reading it and pressing it, and the answer to that is elsewhere - a notification names the agent, not the
chord.

**Arranging the list is arranging the window, and Muster stores no order of its own.** Dragging a row is an ordinary
intent through the one action path: the daemon rearranges its own tree and the list is a view of that, so
`view = f(daemon state)` holds and the order survives a restart the way the panes do. The alternative - a
presentation order in `window.toml` beside tab order and widths - buys free-form insertion at the price of a list
that no longer says where a pane is on screen, and of Muster owning an ordering the daemon has never heard of.

One gesture, two requests, and the choice is the core's. Two panes in one tab exchange places; a pane dropped on a
row in another tab moves into that tab behind it. The shell knows only which two rows were involved, so it sends
both and the core picks the verb from where the panes are - a shell that chose would be a second place that rule
lives, and it would have to read the tree to do it. An exchange rather than an insertion because an arrangement has
no "between", which is also the constraint the backend imposes: herdr's swap is a pair of ids.

A drop across daemons is refused in the shell, before it becomes a request. A pane is a PTY its daemon owns, so
moving one to another machine means killing a process on one host and starting a different one on another - not a
move, and nothing the core could honestly do with the intent.

**Moving the keyboard comes in two axes, and the second is what makes the first a guarantee at all.** Panes and
tabs are different questions: the *relative* pane moves reach everything the window is *showing*, and the tab moves
reach what is behind it. Without the second, a pane in a tab no region has would be reachable only by clicking its
row - and the list can be put away, which would leave those panes with no door. The numbered chords are the third
route and cut across both, because a place names a pane whether or not anything is showing it. Tab moves have no
geometry, because tabs are a list and nothing is to the left of a tab; both directions wrap.

**Within the panes on screen, moving comes in two kinds.** Next and previous walk reading order across every region
and wrap, so between them they reach every pane - that is the guarantee. The four directions are geometric: the core lays the whole
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
running app over a local IPC endpoint, and it covers everything Muster does - panes and tabs as well as focus and
arrangement.

**The endpoint is the same schema on a different transport, not a second path.** A window binds
`~/.muster/state/command-<pid>.sock` and answers a length-prefixed `Request` through the same `dispatch` the C ABI
calls, one request per connection, a thread each so a `pane new --run` waiting on a shell prompt does not hold up a
caller asking what the window looks like. Nothing there decides anything; a second entry point that made its own
decisions would be a second Muster.

**A pid in the socket name, because two Musters are two windows.** A caller has to be able to reach the one it means,
and a single fixed path would mean the second window to open silently took the first one's callers. Which window a
pane belongs to is settled when the pane is made: Muster puts `MUSTER_SOCKET` in the environment of that request,
beside the `MUSTER_PANE` that says which pane it is, and between them a program inside a pane can drive the window it
is drawn in without being configured. Only for a daemon on this machine - a unix socket path means nothing across an
ssh tunnel, so a devenv pane is told nothing and correctly concludes it is not in a window it can drive.

**The command has to be findable, or the surface is taught rather than discovered.** Muster keeps `~/.muster/bin`,
points a link in it at the CLI the running app shipped, and gives the directory to every daemon it starts as the
front of that daemon's `PATH`. A pane is a child of its daemon, so that is every pane. The link is refreshed at each
launch rather than installed once, because the app it points at moves.

**It is not a view-layer CLI beside the backend's own.** That was the earlier plan, on the reasoning that herdr has a
good CLI already and Muster should not reimplement it, and three things sank it. A window can be attached to more
than one daemon, and a backend CLI inside a pane reaches that pane's daemon and no other - so it cannot put a pane on
the devenv, and cannot answer what the window is showing, because no single daemon knows. Making a pane and landing
on it is one intent to whoever asked, and splitting it across two CLIs makes the second half unsayable: an
arrangement made behind Muster's back appears, because view = f(daemon state), but nothing focuses or zooms it and
the caller has no way to ask. And a documented backend-shaped surface becomes the contract whatever this document
says, because every script and skill written against it is what a replacement would have to provide - which is the
one thing "we never let one own our contract" rules out.

So the backend's own CLI is not Muster's agent surface. It stays reachable - herdr sets `HERDR_*` in every pane and
Muster does not hide that - and it is unsupported: it speaks the backend's vocabulary rather than Muster's, and
nothing here tracks it.

What that CLI is *not* is a verb-per-backend-verb translation. It is shaped to intents, one call each, because that
is where the knowledge lives: a pane's program is spawned with the pane, so text sent before its shell has drawn a
prompt races the program's own first output. Anybody scripting "make a pane and run this in it" by hand rediscovers
that wait and gets it wrong under load. One call owns it once.

Reads are half of it. A person driving the GUI can see which panes are on screen and where the keyboard is; an agent
has to ask, and a CLI that only writes leaves one arranging a window it cannot look at. `ReadWindow` answers the
view, the roster, every pane's agent state and each daemon's health as one message, built by the same builders that
produce the events a shell is sent - so a read cannot contradict what is on screen. Health is in the answer because
the rest of it is a mirror, and a mirror nobody has heard from in an hour looks exactly like a current one.

On macOS a keybinding *is* a menu item, because that is where the platform dispatches a key equivalent. Matching
chords before the pane sees them would take shortcuts the user rebound in System Settings and make them mean
something else, and would hide from every menu what the app can do. So the menu is where Muster's own actions live,
and each item does nothing but dispatch.

**The intent is parameterized; the action is not.** `CreateTab { workspace, cwd }` takes arguments, and `new_tab` is
a parameterless name that dispatches it with defaults. That split falls out of the menu: an item has exactly one key
equivalent, and that is also the handle System Settings needs to rebind it, so an action name has nowhere to put an
argument and `[keymap]` stays keyed by action. Ghostty's chord-keyed form - `cmd+shift+h=resize_split:left,150` -
lets a config name two chords for one action, or a binding with no action name at all, and neither is something the
menu can represent or the platform can rebind.

What that costs is paid in the config file rather than in the vocabulary: an amount a chord would have carried
becomes a root key, which is what `resize_step` and `scroll_multiplier` are. Where an action genuinely has a small
closed set of arguments, it becomes that many actions - `focus_pane_1` through `focus_pane_9` are nine names the
config file and the menu can both say, over one `Action::FocusPane(u8)` in the core. Nine menu items need nine
actions; the core still holds one intent.

The CLI does not inherit any of this. It names intents directly and passes arguments, because nothing about it is a
menu - which is the whole point of separating the two, and why the constraint stops at the keymap.

Something the app notices on its own is a *trigger* for an action, never a second way of doing it. The config-file
watcher is the worked example: saving the file dispatches `reload_config`, the same action a chord, a menu item and
a CLI dispatch. That is what keeps "one action path" true of things nobody pressed - the alternative is a second
implementation that drifts from the first, and a bug report where the answer depends on how the reload was asked
for.

**A failure the person caused is reported to the person, as a condition rather than a message.** The core holds a
list of problems keyed by what is wrong (`problems.rs`), publishes it whole, and the roster draws it at its foot;
raising the same condition twice is one problem, and it clears when the condition does. Keyed and whole because both
alternatives break the same way: a stream of messages lets a window go on showing a config refusal after the file
was fixed, and an add-and-remove protocol lets it disagree about how many there are. The disappearance is also the
only acknowledgement a fix ever gets.

The run log is not that surface, and mistaking it for one cost a whole evening: a `resize_step` written without its
unit refused a config file at 18:55, every setting in it went inert, and the window said nothing until somebody
opened a JSON file the next morning. So a run log entry answers "what happened here" for whoever is debugging, and a
problem answers "what do I do now" for whoever is typing - the same fact, twice, because the two readers arrive by
different doors. Severity exists to decide interruption and nothing else: an error opens a roster somebody closed, a
warning waits to be found.

**The list also carries failures nobody caused, and a pane that never becomes typeable is the first of them.** A
pane's keystrokes travel through a bridge that dials a socket the core bound for it, and until that connection
arrives the pane renders, paints, and discards everything typed into it. Three separate bugs ended in exactly that
state - the bridge failed to dial, the socket path had moved, the channel could not be opened - and every one of them
was found by somebody typing. Both ends of the wait were already known to the core, which binds the socket and runs
the callback the accept fires, so what was missing was a deadline between them: five seconds, one problem per pane,
cleared by a bridge that arrives late and by the pane closing. An error rather than a warning, even though nobody
misconfigured anything, because severity is about interruption and a warning waiting to be found would be found the
old way - by typing into a pane that had stopped listening. The decision is a fold in `typeable.rs` and the clock is
a single parked thread in the seam, so an idle window costs no wakeups and the rules are answerable by a case.

Nothing renders an intent optimistically. A split, a close, a focus and a divider drag are all requests, and what
came of them arrives as the next published view - so a window can never show an arrangement no daemon agreed to. The
one thing an intent may settle locally is where Muster's own keyboard lands, because that is Muster's state and not
the daemon's: a split hands back the pane it made, and that pane takes the keyboard, because that is what pressing
the key meant.

**A request may also be waited for off the main thread, and a divider drag is the one that has to be.** Every other
gesture is one request; a drag is one per mouse-moved event, about a hundred a second, and the seam is entered
synchronously - so the window spent whole gestures inside a round trip and had no time left to draw the line being
dragged. The position is handed over instead: one request in flight, the latest position remembered, and what
arrived while a request was out goes next. A gesture then runs at whatever the round trip allows rather than
queueing behind itself, and the position it ends on is always sent, because the remembered one is always the last
asked for. This is contained to that request rather than made general - the other drag in the window moves a region
boundary, which is Muster's own composition and never reaches a daemon.

## The renderer seam

The renderer gets the same treatment as the backend: a narrow contract in Muster's terms - create a surface in a
region, run a pane channel into it, resize it, mark text on it, read its grid (the test oracle) - and nothing
libghostty-shaped escapes the seam.

**Marking text is a division of labour rather than a second implementation.** libghostty has a full search, and it
covers the scrollback of a terminal it owns - which is not the situation here, because a surface is repainted from a
frame stream and holds no history. So the core searches, against what the daemon hands over, and the renderer is
asked only to mark occurrences of a string on the screen it has already painted. There is still exactly one answer to
how many matches exist and it is the core's. What the renderer refuses comes back rather than throwing, on the same
terms as sizing text: a renderer that cannot mark costs the marks and nothing else, and that is a line for the log. Today the only way to feed an embedded ghostty surface is the command it spawns; the embedding
header has no byte-feed API. The pane channel is therefore delivered by a bridge subprocess the surface runs. That
is a fact about current libghostty, not a choice - re-verify on upgrades, and revisit if upstream grows a direct
feed.

**Appearance crosses this seam in Muster's words, and reads no file belonging to another application.** Muster
called `ghostty_config_load_default_files` until 2026-08-16, so a Ghostty config on disk decided what a pane looked
like - which left the renderer the one dependency not behind the contract, since a replacement would have had
nothing to read. What replaced it is `[font]`, `[colors]` and `[cursor]` in `~/.muster/config.toml`, parsed and
refused by the core, published on one `Appearance` read, and translated by the shell for whichever renderer is
behind the seam. One function in `MusterRenderer` knows a ghostty config key exists, and no ghostty spelling
appears in `crates/`, in the schema, or in the corpus - `hollow` is Muster's word and `block_hollow` is the
translation's problem.

The vocabulary names what a person may change, and nothing else: every value is optional, and absent means the
renderer's own default rather than one Muster invented. That is deliberate and it is a stated limit rather than a
gap. Muster has no opinion about which monospace font a machine has, and a sixteen-entry default palette written
into the core would be a transcription of somebody else's rather than a decision - so a replacement renderer
supplies its own defaults for anything unnamed.

How the values get there is a fact about libghostty rather than a choice, and the same shape as the bridge: the C
API has no setter, so the shell writes a derived config file and hands over its path
(`docs/observations/libghostty-9f9b8d1d.md` section 9). A synthesized argv works too and needs nothing on disk, but
`ghostty_init` assigns process-global state and so can only be done once - a file serves both the first launch and
every reload after it, and one mechanism cannot disagree with itself. The derived file is state, lives beside
`window.toml`, and is rewritten every launch; it is also the answer to "what did Muster actually tell the
renderer", which is the first question when a colour does not take.

The backend seam does the same thing for the same shape of reason - herdr takes a value only as a file too - with one
difference worth stating: an appearance naming nothing produces no file at all, because every value in it is
somebody's preference, while the daemon's is written even when nothing is configured. An unconfigured Muster still
has an opinion there, and it is that the daemon it pinned does not go looking for its own updates.

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
| Muster quits or crashes | nothing of the session; the OS permissions its panes were granted | nobody needs to; the daemon owns the PTYs |
| the connection drops (VPN, lid, SSH) | nothing; the view goes stale and resyncs | the degradation model above |
| the daemon restarts | every process; scrollback | herdr restores the tree and cwds |
| the machine reboots | the same, plus the daemon must come back | as above |
| the machine is gone | local work only | remote daemons keep running |

**The first row's exception is the platform's rather than Muster's, and it cannot be written down.** macOS charges a
protected request - a folder, the camera, AppleScript - to the *responsible* process, which for a pane's program is
the Muster that started its daemon. That holds only while that Muster is alive: measured, every surviving pane
becomes its own responsible process the moment the app exits, and a later Muster cannot adopt them, because
responsibility is fixed when a process is spawned and nothing lets an app claim a chain it did not start
(`observations/macos-26.4.1.md`). So a permission granted to Muster covers every pane until the relaunch, and after
it a prompt names the agent's own binary. Nothing in this document fixes that; it is here because "Muster quits or
crashes: nothing is lost" is otherwise a promise with a silent hole in it.

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

**Muster's own durable state is composition, plus what the window looks like.** Composition is which daemons are
attached, and which (daemon, workspace, tab) shows in which window region. Beside it, in the same file and under a
table of its own, is the window's own chrome: whether the roster is open, how far the text is sized from what the
config asked for, how big the window is, and whether it is full-screen. Everything else Muster holds is derived.
That is a few hundred bytes, and its smallness is the point: the shell owns nothing, so there is nearly nothing to
save.

The two are kept apart because they answer different questions. A region is a wish about a session that may have
moved on, checked against what the daemons turn out to hold; a chrome setting has nobody to check with, so it comes
back as it went in. The window's frame is the one thing in either half that is checked against neither: the display
it was measured on may be gone, so the shell reports the screens it has and the core answers where the window should
open. That split is the same one `locale` draws - only a shell can ask the platform, only the core decides what to
do about the answer - and it is what keeps a rule about displays somewhere a test can reach.

Written as it settles rather than at quit, on both halves, because quitting is not how this is usually lost: the
whole durability table above is about crashes, reboots and dropped connections. What follows from that is a rule
easy to break in one line - **nothing writes the file before the window has opened**. A composition nobody has
opened is empty, and a shell reports its frame the moment the window exists, which is before it asks the core to
open anything.

But composition is the piece nobody else can save. A herdr daemon's export is scoped to itself and structurally cannot
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

**It is that surface on purpose, and `./dev --contract` is what reads it.** Worth stating outright because the
alternative reading - that the log is a debugging convenience tests happen to use - would make a record's name and
fields free to change, and they are not. What a check asserts on is the answer to a question (`did the roster open
over a refused config`), and a record that stops answering it is a change to a contract rather than to a log line.
The line this does *not* cross is the one below: the log is not how a **person** is told something is wrong. That
mistake cost an evening - a config refused at 18:55 went to the log and nowhere else - and the roster is where a
person finds out. The two are different audiences for the same fact.

What makes it worth asserting on rather than a poor substitute for a real test is that it spans processes a test
cannot otherwise see at once: the app, its bridges and its daemon write to one file, so a check can state that a
gesture reached a pane without a window, a keyboard or a screenshot. Its limit is the same shape - it says what the
app *did*, never what it *drew*, so pixels, layout and legibility stay outside it and stay manual.

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
