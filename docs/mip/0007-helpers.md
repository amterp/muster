---
mip: 7
title: An agent's helpers - panes an agent makes as its own, kept out of the way
status: Accepted
kind: Product
created: 2026-10-07
decided: 2026-10-17
supersedes:
superseded-by:
related: 3, 4
---

# MIP-7: An agent's helpers - panes an agent makes as its own, kept out of the way

## Summary

An agent that starts other agents to do its work can say so: `muster pane new --helper` makes a
pane that belongs to the pane that ran it. The helper's daemon writes the relation on the pane's
record, beside `quiet` and `hidden`, so it survives a restart and every window agrees about it.

What the relation changes:

- **Made hidden and quiet by default** (question B). The person talks to the agent that made the
  helpers, and the helpers are that agent's business. This is the parallel-work round amterp
  described: the person talks to the integrator, and the workers stay out of sight.
- **Listed under the pane that made them**, in the agent list and in `muster window`, collapsed
  into one line until somebody opens it.
- **Brought forward together**, into one tab, and put away together.
- **A helper that blocks, gets stuck, or goes idle without reporting tells the agent that made
  it**, through `muster msg`. Today a quiet pane in that state tells nobody.
- **A helper whose maker closes stops being quiet**, and the person hears once that it is still
  running.

amterp settled both questions on 2026-10-17: the word is "helper", and a helper is hidden and
quiet by default. Everything else is recommended, with the options laid out where they were
close.

## Decisions for amterp

**Question A. The word.**

- (a) "sub-agent", amterp's word. `muster window` and the agent list already print `2 sub-agents`
  for what `AgentFacts.subagents` counts: the agents a harness runs inside its own session, such as
  Claude Code's Task agents. That count would need renaming, to "tasks" or similar, or one row
  could say `2 sub-agents` while listing three panes as its sub-agents.
- (b) "helper": `--helper`, "Show Helpers", `3 helpers`. `muster docs agents` already calls these
  panes helpers ("An agent that runs helpers it talks to over `muster msg`"). The word is taken
  twice elsewhere, in places the window never shows: a harness manifest's `helpers` key, for a
  harness's own helper processes, and "the helper app the daemon runs as".
- (c) "worker", the parallel-work skill's word. It names one workflow's role, and Muster imposes
  no workflow.

Recommendation: (b). Its other uses are a manifest key and a packaging term that no row or flag
shows, where (a)'s other use is on the same rows these would be listed beside. The rest of this
draft says "helper", and "maker" for the pane that made one. Hard to reverse: the flag, the menu
items and the skill's wording all carry the word.

**Decided 2026-10-17: (b), "helper".** amterp's account of what the word covers: a helper is an
agent that another agent starts for its own work and stays coupled to, and Muster sets up the
coupling for it - the maker hears when the helper needs it, and a group for the two exists without
anybody making one - with flags to opt out. An agent started as a peer is not a helper (section
1).

**Question B. Whether a helper is quiet by default.**

- (a) Hidden and quiet, with `--shown` and `--notify` to opt out of each. What amterp described.
- (b) Hidden only. Its blocks and finishes notify the person as any pane's do.

Recommendation: (a), because section 4 of the decision sends a quiet helper's one urgent case, a
block or a stall, to its maker, which is the agent able to act on it. Without that section this
would be (b), since a quiet helper at a permission prompt would otherwise wait on nobody. Easy to
reverse: a default changes in one place.

**Decided 2026-10-17: (a), hidden and quiet.**

## Context / Motivation

### What was asked for

amterp, 2026-10-07 (kan `a_2die894mw`): "you could kind of imagine ... in the parallel work skill
example, like the main integrator agent maybe like has some way to express with muster that these
agents are sub agents or they belong to my group, and so therefore it has like the option to spawn
them hidden, and there's some easy connected way to kind of see and like bring them forward,
unhide them and see them." And: "This might take some design thought."

### What exists, and what is missing

Hiding and quiet landed the same day (kan `a_2die7jO4X`, `a_2die7W9ru`; `muster docs agents`,
"Out of sight"):

- `muster pane new --hidden --quiet` makes a pane in a tab of its own that no window shows, on
  the machine of the pane it was split from. Nothing about the pane notifies the person.
