---
mip: 2
title: Muster's own units - the window, the tab and the pane
status: Accepted
kind: Architecture
created: 2026-08-31
decided: 2026-09-02
supersedes:
superseded-by:
related: 1
---

# MIP-2: Muster's own units - the window, the tab and the pane

## Summary

A window, a tab and a pane are Muster's own units, and none of them takes its shape from herdr.
Muster mints their names, decides what they hold, and translates whatever arrangement somebody
asks for into whatever the backend has to be told.

Two of the three are already partly Muster's. A pane has a Muster name and a tab has one, minted
by the registry in `crates/muster-core/src/names.rs`. What is not Muster's is the shape: a tab
belongs to one machine because it *is* one daemon's tab, a window is not a unit at all, and the
record the window is rebuilt from carries herdr's workspace and herdr's tab into a file Muster
owns.

What follows from that, in use:

- **A tab holds whatever panes you put in it**, laptop and devenv side by side if that is what
  the work needs.
- **A window holds whatever tabs you put in it**, and two windows are two workspaces rather than
  two views of one.
- **A pane moves between tabs by drag or by command**, without ending what is running in it.
- **A pane stays on its machine.** It is a process, and processes do not move between machines.
  This is the one limit that is not herdr's to lift, and it stays stated so nobody builds on the
  opposite.

One thing this proposal does not deliver, and it is worth naming here rather than in a footnote:
**a pane cannot be shown by two windows at once.** herdr allows one client per terminal, and no
amount of Muster-side vocabulary changes that. `a_2IZ6Of6JP` holds what is known about handing a
pane from one window to another, including the measurement that makes it cheap.

## Context / Motivation

### What was asked for

Driving a laptop and a devenv in one window (kan `a_2HtF52Itm`), with one local tab and one
devenv tab: *"I would prefer these not as two parallel panes but genuinely two different
tabs"*, wanting `cmd+1` and `cmd+2` to switch between them the way tabs switch everywhere else.
And on where the model should come from: *"we shouldn't let ourselves be constrained by
herdr... I would want a very clear API boundary that translates between Muster modelling and
herdr modelling."*

Stated again a day later (`a_2IZPRaJvB`), after driving two machines for a day: *"windows/tabs/
panes must be concepts that Muster itself has its own definitions of, completely un-dictated by
herdr. They are simply organizational units."*

Neither is expressible today, and the second is the reason the first is not.

### The model today, in one sentence

**The window is a column per machine, each column showing one of that machine's tabs.** Not
"tabs, one of which fills the window". Everything else in this section follows from it.

Measured, with an arrangement cut down to a single region naming the local tab:

    saved:     1 [[region]]  (daemon = "local")
    after:     view.received  regions=3  panes=3
    muster window: local Tab 1 "on screen", devenv Tab 2 "on screen"

`open_remaining_regions` opens a region for every attached daemon that has none, and its guard
is per daemon, so a machine holding a tab always gets a region and two machines are always both
on screen. (The third region was `a_2Ht74jTXV`, fixed separately.)

Within one machine the behaviour asked for already exists: that machine's second tab is not on
screen, its panes are marked `(hidden)`, and focusing it swaps what that machine's column
shows. So Muster already has "tabs take turns". It scopes it to a machine.

### What that costs a person

`cmd+2` means "switch this machine's column to that tab" when the tab belongs to a machine
already showing, and "move the keyboard to that other machine's column" when it does not. Same
chord, same-looking rows, two acts. Marking the tabs that share the screen (done in the 0.4.0
round, in the sidebar) lets somebody predict which they are about to get. It treats the symptom.

The caption is the other symptom. `tab_label` writes `Tab <place>` counting across the whole
window, because herdr labels an unnamed tab by its position inside its workspace and two
daemons would each contribute a `1`. So `Tab 1` and `Tab 3` sit side by side on one screen while
every other terminal and every browser has trained the reader that tabs are what you switch
between.

### And what it cost a window

Added 2026-09-02, from a second day of driving two machines. The window turned out to be the
sharper case, because it is not a unit at all.

`muster window new` opened onto the panes the first window was already rendering, refused every
bridge, and left a dead copy of the layout (`a_2IZ5TL6DQ`). Two things caused it and only one was
the obvious one. Both windows read and wrote a single `~/.muster/state/window.toml`, so a second
window restored the first one's regions and whichever published last decided what came back next
launch. Underneath that, a window opening with no saved arrangement at all would have landed in
the same place: the standing rule gives every machine a region on whatever tab that machine last
had focused, and that is the tab the other window is showing.

