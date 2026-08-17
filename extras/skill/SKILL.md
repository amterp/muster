---
name: muster
description: Drive a Muster window - make panes, start agents in them, read what every agent is doing, and send them instructions. Use when running inside a Muster pane, or when a task involves several agents working side by side.
---

# Muster

Muster is a native macOS workspace for AI coding agents: real splits, agent status at a glance,
local and SSH agents in one window, on daemon-owned sessions that outlive the app.

You can drive the window you are drawn in. Start here:

```
muster docs
muster --help
```

Both come out of the running binary, so they describe the version you are talking to rather than
whatever a page on the internet last said. `muster docs overview` is the vocabulary,
`muster docs window` is every field of `muster window --json`, `muster docs agents` is making
panes and instructing what runs in them, and `muster docs limits` is what this cannot do.

## Before you reach for it

`muster` is on your `PATH` only if a Muster window put it there. Check with `muster window`: exit
3 means there is no window to talk to, and then nothing here applies.

## What is worth knowing that reference docs will not tell you

**Read `muster window` before you act.** You have no eyes. A pane you remember making may have
been closed, renamed, or moved to another tab by the person at the keyboard.

**Check `daemons[].state` in the same answer.** `stale` means the rest of it is an old picture,
and acting on an old picture is how you send an instruction to a pane that is gone.

**Do not take focus you were not asked for.** `pane new` leaves the keyboard where it is, and that
is the right default: the person is reading something. `--focus`, and `muster focus`, interrupt
them. Use them when an agent needs attention, not to show off work.

**Name every pane you make.** `--name '🤖 A'` costs nothing and is the only way a person running
several agents can tell them apart. Muster's own pane names are unique but unmemorable.

**`muster tab rename` with no `--tab` is not your tab.** It means the tab the person's keyboard is
in, which is usually somewhere else. Nothing tells a pane which tab holds it, so name the tab
outright: `muster window --json` gives every pane a `tab`, and yours is the row whose `pane` matches
`$MUSTER_PANE`.

**Muster imposes no workflow.** Panes, states and names are primitives. Nothing here is a way of
working you are expected to follow.