- A pane made with `pane new` beside a hidden one goes into that hidden tab, so a hidden agent's
  own helpers are hidden with it.
- `pane hide`, `pane show` and `pane quiet` change a pane later. Both facts are the daemon's, on
  the pane's record, so every window agrees and they outlive a restart.
- The agent list shows hidden panes under a Hidden caption, and `muster window` under `hidden`.

What none of it records is which pane made which. A parallel-work round of four workers today is
four rows under Hidden, beside anything else anybody hid, with nothing saying they are the
integrator's. Bringing them forward is four `pane show`s into four new tabs. A quiet worker
blocked at a permission prompt raises nothing: the person asked not to hear from it, and nothing
tells the integrator, which cannot see the prompt. And if the integrator's pane closes, the
workers go on running hidden and quiet with nobody watching them at all.

Kan `a_2ditnGlQz` gives every pane a short name, such as `otter`, that works wherever its id does.
This draft uses those names (`otter`, `maple`, `cobalt` below) and does not depend on how they
are built.

## Decision

### 1. What records the relation

A helper's pane record carries `maker`: the id of the pane that made it. `muster pane new
--helper` sets it to the pane the command runs in, its `$MUSTER_PANE`, which every request already
carries as `from_pane`. `--helper-of <pane>` names another, by id or by name. Outside every pane
`--helper` is refused, since there is no pane to belong to.

The maker is a pane, and messages meant for it go to whichever agent is running in it. The record
is the helper's, kept by the helper's daemon beside `quiet` and `hidden` and written to its state
file. A pane id is unique across every machine (`muster-core/src/names.rs`), so a helper on a
devenv can name a maker on the laptop.

The relation is not implicit on every `pane new` run from a pane. A person splitting their own
shell, and an agent opening a pane for a dev server, are not making helpers, and nesting every
such pane under whatever made it would make the agent list worse for everyone who runs no
helpers.

Nor is an agent that the person asks for as a peer. Told to fork off part of its work, or to
start another agent beside it, an agent starts one that works at its own level and answers to the
person: it is made with plain `pane new`, nobody watches it on its maker's behalf, and its blocks
reach the person as any pane's do. The two can still work together in a group.

### 2. What it changes by default

A helper is made hidden and quiet, and `--shown` and `--notify` opt out of each. `--shown`
splits it beside its maker as `pane new` does today; it is still a helper and still listed under
its maker.

**Each maker's hidden helpers share one hidden tab**, made with the first of them and named for
the maker: `🤖 integrator's helpers`. Today's rule already puts a pane made beside a hidden one
into that hidden tab. This one also applies when the next helper is made from the maker's own
pane, which is not hidden. One tab per maker is what lets section 5 bring them forward together,
arranged as they were.

### 3. How it shows

A helper is listed under its maker wherever the helper is held, so a hidden helper's row moves
from the Hidden caption to beneath its maker's row.

**The agent list** lists a maker's helpers under its row, indented, collapsed by default into one
line: `3 helpers · 1 blocked`, with the most urgent state among them shown as the dot's color.
Opening it shows each helper's row. Helpers leave the Hidden caption, which keeps the panes
somebody hid by hand.

**`muster window`** prints each helper indented under its maker, by its short name where its id
would be, and its label after as today:

    tab 1  t1w3r07bsd  ~/src/muster  on screen
      ▸ 1  p1w3r07bsd  working   4m  🤖 integrator  3 helpers
             otter     working  12m  🤖 1 · building the gate  (hidden, quiet)
             maple     blocked   2m  🤖 2  (hidden, quiet)
             cobalt    idle     40m  🤖 3  (hidden, quiet)

**`--json`** gives every pane a `maker`, null for a pane nobody made, in `panes[]` and in
`hidden[]` alike. A maker's helpers are then
`[.panes[], .hidden[].panes[]] | .[] | select(.maker == env.MUSTER_PANE)`.

A maker on a machine the window cannot reach reads `unreachable` on its own row (kan
`a_2cJ6cgiLw`), and its helpers stay listed under it, each reading what its own daemon says.

### 4. A quiet helper that needs somebody tells its maker

