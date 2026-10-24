---
mip: 8
title: Panes from any machine, anywhere in a tab
status: Draft
kind: Architecture
created: 2026-10-21
decided:
supersedes:
superseded-by:
related: 2, 3, 6, 7
---

# MIP-8: Panes from any machine, anywhere in a tab

## Summary

A tab gets one split tree over all of its panes, whatever machine each runs on. A devenv pane can
sit below one laptop pane and beside another, and every way of placing a pane - the right-click
menu, dragging, `muster pane new --daemon`, `muster pane move --onto` - treats a pane on another
machine like one on the same machine. A pane's process still never moves: what moves is its place
in the tab.

The tree is kept on every daemon holding a part of the tab. Each part stores the whole tab's tree,
in which a pane on another machine is a *foreign leaf*: its id and its machine's name, kept in
place and never run. The tree carries a generation, as a tab's name does. A window merges the
copies it can reach into one tree, draws that, and writes it back to every part that lags. So no
tab's shape depends on the window's arrangement file, any one reachable machine brings back the
whole shape, and the window still owns no truth.

This reverses two linked decisions in `docs/architecture.md`: that a tab holds one region per
machine side by side, and that Muster owns no tree over panes because owning one "would make it a
multiplexer". Section 10 says what changes in the doctrine. Four questions were put to amterp
and end this document; D is decided, A to C are open.

## Context / Motivation

### What was asked for

amterp, chat notes 2026-10-20: "right click split into daemon doesn't allow seamless pane
placement like a same daemon pane would". At triage on 2026-10-21 amterp asked for the general
form and made it high priority: any arrangement of panes in a tab, with the machine a pane runs
on having no effect on where it can go (kan `a_2lvMf2JeD`). And on what it would take: "Maybe
there's some input that is needed from me regarding some compromises ... maybe there are some
decisions we need to make in terms of what happens if one of the daemons goes down and stuff like
that."

### What exists

A tab holds one region per machine that has panes in it. Each region draws that machine's own tree,
and the regions sit side by side, in a list, at weights the window keeps (`architecture.md`, "The
core owns composition" and "The arrangement over regions is Muster's, and only Muster's"). Every way
of placing a pane follows that rule:

- **Split on Machine**, in a pane's right-click menu (`Sources/MusterMac/ContextMenus.swift`),
  splits right only. When the chosen machine already has a part of the tab, the new pane splits
  the pane that part's keyboard is on, not the pane clicked (`split_onto` in
  `crates/muster-seam/src/handler.rs`). When it has none, the machine joins the tab as a new
  region beside the clicked pane's region.
- **Dragging a pane** onto another machine's pane is refused in the shell (`PaneDrag.accepts` in
  `Sources/MusterMac/PaneDrag.swift`), and `muster pane move --onto` refuses it in the core
  (`arrange_pane` in `crates/muster-seam/src/session.rs`). Only a drop on a tab's caption, or
  `muster pane move --tab`, adds a machine's part to a tab.
- **`muster pane new --daemon devenv --down`** run in a laptop pane puts the new pane in devenv's
  region, not below the pane it ran from.

So a devenv pane can be left or right of the laptop's panes as a whole, and never below one of
them. The narrow fix, making Split on Machine split the pane clicked, cannot put a devenv pane
below a laptop pane either: when the clicked pane is on the laptop and the new one on the devenv,
no region holds both.

### Why it needs a MIP

The one-region-per-machine rule is a recorded decision, and its reason is recorded with it. The
reason was written on 2026-08-15 (commit `1ab45037`), when every tree belonged to herdr: "regions
are a list and not a tree - owning a tree over them is the multiplexer this project deliberately
is not." MIP-3 has since made the session daemon Muster's own, so the question has changed from
"should Muster keep a tree herdr does not have" to "which of Muster's daemons keeps it, and how
does the tree survive one of them going away". Durability depends on the answer, and so do
degradation, the saved arrangement, the protocol, and every placement path.

## Decision

### 1. One tree per tab, on every machine holding part of it

A tab's *part* on a machine is that machine's daemon's record of the tab; today's window draws
each part as a region. Under this MIP a tab's tree is the tree of the whole tab: every pane in it,
on every machine. Each part stores a copy of that whole tree. A tab with parts on more than one
machine is a *spanning tab*. In a part's copy a leaf is one of two kinds:

