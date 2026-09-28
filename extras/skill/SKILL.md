---
name: muster
description: Drive a Muster window - make panes, start agents in them, read what every agent is doing, and message them. Use when running inside a Muster pane, or when a task involves several agents working side by side.
---

# Muster

Muster is a native macOS workspace for AI coding agents: real splits, agent status at a glance,
local and SSH agents in one window, on daemon-owned sessions that outlive the app.

Everything you need is in the reference that ships inside `muster`, so it describes the version
you are talking to:

```
muster docs            # the topics
muster docs agents     # making panes, running agents in them, waiting on them, reading them
muster docs msg        # messaging other agents, and being woken by them
muster docs window     # every field of muster window --json
muster docs limits     # what this cannot do
muster --help
```

Read `muster docs agents` and `muster docs msg` before you start other agents. What follows is
only what goes wrong when you skip them.

## Before you reach for it

Every pane Muster makes has `muster` on its `PATH` and `$MUSTER_SOCKET` naming the window it is
drawn in, on this machine and on an SSH devenv alike, so every verb works from either. With no
window to talk to - a terminal outside Muster, or a pane whose window has since quit - `muster
window`, `pane read`, `pane send`, `pane wait` and `muster msg` still answer, from the machine's own
daemon, and every other verb says it needs a window. `muster docs overview`, under "With no
window", has the rest.

**Do not retry on exit 4.** A window or daemon took the request and never answered, so what you
asked for may already have happened, and sending it again is how an agent gets your instruction
twice. Only 3 is safe to repeat. After a 4, `muster pane read --pane X` before deciding anything.

## Rules the reference states once and agents still break

- **Read `muster window` before you act**, and check `daemons[].state` in the same answer:
  `stale` means the rest of it is an old picture. A pane you made may since have been closed,
  renamed or moved by the person at the keyboard.
- **Ask the person with `muster msg post --to @human`.** It notifies them, and their answer
  wakes you; do not sit at your prompt hoping they notice. On a devenv the person is on the
  laptop, so this needs a group they have joined.
- **Tell an agent something with `muster msg post --to <pane>`, not with `muster pane send`.**
  A message arrives whole and wakes the agent once it is idle at an empty prompt; `pane send`
  types into the pane, and is for answering a prompt the agent is blocked on.
- **After posting, end your turn.** An answer wakes you. Exit 6 means nobody live heard the post
  and no answer is coming; a post that says `its prompt cannot be read` reached a harness the
  doorbell cannot ring. When you are woken, run the `muster msg read` the wake names before
  posting to that group again.
- **Several agents working one question together is a council**, and the `council` skill, which
  ships beside this one, covers taking part in one, directing it, and convening it.
- **Do not poll.** `muster pane wait --pane X --until idle,blocked` blocks until the agent gets
  there. A pane already idle answers at once, so after handing an idle agent work, wait
  `--until working` first. `waiting` is not `idle`: add it to hear of an agent waiting on its own
  build, and give `--timeout`.
- **Do not take the keyboard.** `pane new` leaves focus where it is, which is right: the person is
  reading something. `--focus` and `muster focus` are for an agent that needs them.
- **Name every pane you make**, `--name '🤖 A'`, so a person can tell your agents apart.
- **`muster tab rename` without `--tab` renames the tab the person's keyboard is in**, not yours:
  name it from the `tab` on your own row of `muster window --json`.
- **Fix a layout with `muster pane move`, never by closing a working agent.**

Muster imposes no workflow. Panes, states, names and messages are primitives.