Both are fixed (2026-09-02), and the fix is a rule rather than a model: one window owns the
arrangement, and a window somebody asked for opens onto tabs of its own. That is enough to stop
anybody meeting the failure by accident and it is not the unit this MIP is about - a window still
has no identity of its own to remember anything under, which is why a second window's arrangement
cannot be brought back at all.

## Where herdr's model got into the core

This is the MIP's first job, and the answer is: through the composition record, and from there
onto disk.

Written in the present tense as it was on 2026-08-31, and two of the three ways in are closed
since: `Region` and the saved arrangement no longer carry a workspace, and `BackendIntent`
no longer names one. The tab is the way in that remains, and the one the rest of this is about.

**The glossary says it outright.** `docs/glossary.md`: *"**tab** - the unit that owns
one pane tree, inside a workspace; daemon truth."* Muster's own vocabulary defines its own
noun as somebody else's.

**`Region` names a daemon's workspace and tab.**
`crates/muster-core/src/composition/record.rs`:

```rust
pub struct Region {
    pub id: RegionId,
    pub daemon: DaemonId,
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub weight: f32,
    pub pane: Option<PaneId>,
}
```

The composition is described as "the Muster-owned arrangement". Three of its six fields are
herdr's nouns. `WorkspaceId` reaches `composition/record.rs`, `composition/saved.rs` and
`intent.rs` from `mirror/backend.rs`, which is where a backend's vocabulary is supposed to
stop.

**And it reaches disk.** `composition/saved.rs` writes `daemon`, `workspace` and `tab` into
`~/.muster/state/window.toml`. A herdr workspace id is in a file Muster owns, so a window
reopening reads herdr's model back before it has spoken to a daemon.

**The intent carries one too.** `BackendIntent::CreateTab { workspace, .. }`, resolved in the
seam before submitting, with this reason on the handler: *"`tab.create` takes a workspace and
ignores keys it does not know, so a request that named the pane instead would be accepted and
put the tab wherever that daemon last had focus."* That is a real defence against a real herdr
behaviour, and it belongs in `muster-herdr`, not in the core's intent type.

**The outward surface has already drawn this line.** `proto/muster.proto` on `CreateTab`: *"A
workspace is deliberately not here. It is herdr's unit for a whole project rather than
something a window makes several times an hour."* So Muster's CLI and API already refuse to
expose a workspace while Muster's own record is built on one. The line exists; it is drawn one
layer too far out.

The swappable-organs desideratum fails here, in the expensive place. An adapter can absorb a
difference at the edge; this difference sits in the record the window is rebuilt from, and in
the file that record is read back out of.

## Decision

### A Muster tab

**A Muster tab is a named set of panes that a window shows together, one tab at a time.** Muster
mints its name the way it mints a pane's, and `t1w3r07bsd` already is that name. A window holds
an ordered list of them; exactly one is on screen; `cmd+1` to `cmd+9` and `next_tab` walk that
list.

**A pane belongs to exactly one machine. A tab does not.** A pane is a PTY and it lives where
it lives, which is why `pane move` refuses across machines and will keep refusing. A tab is a
grouping Muster made, so one tab holding a laptop pane beside a devenv pane is something Muster
can express.

**Every backend tab is a Muster tab of one machine until somebody groups it.** That is the
migration and the durability answer in one sentence: a Muster tab that holds one machine's panes
maps to exactly one herdr tab, so nothing about the common case changes, and only a tab somebody
has deliberately made span two machines depends on Muster's own file.

**A region divides a tab, not the window.** Regions stop being "one per machine" and become
what a split makes: the parts of the tab currently on screen. Region weights and boundaries
keep their present meaning inside a tab.

### A Muster window

**A window is a unit with an arrangement of its own.** What that buys is the thing a window
cannot do today: remember anything. Arrangements move from one `~/.muster/state/window.toml` to a
file per window, so two windows have two arrangements rather than one they overwrite in turn, and
a window that was closed has a record to be reopened from. Built 2026-09-02.

The records are numbered rather than named from the registry that mints pane and tab names, and
that is a limit worth stating rather than a choice: the registry is the core's, and which file to
hand the core is asked before the core is running. A window therefore has an arrangement of its
own and no name of its own, which is why nothing lists the windows you have closed - only the
most recent comes back. Giving one a name means minting it somewhere the shell can reach before
startup, and nobody has asked for that yet.

**A window somebody asked for is a different launch from the window Muster comes back to**, and
the launch says which. That rule is built (2026-09-02) and stands whether or not the rest of
this lands: `muster window new` and ⌘N pass `--fresh`, and a fresh window remembers nothing and
opens onto tabs of its own.