- **Its own pane**, as today: a pane id this daemon runs.
- **A foreign leaf**: a pane on another machine, as its id and the name of its machine. The daemon
  keeps it where it is, carries it through every save, restore and handoff, and never runs,
  checks or reports it. The id is what identifies the pane, since a pane id is unique across
  machines (`crates/muster-core/src/names.rs`). The machine's name is the one the writing window's
  config gives it (`devenv`), and another window may call that machine something else, so a
  window finds a leaf's machine by asking which attached machine holds the pane, and goes by the
  name only when none does: to say which machine it is waiting for, and to drop the leaf once
  that machine answers holding no such pane (section 2). A window never rewrites a copy only to
  change a name in it, since two windows naming one machine differently would then rewrite each
  other's copies for as long as both are open.

A tab held by one machine has no foreign leaves, and nothing about it changes.

Each copy carries a **layout generation**, a number that goes up with every write a window makes to
the arrangement, as a tab's `Label` generation does today. The zoomed pane, which `Tab.zoomed`
already records per part, becomes part of the tree and is copied with it, so every part records
the zoom even when the zoomed pane is on another machine.

### 2. The window merges the copies and writes them back

A window already reads every part of a tab it holds. It now merges their trees into one, in a pure
function beside `composition/view.rs`, and draws the result:

1. **Start from the copy with the highest generation.** Copies of equal generation differ only in
   panes their own machines added or removed (section 3), which steps 2 and 3 reconcile whichever
   copy is taken. The window takes the one naming the lowest pane id among its own panes, so two
   windows merging the same copies get the same tree whatever their configs call the machines. A
   copy whose machine has not yet said it holds any of its panes loses to one whose machine has.
2. **Drop leaves whose pane is gone.** A leaf goes when its machine is connected, has finished
   restoring, is not stopping, and holds no such pane in this tab. A leaf whose machine is
   unreachable, still restoring, stopping, or not attached to this window stays: only the machine
   running a pane can say it has gone, and a stopping machine's closes say only that it is
   stopping (section 7). So does a leaf whose machine holds a part of the tab that the window has
   not taken into the tab yet: the window hears a machine before it reconciles what it heard, and
   until then that part's panes are missing from the merge without being gone.
3. **Add panes the tree lacks.** A pane a machine holds in this tab that the starting copy does
   not name is put where that machine's own copy put it: beside the same neighbor, on the same
   side, at the same ratio. If that neighbor is not in the starting copy, the pane goes in as a
   column at the right.
4. **Write the result back** to every connected part whose copy differs in its tree or its zoom,
   though not in the names it gives other machines (section 1). When only copies behind the
   highest generation differ, the write catches them up at that generation; when a copy already
   at it differs, every write goes one above it. Each write names the generation it was based on,
   and a daemon whose copy has moved on since refuses it; the window then merges again from the
   newer copy.

This is how tab names already work: `rename_tab` writes one generation to every part and
`catch_up_tab_names` rewrites a part that lagged when its machine answers again (both in
`crates/muster-seam/src/session.rs`). The merged tree is derived and disposable, like the rest of
the mirror; the copies on the daemons are what is kept.

A window that changes the arrangement - a split across machines, a drag, a divider moved in a
spanning tab - works out the new tree itself, on the whole tree, foreign leaves and all, and writes
it to every part. It never drops a leaf it does not draw.

### 3. What a daemon still does on its own

In a spanning tab, a daemon changes its copy on its own only as its panes come and go: when one
of its panes exits, when it places a helper (`Placement.Helpers`, MIP-7), when it makes a pane
beside one of its own, and when a restore leaves one of its panes behind. None of these changes
the copy's generation. Each arrives at the window as an ordinary `TabChanged`, and the merge's
steps 2 and 3 work it out from the panes themselves, whichever copy step 1 started from, so
nothing a daemon does on its own can be lost to a window's write.

Every change to a spanning tab's arrangement is the window's write: a resize, a swap, a zoom, a
divider dragged, a pane moved. The window works out the tree and sends `SetTree`. A daemon asked
directly for a resize, swap or zoom in a spanning tab refuses, saying the tab's arrangement is the
window's to write, because a change it made itself would race the window's next write and could
be lost to it. A tab held by one machine takes those requests as today.