When a helper becomes `blocked` or `stuck`, or goes idle without having posted to its maker since
it last went to work, its daemon tells the maker, once each time, in a message from `@muster` in
the group the two share, made for the purpose if they have none: `maple is blocked at a dialog`,
`maple has stopped reading its input`, or `maple went idle without posting to you`. The idle case
is a helper whose turn ended on an API error, or one that gave up without saying so; on
2026-10-13 one sat idle for 35 minutes before its maker noticed. A helper whose declared wait
ran out with nothing ending it is idle for a known reason, and the notice says that instead,
naming the wait (added 2026-10-24). A helper that posts its report
and then goes idle is not reported, and a compaction or a rename Muster typed at it is not going
to work: on 2026-10-17 helpers compacted at their maker's request went idle afterwards and their
maker was told, six times in one evening. MIP-4 section 16 already
has a daemon tell a group once, and say why, when the group is held up on a member; this does the
same for a helper and sends it to the maker, the agent that can act on it. The maker can answer
the dialog with `muster pane send --key`, or bring the helper forward and ask the person.

A maker at work is told once it can hear it, and only if the need still holds. A post to it would
wait for its turn to end anyway, and on 2026-10-21 every plan dialog its maker answered within
that turn reached it as a notice two minutes later, after the helper was back at work. A maker
whose hooks fetch messages mid-turn is told at once, as before.

The person is not told while the maker is alive and reachable. The person is told in two cases:

- **The maker is unreachable**, its machine not answering. The person is the only one who can
  act, so the helper notifies as if it were not quiet.
- **The maker is gone**, which section 6 covers.

The daemon notices rather than a window, so on one machine a round runs with no window open. A
maker on another machine is reached as MIP-4 section 11 reaches a member there, which needs a
window attached to both.

### 5. Bringing them forward together

The maker's row gets **Show Helpers** and **Hide Helpers** in its right-click menu, and the CLI
gets:

    muster pane show --helpers-of integrator
    muster pane hide --helpers-of integrator

Show brings the maker's helper tab into the asking window, after its other tabs, with the helpers
arranged as they were. Hide sends that tab back. A helper brought forward on its own, with
`pane show`, comes into a tab of its own as today and stays a helper.

*Amended 2026-10-18:* a helper brought forward on its own, by `pane show` or by going to it, now
brings back the helpers' tab whole, so no helper leaves it. That tab, from Show as well, comes in
just after the tab holding the maker when the asking window holds that tab, and after the
window's other tabs otherwise.

### 6. When the maker goes away

The relation is to the maker's pane, not to its agent. An agent that exits and is resumed in the
same pane keeps its helpers: kan `a_2ditnGlQz` settled that what persists is the pane, and Muster
resumes an agent into the same pane.

When the maker's pane closes:

- (a) Its helpers close with it.
- (b) They go on running, hidden and quiet, as they are.
- (c) They go on running, stop being quiet, and the person is told once:
  `🤖 integrator closed; 3 of its helpers are still running`, and clicking the notification
  brings them forward as Show Helpers does.

Recommendation: (c). (a) ends work nobody asked to end, and a maker that crashed is not a decision
to stop its helpers. (b) leaves agents running that nobody is watching or will hear from, the
case Context describes. Under (c) their `maker` stays on the record, and the agent list keeps
them under the closed maker's name until they are closed.

Across machines, a helper's daemon cannot see a maker on another machine close. The window can,
and tells the helper's daemon; with no window open, it is told when one next opens.

### 7. What the skill tells an agent

`extras/skill/SKILL.md` gains one rule, replacing the advice to make helpers with `--hidden
--quiet`:

> **Make the agents you run for your own work with `--helper`.** They are hidden and quiet: the
> person does not see or hear from them, and you are told, in a message from `@muster`, when one
> is blocked or stuck. Name each one. When the person wants to watch them, say
> `muster pane show --helpers-of $MUSTER_PANE`, or Show Helpers on your row. Close them when
> their work is done. An agent the person asks you to start as a peer, to take part of the work
> at its own level, is not a helper: make it with plain `muster pane new`.

## Rationale