### A Muster pane

The pane is the unit that is already Muster's, and what this MIP does to it is remove the last
place it is not. A pane's *destination* was herdr-shaped: `pane move` could only name a tab that
already existed, because a drag onto a row was the only gesture it had. It now says where in
Muster's words - beside another pane, or in a tab of its own - and the adapter picks herdr's tag
(built 2026-09-02, `a_2IXGSgZi7`).

What stays refused is a pane moving between machines, and that is not herdr's limit to lift.

### The adapter owns the mapping

`muster-herdr` translates a Muster tab into the herdr tabs it needs, one per machine that has
panes in it, each inside whatever workspace that machine supplies. The core stops naming a
workspace at all: `BackendIntent` says "a tab beside this pane" and the adapter resolves which of
herdr's workspaces that means, keeping the defence quoted above where herdr's behaviour is known.

### The saved arrangement stops carrying herdr's ids

It names Muster's tabs and Muster's panes, both of which come from the registry that already
survives a restart.

### What it looks like

The sidebar reported in `a_2HtF52Itm`, with the devenv agent and the local shell side by side on
screen and the local one called `Tab 3`:

    DEVENV                            Tab 1   memri · claude   (devenv)
      1  Tab 1     memri · claude     Tab 2   memri            (devenv)
      2  Tab 2     memri              Tab 3   dotfiles         (local)
    LOCAL
      3  Tab 3     dotfiles

On the right, one of the three is on screen at a time, `cmd+1` to `cmd+3` switch between them
the way tabs switch everywhere else, and nothing stops a fourth tab holding the devenv agent
beside the local shell.

## Rationale

**The user's question is not answerable inside the current model.** "Make these two tabs take
turns" is a request about tabs; today it is a request about regions, and regions are per
machine because tabs are per machine. Any fix at the sidebar leaves the list describing a
window it no longer matches.

**The seam is where this belongs, and the project already says so.** AGENTS.md: *"the core
speaks Muster's own vocabulary, each dependency lives in one adapter"*. A tab is the noun a
person uses most after a pane, and it is the one Muster does not own.

**It costs less now than later.** Every arrangement feature added between now and then is
written against `Region { daemon, workspace, tab }` and will have to be rewritten with it.

## Alternatives Considered

**Leave it, and make the sidebar honest.** Mark which tabs are on screen together, keep
everything else. Done in the 0.4.0 round, and it is the right thing to have done whatever
happens next: it makes `cmd+2` predictable today at the cost of nothing. Rejected as the whole
answer because it does not make the request expressible. Somebody driving two machines learns
the model in a day; the question is whether the model is the one we want.

**Label by machine instead of grouping by it.** Move `LOCAL` / `DEVENV` from a heading onto the
row, as a badge or a tint, and drop the top-level split. Rejected on its own: the sidebar groups
by machine because the *layout* does, so moving the machine onto the row without changing what a
region is leaves a list that no longer describes the window beside it, which is worse than the
mismatch it set out to fix. It is a consequence of this MIP rather than an alternative to it.

**Let a region show any machine's tab, keeping tabs per machine.** `cmd+1` and `cmd+2` would
then switch across machines, which is most of what was asked for, and it is a smaller change:
`Region` keeps its fields and only `open_remaining_regions` and the focus rules move. Rejected
because it stops one step short of the thing that makes the model coherent. A tab still could
not hold panes from two machines, so "which machine" would remain the axis while no longer
being the axis the window is drawn on, which is the worst of both.

**Keep herdr's tab and add a Muster grouping above it.** A "view" or "layout" that names
several herdr tabs. Rejected as the same thing with two nouns: a person would have Muster tabs
and herdr tabs to think about, and every message would have to say which.

**Give a window a pid-named arrangement, the way the command socket is named.** The socket
carries a pid so a caller can reach the window it means, and copying that would give each window
a file of its own in one line. Rejected because a pid is not an identity: it is different next
launch, so a window could remember something and never be the window that reads it back, which
is the one thing a per-window arrangement is for. A name from the registry survives a restart
for the same reason a pane's does.

## Consequences & Trade-offs

### Durability, which is the question this was waiting on

**A tab spanning two machines is a shape no single daemon can write down.** Today the daemon
holds the tab, which is why AGENTS.md can say *"sessions outlive the app, and their shape
outlives the daemon"*: quitting Muster costs nothing, and a Muster that never comes back costs
nothing either, because herdr still has the tab and its tree. A cross-machine tab can only live
in Muster's own state, which turns a daemon guarantee into a Muster one.