When a daemon's last own pane in a tab closes, its part closes, as today. The other parts still
name that pane until a window merges, and step 2 drops it then, because the machine is connected
and holds nothing.

### 4. The protocol

Protocol minor 7 adds:

- `Node.foreign { pane, daemon }`, the foreign leaf. `daemon` because that is the word `muster
  window --json` already uses for a machine.
- `Tab.layout_generation`.
- `TabRequest.SetTree { tab, root, zoomed, generation, based_on }`, the window's write, which
  leaves the tab at `generation`. Refused, with the reason, when `based_on` is below the part's
  generation, when `generation` is not above `based_on`, when the tree leaves out one of this
  daemon's panes in the tab, names a leaf twice, names as this daemon's a pane it does not hold
  or holds in another tab, or `zoomed` names no leaf. The checks on panes are what catch a pane
  that opened or closed on this daemon after the window read its copy: the window's tree then
  names one too few or one too many, the write is refused, and the window merges again from the
  copy the daemon's own `TabChanged` brings.
- `Placement.InTree { tab, root, generation, based_on, label, hidden }`, for a `Create` or `Move`
  whose resulting tree the window worked out: the pane goes in this tab, at the leaf in `root`
  that names it, under `SetTree`'s rules. When the daemon has no part of the tab yet, this makes
  one, called `label` and hidden when `hidden` says so, which today takes the window's `JoinTab`
  intent, sent as a `Placement.NewTab` of the tab's name. It leaves the zoom as it was: `SetTree`
  is the only write that changes it.
- `Event.stopping`, which a daemon told to stop sends once, before it closes anything. A
  stopping daemon closes every tab and pane it holds, as it always has, and its next run brings
  them back; without this event a window cannot tell those closes from panes somebody closed.

The state file gains the generation as a field with a default, but a foreign leaf is a new kind of
node, which a daemon from before it cannot parse at all: it would move the file aside as damaged
and start empty. So a file holding a foreign leaf is version 2 of the format, and a daemon from
before it refuses the file as newer, starting with no tabs and saving nothing over it (MIP-3,
section 2). A file with no foreign leaf is still written as version 1, so going back to an older
daemon costs nothing until somebody makes a tab spanning machines. A daemon handoff carries
`persist::Tab` whole (`crates/muster-daemon/src/handoff/mod.rs`), so it carries both without a
change.

### 5. What the window draws

- **One tree per tab.** `View` publishes the merged tree, with a machine and a socket on each
  leaf, where it published a list of regions with a tree each. A region's weight becomes an
  ordinary split ratio. A surface still belongs to its pane and is parked between tabs, so nothing
  about bridges changes.
- **Zoom** keeps its doctrine (`architecture.md`, "Where the keyboard is belongs to the window"):
  the pane the keyboard is on fills the tab, on whichever machine it runs. There are no other
  regions left to cover.
- **Moving the keyboard.** Next and previous walk the tree's reading order, which now interleaves
  machines. The four directions lay the tree out once rather than regions plus trees, so they
  get simpler. The ⌘ chords and the agent list number panes in the same reading order.