The record lives where `quiet` and `hidden` already live, because it is the same kind of fact: it
is about a pane, it has to survive the app quitting and the daemon restarting, and every window has
to agree about it. Kept in a window, two windows could disagree about whose helper a pane is.

Helpers come forward as one tab rather than one tab each because a person who wants to see the
workers wants to see all of them, arranged as the maker left them.

## Alternatives Considered

**The pane that ran `pane new`, recorded for every pane.** Every pane would get a maker for free,
and no flag would be needed. Rejected for the reason section 1 gives: most panes made from a pane
are not helpers.

**A message group as the relation.** An agent and its helpers already share groups in a
parallel-work round. Rejected because a group is a conversation among peers: a council has members
and no maker, a helper may be in several groups, and membership changes as people join and leave.
None of that answers "whose is this pane".

**The hidden tab as the relation, with no field.** A maker's helpers would be the panes in the
hidden tab named for it, and showing the tab would bring them forward. It needs nothing new on the
record. Rejected as the record, because the relation would end the moment somebody moved a helper
to another tab, and a `--shown` helper would have no relation at all. Kept as how hidden helpers
are held (section 2).

**Escalating to the person after a timeout.** Left out of the decision; see Open Questions.

## Consequences & Trade-offs

- **Another field on the pane record**, and a protocol minor for it: a daemon older than it
  ignores `maker`, so a helper made through one is an ordinary hidden pane.
- **The agent list grows a level.** Rows under rows are new to it, and its numbering (⌘1 to ⌘9
  counted down the whole list) has to say whether a collapsed helper counts. Proposed: helpers are
  not numbered, as hidden panes are not today.
- **`--hidden` stays.** A pane hidden with no maker is still useful, for a dev server somebody
  wants running and out of sight, and the Hidden caption keeps those.
- **A helper that notifies after its maker closes** is a notification the person did not ask for.
  It comes once per closed maker rather than once per helper.

## How it lands

Each stage leaves helpers in a state somebody can use.

1. **The record and the CLI.** `maker` on the pane record and in `muster window --json`,
   `pane new --helper` and `--helper-of`, hidden by default with one hidden tab per maker, and
   `muster window` indenting helpers under their maker. Not yet quiet by default: until stage 3,
   nothing would tell anybody that a quiet helper is blocked.
2. **The agent list.** Helpers under their maker, collapsed, with Show Helpers and Hide Helpers,
   and `pane show` / `pane hide --helpers-of`.
3. **Telling the maker, and quiet by default.** Sections 4 and 6 on one machine, and the skill's
   new rule.
4. **Across machines.** Sections 4 and 6 for a maker and a helper on different daemons.

## Open Questions

- **Helpers of helpers.** A helper can make helpers of its own. The record allows it; how deep the
  agent list nests before it flattens is undecided.
- **A maker ignoring its helper.** Whether a block its maker has not answered in some minutes
  should reach the person after all.
- **A helper moved by hand** into one of the person's tabs stays a helper; whether Hide Helpers
  should take it back out of that tab is undecided.
- **Making a pane somebody's helper later**, with `pane helper --of`, for a pane made before this
  existed, or one whose maker closed.

## References

- kan `a_2die894mw` (this), `a_2die7jO4X` (hiding), `a_2die7W9ru` (quiet), `a_2ditnGlQz` (short pane
  names), `a_2cJ6cgiLw` (a pane on a machine Muster cannot reach).
- MIP-3: the daemon's pane record and its state file.
- MIP-4, sections 11 and 16: messaging across machines, and a group held up on a member.
- `muster docs agents`, "Out of sight": hidden and quiet as built.

---

## History
- 2026-10-07 Draft.
- 2026-10-18 Section 5 amended: going to one hidden helper brings its helpers' tab back whole
  rather than moving the helper into a tab of its own, and a helpers' tab comes in after its
  maker's tab rather than at the end. Clicking a helper's row had moved panes and tabs nobody
  asked to move.
- 2026-10-21 Section 4 amended: a compaction or a rename Muster typed at a helper is not the
  helper going to work, and a maker at work is told of a need once it can hear it, if the need
  still holds. Makers were told a helper went idle unreported after its compaction, and told of
  plan dialogs they had answered two minutes before.