**Confirmed, and the answer is to degrade per tab rather than wholesale.** A tab whose panes are
all on one machine maps to one herdr tab, so the common case keeps the guarantee it has now -
including every tab that exists on the day this lands, since a backend tab is a Muster tab of one
machine until somebody groups it. Only a tab that actually spans machines depends on Muster's
file, and only that tab is lost if the file is.

What that costs is a guarantee with two tiers, and it has to be written down where somebody
looks for it rather than discovered: `docs/architecture.md`'s durability table and
`docs/glossary.md`'s entry for `tab` both say which tier a tab is in and why.

Two states it has to answer either way:

**A daemon restarts.** herdr returns the pane tree and each pane's directory, not the
processes. A single-machine tab comes back as it does today. For a cross-machine tab, Muster's
file says which panes were together; the panes themselves come back per machine, so the tab is
reassembled from two halves and is whole as soon as both daemons have spoken.

**One of a tab's machines is unreachable.** The tab has to open showing the panes it can reach
and say the others are missing, rather than refusing to open. That is the same rule the mirror
already follows for a stale daemon (`architecture.md`, degradation), applied to a tab.

### What gets easier

The sidebar becomes a flat list of tabs with their panes under them, which is what a reader
already expects. `tab_label` stops needing a place counted across the window to be unique,
because Muster mints the name. `open_remaining_regions` stops existing in its present form:
there is no "give every machine a column", which also answers `a_2I6h18OU6` - closing a
machine's last pane stops refilling it, because nothing is owed a column.

### What gets harder

Every rule that today says "this daemon's region" has to be restated in terms of a tab that may
touch several daemons. `reconcile` is per daemon on purpose, because streams from different
daemons have no mutual order, and a tab spanning two of them has to be reconciled without
introducing a cross-daemon ordering that does not exist.

### How the shakedown fixes survive it

**The sidebar's on-screen mark** answers a question the new model deletes: the mark existed to
say which tabs shared the screen, and nothing shares it any more. What it became instead is the
ordinary current-tab mark every tab strip has, on the one tab the window is showing - a flat list
of tabs with nothing saying which one you are looking at would be worse than the mismatch the
mark was added to fix.

**One tab collapsing the numbered chord (`a_2Hx68fXqr`)** survives unchanged and gets more
honest. It reads the roster's tab list, which becomes the list of Muster tabs, and "a window
holding one tab numbers panes" means a window genuinely holding one tab rather than one tab per
machine.

**Which machine a request acts on (`a_2Hwef7lQT`, `a_2HpkpfIfq`)** survives, because it is
already expressed in the terms this MIP moves towards: a pane's name says which machine holds
it, and `--daemon` names a machine directly. What needs restating is `focused_pane_on`, which
today means "the pane that machine's region has the keyboard on" and has no meaning once
regions are not per machine. It becomes "the pane the keyboard is on if that is on the named
machine, and otherwise that machine's first pane in the tab on screen".

**The pane drawn twice (`a_2Ht74jTXV`)** is composition code and the most exposed. Its guard is
about not opening a second region for something already shown, and the restore path is where it
went wrong. Both are rewritten by this. Whatever replaces them has to keep the property that
card established: one surface per pane, decided in the composition rather than at the bridge.

**A window says on screen the panes it is drawing (`a_2Ibz6NXjV`, fixed 2026-09-02)** is the one
to carry rather than rewrite. Its rule - a region shows a tab, and a tab on screen has its panes
on screen unless a zoom covers them - is stated in terms this MIP does not change, and it is
pinned in all 51 cases of `composition.json`.

## How it lands

Four changes, each green on its own, in this order. The first two are the ones that need nobody
else's cooperation and no behaviour change at all.

1. **The core stops naming a workspace.** `Region`, `SavedRegion` and `BackendIntent::CreateTab`
   drop `WorkspaceId`; the adapter resolves which of herdr's workspaces a tab means. The saved
   file's version goes up. Nothing a person can see moves. Built 2026-09-02.

2. **A window gets an arrangement of its own.** Per-window records under
   `~/.muster/state/windows/`, and with them a window that was closed can be reopened - the other
   arrangement gap in `a_2Ic6mB36E`. Built 2026-09-02.

3. **A window holds an ordered list of Muster tabs, one on screen.** Regions become the parts of
   that tab. `cmd+1` and `cmd+2` switch between a local tab and a devenv tab. The sidebar
   flattens. This is the change the original card asked for. Built 2026-09-03.

