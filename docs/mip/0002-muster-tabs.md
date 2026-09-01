---
mip: 2
title: A Muster tab, and the adapter that maps it to herdr's
status: Draft
kind: Architecture
created: 2026-08-31
decided:
supersedes:
superseded-by:
related: 1
---

# MIP-2: A Muster tab, and the adapter that maps it to herdr's

## Summary

A tab becomes Muster's own noun. Today a tab is herdr's: it lives in one daemon's workspace,
so a window showing two machines is a column per machine, each column showing one of that
machine's tabs. Two machines are therefore always both on screen, `cmd+2` means two different
things depending on which machine holds the tab it names, and a tab holding a laptop pane
beside a devenv pane cannot be expressed at all.

The proposal: a Muster tab is a named set of panes the window shows together, one at a time,
and a pane's machine is a property of the pane rather than the axis the window is built on.
herdr's workspace and tab stay behind the backend seam, where the adapter maps between them.

This is a Draft and no build follows from it yet. What it has to settle before it can leave
Draft is the durability question in "Consequences": today the daemon writes the shape down, so
a tab survives Muster quitting and Muster never coming back. A tab spanning two machines is a
shape no single daemon can hold.

## Context / Motivation

### What was asked for

Driving a laptop and a devenv in one window (kan `a_2HtF52Itm`), with one local tab and one
devenv tab: *"I would prefer these not as two parallel panes but genuinely two different
tabs"*, wanting `cmd+1` and `cmd+2` to switch between them the way tabs switch everywhere else.
And on where the model should come from: *"we shouldn't let ourselves be constrained by
herdr... I would want a very clear API boundary that translates between Muster modelling and
herdr modelling."*

Neither is expressible today, and the second is the reason the first is not.

### The model today, in one sentence

**The window is a column per machine, each column showing one of that machine's tabs.** Not
"tabs, one of which fills the window". Everything else in this card follows from it.

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
chord, same-looking rows, two acts. Marking the tabs that share the screen (done this round, in
the sidebar) lets somebody predict which they are about to get. It treats the symptom.

The caption is the other symptom. `tab_label` writes `Tab <place>` counting across the whole
window, because herdr labels an unnamed tab by its position inside its workspace and two
daemons would each contribute a `1`. So `Tab 1` and `Tab 3` sit side by side on one screen while
every other terminal and every browser has trained the reader that tabs are what you switch
between.

## Where herdr's model got into the core

This is the MIP's first job, and the answer is: through the composition record, and from there
onto disk.

**The glossary already says it outright.** `docs/glossary.md`: *"**tab** - the unit that owns
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

Proposed, not settled.

**A Muster tab is a named set of panes that a window shows together, one tab at a time.** Muster
mints its name the way it mints a pane's, and `t1w3r07bsd` already is that name. A window holds
an ordered list of them; exactly one is on screen; `cmd+1` to `cmd+9` and `next_tab` walk that
list.

**A pane belongs to exactly one machine. A tab does not.** A pane is a PTY and it lives where
it lives, which is why `pane move` refuses across machines and will keep refusing. A tab is a
grouping Muster made, so one tab holding a laptop pane beside a devenv pane is something Muster
can express.

**A region divides a tab, not the window.** Regions stop being "one per machine" and become
what a split makes: the parts of the tab currently on screen. Region weights and boundaries
keep their present meaning inside a tab.

**The adapter owns the mapping.** `muster-herdr` translates a Muster tab into the herdr tabs it
needs, one per machine that has panes in it, each inside whatever workspace that machine
supplies. The core stops naming a workspace at all: `BackendIntent` says "a tab beside this
pane" and the adapter resolves which of herdr's workspaces that means, keeping the defence
quoted above where herdr's behaviour is known.

**The saved arrangement stops carrying herdr's ids.** It names Muster's tabs and Muster's
panes, both of which come from the registry that already survives a restart.

What that gets, concretely. The sidebar reported in the card, with the devenv agent and the
local shell side by side on screen and the local one called `Tab 3`:

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
everything else. Done this round, and it is the right thing to have done whatever happens next:
it makes `cmd+2` predictable today at the cost of nothing. Rejected as the whole answer because
it does not make the request expressible. Somebody driving two machines learns the model in a
day; the question is whether the model is the one we want.

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

## Consequences & Trade-offs

### The durability question, which is the one that decides this

**A tab spanning two machines is a shape no single daemon can write down.** Today the daemon
holds the tab, which is why AGENTS.md can say *"sessions outlive the app, and their shape
outlives the daemon"*: quitting Muster costs nothing, and a Muster that never comes back costs
nothing either, because herdr still has the tab and its tree. A cross-machine tab can only live
in Muster's own state, which turns a daemon guarantee into a Muster one.

The proposal that keeps most of it: **degrade per tab, not wholesale.** A tab whose panes are
all on one machine still maps to one herdr tab, so the common case keeps the guarantee it has
now. Only a tab that actually spans machines depends on Muster's file, and only that tab is
lost if the file is. Whoever builds this has to write the weaker case into the glossary and
into `architecture.md`'s durability section rather than leave it to be discovered.

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
there is no "give every machine a column".

### What gets harder

Every rule that today says "this daemon's region" has to be restated in terms of a tab that may
touch several daemons. `reconcile` is per daemon on purpose, because streams from different
daemons have no mutual order, and a tab spanning two of them has to be reconciled without
introducing a cross-daemon ordering that does not exist.

### How this round's shakedown fixes survive it

**The sidebar's on-screen mark (this card's other half)** answers a question the new model
deletes: with one tab on screen, the mark is true for one row and says nothing. It should be
removed with the change rather than carried, and its test with it.

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

## Open Questions

- **Where does the machine go on a row?** On the pane, if a tab may span machines, which this
  proposes. Then what does a tab row say about machines, if anything?
- **How does the machine stay quiet?** A heading is read once; a badge repeats on every row.
  Tinting, labelling only the machines that are not this one, or labelling only while more than
  one is attached are all cheaper and all lose something.
- **Where do a machine's own states go?** The heading carries `connected`, `disconnected` and
  `no tabs, so this daemon is holding nothing` today. Without a heading, a machine holding zero
  panes would vanish from the window, which is exactly the state `a_2HpkpfIfq` was about. This
  probably has to be solved first or together.
- **Does a Muster tab map to a herdr tab per machine, or to a herdr workspace per machine?** A
  workspace is herdr's unit for a whole project, so a tab is the closer fit, but a workspace is
  the thing herdr restores as a unit.
- **What does `muster window --json` say?** `regions[]` is documented as "the columns the window
  is divided into", which is a machine-shaped sentence. It becomes the parts of the tab on
  screen, and the answer needs a way to say which tab that is.
- **What happens to an existing `~/.muster/state/window.toml`?** It names herdr workspaces and
  tabs. Read each saved region as a single-machine Muster tab, or drop the file and open fresh?
- **Does `pane move` across machines become expressible?** It stays refused, because a pane is a
  process. But "move this pane into that tab" now has an answer that spans machines, and the two
  should not be confused in one verb.

## References

- kan `a_2HtF52Itm` - the observation, the measurements, and the direction.
- kan `a_2Hx68fXqr`, `a_2Hwef7lQT`, `a_2HpkpfIfq`, `a_2Ht74jTXV` - the shakedown fixes this has
  to survive.
- MIP-1 - the seam this argues is in the wrong place for one noun.
- `docs/architecture.md`, durability and degradation.
- `docs/glossary.md`, which defines `tab` as daemon truth.

---

## History
- 2026-08-31 Draft