- **Dividers** between a laptop pane and a devenv pane are split ratios like any other. Dragging
  one in a spanning tab is a `SetTree` to every part. The window draws what a daemon says rather
  than its own result, as it does for every change (`architecture.md`: "nothing is rendered
  optimistically"), and the merge takes the copy with the highest generation, so the drag shows
  as soon as the fastest part answers. A tab whose parts are all remote drags at remote speed, as
  a remote tab does today.
- **Equalize** (`crates/muster-core/src/equalize.rs`) works over the whole tree, so it evens out
  panes across machines.

### 6. Each placement path, afterwards

- **Split on Machine** creates the pane on the chosen machine beside the pane clicked, on the side
  asked for, with `Placement.InTree`. Which sides the menu offers is question C.
- **`muster pane new --daemon devenv --down`** puts the pane below the pane it ran from, or below
  `--pane` when one is named, whatever machine either is on.
- **Dragging a pane onto another pane's edge** is accepted across machines. `PaneDrag.accepts`
  keeps refusing a drop onto the pane itself and a drop from another window, and stops comparing
  machines. Rearranging around an unreachable machine's panes works, since only the reachable
  parts are written (section 7). The core refuses only what needs the unreachable machine to act:
  making a pane there, or moving one of its panes into another tab.
- **Dropping a pane on an agent-list row in another tab** puts it in that tab, beside the row's
  pane, with `Placement.InTree` when its machine has no part there.
- **`muster pane move --onto`** with a side is the same move as the drag. Without a side, two
  panes in one tab exchange places, which across machines is two leaves trading places in the
  tree, and a pane in another tab joins it as the row drop does.
- **A drop on a tab caption**, and `muster pane move --tab`, put the pane in as a column at the
  right of that tab's tree, which is where they put a second machine's region today.

### 7. When a machine restarts, hands over, or goes away

- **A daemon restarts.** On its way down it says it is stopping and then closes every tab and pane,
  and the window keeps that machine's leaves in every other part's copy, since the closes are of
  panes its file holds (section 4). Its file holds the whole tree, foreign leaves included. It
  restores its own panes, keeps every leaf's place, and drops the leaves of its own panes that did
  not come back. What the window says about a restart (`crates/muster-core/src/mirror/restart.rs`) counts
  only the daemon's own panes, as now.
- **A daemon hands over** to a newer one (MIP-3, section 10). The tree is part of the tab record
  the handoff carries, so the tab is the same shape on the other side.
- **A machine is unreachable, for any length of time.** Its panes keep their places, drawn stale
  with their last frames, which is what `architecture.md`'s "Degradation" already asks of a stale
  daemon. The other parts' copies still hold the whole tree. Rearranging the reachable panes
  meanwhile writes the reachable parts; the unreachable part catches up when it answers. If both
  sides changed the arrangement while apart, the copy with the higher generation wins, which is the
  side that wrote more changes rather than the one that wrote last, and the other side's changes to
  the arrangement are lost. No pane is lost, because only its own machine can drop it.
- **A window opens while a machine is down.** The reachable parts carry the whole tree, so the
  window knows where the missing panes go, but it has no frames to draw in their places. What it
  draws there is question A.
- **A pane closes on one machine.** That daemon drops its leaf, and the window writes the result
  to the other parts. With no window open, the other parts keep a foreign leaf for a pane that has
  gone; the next window to merge drops it.
- **A machine leaves the config, or is gone for good.** Question B.
- **A tab is closed while one of its machines is unreachable.** MIP-3 section 2 already answers
  this, and the answer stands: that machine's part comes back as a tab of its own when it
  answers, because its processes are still running. Its copy of the tree still names the other
  machines' panes, which have closed, so the first merge drops them.
- **A window is attached to only some of a tab's machines**, such as a second laptop's window
  attached to the devenv alone. It draws the panes it can reach, leaves the others out of its
  view, and keeps their leaves in every tree it writes.

Hidden tabs and helpers are unchanged. Every part of a tab is hidden or shown alike, so a pane
made from a hidden pane onto another machine is hidden there too, and `Placement.Helpers` adds a
helper as a column after the others, which works the same in a spanning tree.

### 8. `muster window --layout` and `--json`

`--layout` draws one tree per tab, and each box names its machine whenever more than one is
attached, as boxes already do. A pane on an unreachable machine is drawn in its place and says
`unreachable`. A zoomed tab still draws the one pane filling it and names the others under it.

`--json` gives each tab a `layout`, the tab's whole tree, with `daemon` on each leaf. A list of
regions cannot describe a tab whose machines interleave; what happens to `regions[]` and
`tabs[].regions` is question D. Pane rows keep their `rect`, which already says where each pane
ended up.

### 9. Spanning tabs made before this, and daemons older than it

**A spanning tab made before this** has region weights in its window's arrangement, and every
part of it is at generation zero. The merge starts such a tab from its regions in order, each
region's tree as a column, the weights as the columns' ratios, which is how the window drew it.
Writing that tree to the tab's parts waits until the window sends every change to a spanning
tab's arrangement as a whole tree, because a part holding foreign leaves refuses the resize, zoom
and swap that a window sends one machine at a time (section 3, and "How it lands"). From the
first write on, the arrangement file stops carrying that tab's weights.

**A daemon older than this minor** keeps its own tree and cannot hold foreign leaves. The window
never writes to it; its panes are placed in the newer parts' copies, wherever somebody puts them,
and step 3 of the merge puts one the newer copies have never named as a column at the right, which
is today's arrangement. A pane created on it beside a foreign pane is created with the placements
it knows, and then placed in the newer copies where it was asked for. It never says it is
stopping, so a window takes its closes on the way down at their word and drops its panes from the
newer copies; they come back as columns at the right once it restarts. Only a daemon somebody runs
themselves, at a socket named in the config, can be older for long, because Muster installs its
own build on every machine it reaches over ssh. A tab whose parts are all on such daemons has
nowhere to keep a spanning tree, so it is arranged as today, and a placement that needs one, such
as a devenv pane below a laptop pane there, is refused, naming the daemon to update.

### 10. Doctrine that changes

When this is accepted and built, these change with it:

- `architecture.md`, "The core owns composition": mixing stops being at tab granularity, and "a
  region per machine ... side by side" becomes one tree per tab, kept by its daemons. The line
  that a pane can never move between daemons stays true of its process.
- `architecture.md`, "The arrangement over regions is Muster's, and only Muster's": goes. The
  window still owns which tabs it holds and in what order; the shape inside a tab is daemon state.
- `architecture.md`, "A drop across daemons is refused in the shell": goes, replaced by section 6.
- `architecture.md`, "Durability" and "Degradation": "Only the order and widths of the tab's
  regions are Muster's" and "halves of tabs go back where they were, with their widths" become
  the merge of section 2. "A grouped tab is whole again as soon as both daemons have spoken"
  stays true.
- `AGENTS.md`'s "Not built" line, which names exactly this. Its non-goal "Not a multiplexer"
  stays as written: the tree lives in a daemon that "exists only to serve Muster and has no
  interface of its own".
- `crates/muster-core/src/composition/mod.rs`'s module comment, and `docs/cli/window.md`'s
  `regions[]`.
- MIP-2 said a pane belongs to one machine and "`pane move` refuses across machines and will keep
  refusing". The process still belongs to one machine; the refusal lifts for a pane's place.

## Rationale

**The daemons keep the tree because nothing else may.** `AGENTS.md` holds that the app owns no
truth, and `architecture.md` that no tab depends on a file of Muster's. MIP-3 section 2 ended the
case where a spanning tab existed only in `~/.muster/state/names.toml`, and a tree in the window's
arrangement file would bring that case back. It would also have to move with a tab handed to another
window (MIP-6), and a window attached to only some of the machines would never see it.

**Every part keeps the whole tree, rather than one part keeping it for all.** Which machine would
be down is not knowable in advance, and one machine being down is the case amterp raised first.
With a copy on every part, any one machine that answers restores the tab's whole shape, and the
window's merge needs no notion of which part is in charge.

**The window merges because only the window sees every machine.** No daemon knows another
exists (`architecture.md`, durability), which is why the window already reconciles tab names
across parts. A merge that drops a pane only on its own machine's word, and otherwise keeps the
highest-generation arrangement, cannot lose a pane, and can lose only an arrangement change made
on both sides while the machines could not reach each other. Then the panes are all still there,
in the places one side last put them.

**Whole trees rather than edits.** A tree is a few hundred bytes. Writing the whole tree with the
generation it was based on is idempotent, needs no ordering between machines, and gives a lagging
part everything in one write.

**The multiplexer non-goal is about an interface, not a tree.** The worry in commit `1ab45037` was
Muster growing a session layer of its own over herdr's. Muster has since built its own session
daemon and drawn the line in `AGENTS.md`: a daemon that exists only to serve Muster and has no
interface of its own. A tree in that daemon naming panes it does not run keeps to that line.

## Alternatives Considered

**The tree in the window's arrangement file.** The smallest change: the core already keeps
region weights there. Rejected for durability, as the Rationale says, and because the window
would then hold the tree that every daemon's own change must be checked against, and a daemon
with no window open could check nothing.

**One home daemon keeps the tree; the other parts keep membership.** Nothing to merge. Rejected
because the tab loses its shape whenever the home machine is down, which is the case that most
needs it, and because the other parts would still need a tree of their own to restore from.

**An outer tree over each machine's subtree.** Muster would keep a tree whose leaves are regions,
so a devenv region could sit below the laptop's region. It cannot interleave: laptop pane A, devenv
pane B below A, and laptop pane C to the right of both needs A and C in one laptop subtree with B
between them. It also keeps the outer tree in Muster, with the durability problem of the first
alternative.

**Replicating edits rather than trees.** Each daemon would apply the same split or move. Rejected:
edits need an order across machines that nothing provides, and a part that missed one needs every
edit since, where a tree write needs only the latest tree.

**The narrow fix**: Split on Machine splitting the pane clicked rather than the other machine's
part. amterp asked for the general form instead, and the narrow fix cannot put a devenv pane
below a laptop pane at all (Context, "What exists").

## Consequences & Trade-offs

- **A daemon holds facts about panes it does not run.** Foreign leaves are inert, but they are new
  in the state file and the protocol, and a daemon's copy can name a pane that closed while no
  window was open, until one opens.
- **Going back to an older daemon loses its tabs** once any of them spans machines, because the
  older daemon refuses a version-2 state file (section 4). It overwrites nothing, so going
  forward again brings them back.
- **More writes across the ssh link.** Every change to a spanning tab's arrangement is one
  `SetTree` to each part, so a divider drag in a tab spanning the laptop and the devenv sends
  each drag step to both. Tabs held by one machine send nothing new.
- **An arrangement changed on both sides while two machines could not reach each other keeps
  only the changes of the side that wrote more of them** (section 7).
- **A breaking change to `muster window --json`**, unless question D keeps `regions[]` for a while.
- **The shell's view changes shape.** Regions laid out side by side become one tree. This is the
  largest single change here, and stage 3 of How it lands is that change alone.
- **What gets simpler.** Zoom covers nothing but the tab, the four directional moves lay out one
  tree, and the saved arrangement stops carrying widths. A spanning tab stops being a special case
  of the view.

## How it lands

Each stage lands on its own with the suite green, and each is one worker's assignment.

1. **The daemon keeps a spanning tree.** Foreign leaves, the generation, `SetTree`,
   `Placement.InTree`, the state file's version 2 and handoff, and the protocol minor. Nothing in
   the window changes. Tests, in the daemon's suite: a restart keeps every foreign leaf in place; an
   own pane closing keeps them; `SetTree` refuses a stale base and a tree that leaves out or doubles
   an own pane; a resize, swap or zoom asked of a daemon in a spanning tab is refused; a checked-in
   state file with foreign leaves loads as written; the protocol compatibility test across the
   minor.
2. **The window merges and writes.** The merge as a pure function; `View` publishing the tree of a
   tab that spans machines beside the regions it publishes now, so the shell is untouched and a tab
   on one machine costs nothing more; and the write-back, to tabs a window has already written as a
   spanning tree. A spanning tab made before this is merged from its regions, side by side at their
   weights, and not written: no window writes one until stage 3, so in real use this stage writes
   nothing. Tests: conformance cases for each merge rule and for the tree the view publishes; seam
   tests against two real daemons: a lagging copy caught up, a pane closing on one machine dropped
   from the other's copy, a restarted machine needing nothing written, and a tab grouped before
   this left unwritten.
3. **The shell draws one tree.** The Swift view renders the per-tab tree and stops reading regions,
   so `View` publishes the tree for every tab, a tab on one machine taking its one copy as it is
   without merging; dividers and zoom across machines; `--layout` and `--json` move to the tree,
   per question D. In the same stage the window sends every change to a spanning tab's arrangement
   as `SetTree` - resize, swap, zoom, divider ratios and equalize - and switches on writing
   spanning tabs made before this, since from the first write on a part refuses those requests sent
   one machine at a time. Tests: the Swift view tests, the CLI's `diagram.rs` unit tests, a
   conformance case per arrangement change written as a tree, `docs/cli/window.md`.
4. **Every placement path.** `split_onto` beside the pane clicked on any side, `pane new
   --daemon` with a side, cross-machine drags and `pane move --onto`, the menu per question C,
   and the doctrine in section 10, since this stage makes `AGENTS.md`'s "Not built" line false.
   Tests: handler tests and a conformance case per path, `PaneDrag` tests in Swift.
5. **Unreachable and departed machines.** What questions A and B decide. Until it lands, a window
   opened with a machine down leaves that machine's panes out and puts them back when it answers,
   which is what it does with that machine's region today. Tests: conformance cases for a window
   opening with one machine down and for a machine leaving the config; an `--ssh` tier case
   restarting the devenv's daemon under a tab that interleaves it with the laptop.

Stages 1 and 2 change nothing a person sees. Stage 3 changes how every spanning tab is drawn
without changing where anything is, and stage 4 is the one that delivers what was asked for.

## Open Questions

- **How often a divider drag writes.** Section 5 writes each drag step to every part. Whether the
  remote part should get only the settled ratio at the end of the drag is a measurement for
  `./dev --perf`, not a decision.

## References

- kan `a_2lvMf2JeD` (this).
- MIP-2: the window, the tab and the pane, and stage four, where a tab came to span machines.
- MIP-3, section 2: what the daemon holds, a spanning tab recorded on both daemons, and the state
  file's rules; section 10: handoff.
- MIP-6: tabs handed between windows of one process.
- MIP-7, section 2: `Placement.Helpers`.
- `docs/architecture.md`: "Ownership of truth", "Degradation", "Durability".

## Decisions for amterp

**Question A. What a window opened while a machine is down draws where that machine's panes go.**

- (a) A placeholder in each pane's place, naming the pane and saying its machine is unreachable.
  When the machine answers, each placeholder becomes its pane and nothing else moves.
- (b) The space closed up until the machine answers, and then its panes return to their places,
  moving the reachable panes back to make room.

Recommendation: (a). It is the same picture as a machine that drops mid-session, and nothing
moves under somebody who is typing when the devenv comes back. It costs room on screen for panes
that cannot be used yet.

**Question B. How a machine's panes leave a tab when the machine is gone for good.** This assumes
(a) in question A; with (b), the only way out is the config.

- (a) Both of: a placeholder offers "Remove from Tab", and taking the machine out of the config
  removes its panes from every tab the window holds. If the machine ever answers again, its part
  comes back as a tab of its own, as MIP-3 section 2 does for a tab closed while a machine was
  unreachable.
- (b) Only taking the machine out of the config.
- (c) Only the placeholder's "Remove from Tab".

Recommendation: (a). (b) leaves a placeholder somebody can see and not remove without editing a
file, and (c) leaves a removed machine's placeholders in every tab until each is removed by hand.
(a) costs one more item on the placeholder and one more thing a config edit does, which somebody
taking a machine out of the config for an afternoon may not expect.

**Question C. What the right-click menu offers for putting a pane on another machine.** The pane
menu already has Split Right, Split Down, Split Left and Split Up.

- (a) "Split on Machine", then the machine, then Right, Down, Left and Up. One entry point, every
  side, and a split on the same machine stays one click.
- (b) A submenu per side: "Split Right on", "Split Down on", and so on, each listing the
  machines.
- (c) One submenu that splits right, as now, with dragging for every other side.

Recommendation: (a). (b) puts four submenus in a menu that has one, and (c) leaves three sides
reachable only by dragging, which a person looking in the menu for them will not think to try.
(a) costs a third level of menu for every split onto another machine.

**Question D. Whether `muster window --json` drops `regions[]` at once.**

- (a) Drop `regions[]` and `tabs[].regions` for one tree per tab with a machine on each leaf, and
  say so in the release notes.
- (b) Keep `regions[]` for a release beside the tree.

Recommendation: (a). A list of regions cannot describe a tab that interleaves machines, so for
exactly the tabs this makes, (b) would print an answer that is wrong. Its cost: scripts that read
`regions` break.

Decided 2026-10-23: (a). amterp left the choice to an agent, which took the recommendation. The
release that ships stage 3 says in its notes that `regions[]` is gone and what replaces it.

---

## History
- 2026-10-21 Draft.
- 2026-10-23 Question D decided: drop `regions[]` at once. Stage 3 is unblocked.