4. **A Muster tab holds member tabs on several machines.** The only stage that turns a daemon
   guarantee into a Muster one, and the last to land for that reason. Built 2026-09-03.

**Three and four land together or not at all**, and that is the one sequencing constraint here.
Before them a window showed every machine at once, side by side. Stage 3 alone would replace that
with one tab filling the window, and grouping - the thing that puts a laptop pane beside a devenv
pane again - is stage 4. Landing 3 on its own would take away an arrangement people are using and
give back nothing until 4 arrived. They landed in one branch.

## Open Questions

All settled, with stages three and four (2026-09-03).

- **Where does the machine go on a row?** On the pane row, at its trailing edge. A tab row says
  nothing about machines: a tab may span two, so any single answer there would be wrong for some
  of its panes.
- **How does the machine stay quiet?** It is not drawn at all while one machine is attached,
  which is the common case and leaves that window reading exactly as it did. Labelling only the
  machines that are not this one was the alternative and costs more than it saves: with two
  devenvs attached, a reader has to already know that a blank means local.
- **Where do a machine's own states go?** A machines section at the foot of the agent list,
  holding a row per machine that is unreachable or holding no panes. A machine that is connected
  and holding panes contributes no row - its panes carry its name. Picking a row there asks for a
  pane on that machine, which is the affordance `a_2HpkpfIfq` deferred and what makes deleting
  the refill rule safe rather than a reopening of the hole that card closed.
- **What does `muster window --json` say?** `regions[]` keeps its name and becomes the parts of
  the tab on screen - `region`, `daemon`, `pane`, `weight`, `keyboard`, `zoomed` - and drops
  `tab`, because they all show the same one. A top-level `showing` names that tab. `tabs[]` drops
  `daemon` and gains `daemons`, the machines it holds panes on.
- **What happens to an existing `~/.muster/state/window.toml`?** Neither answer this offered. A
  version 3 arrangement becomes **one Muster tab holding all of it**, so the first launch after
  this lands looks like the last launch before it - splitting that into a tab per machine is
  something to do afterwards and on purpose, once the new model is on screen to do it in. The
  file goes to version 4 and version 3 is read rather than refused.

Two questions the Draft carried were settled earlier. **A Muster tab maps to a herdr tab per
machine**, not a workspace per machine: a workspace is herdr's unit for a whole project, and
Muster's own schema already refuses to expose one. And **`pane move` across machines stays
refused** while "move this pane into that tab" gains an answer that can span machines - the two
are different requests and the vocabulary keeps them apart, which is why the destination is an
enum rather than a second meaning for the same field.

## References

- kan `a_2HtF52Itm` - the observation, the measurements, and the direction.
- kan `a_2IZPRaJvB` - the same direction stated for all three units, a day later.
- kan `a_2IZ5TL6DQ`, `a_2IZ6Of6JP`, `a_2IXGSgZi7`, `a_2I6h18OU6` - what the window and the pane
  cost in practice.
- kan `a_2Hx68fXqr`, `a_2Hwef7lQT`, `a_2HpkpfIfq`, `a_2Ht74jTXV` - the shakedown fixes this has
  to survive.
- MIP-1 - the seam this argues is in the wrong place for one noun.
- `docs/architecture.md`, durability and degradation.
- `docs/glossary.md`, which defines `tab` as daemon truth.

---

## History
- 2026-08-31 Draft
- 2026-09-02 Accepted, and widened from the tab alone to the window and the pane. The durability
  question the Draft was waiting on is answered: degrade per tab, with every tab that exists today
  in the tier that keeps the guarantee.
- 2026-09-03 Built. Stages three and four landed together, and every Open Question above is
  answered in place. Three things settled while building that this had not asked:
  - **Closing needed a region showing the thing, and now needs only that the window holds it.**
    Every tab but one is in the background once a window shows one at a time, so the old rule
    would have made `muster tab close --tab` and `muster pane close --pane` refuse nearly every
    name a script could give them. What the rule was protecting against - acting on a session
    this window is not attached to - is still refused. The agent list names every tab and every
    pane, so you can still see what you are about to destroy.
  - **`--daemon` with no pane falls back further.** It was the keyboard's pane on that machine,
    then that machine's region; a machine whose tabs are all in the background has neither, which
    is now the ordinary state. It now falls back to that machine's first pane anywhere in the
    window.
  - **A window asks a machine for a workspace only once that machine has spoken.** Asking in the
    moment before a first snapshot lands opened a tab nobody wanted and left the one they did
    behind.
