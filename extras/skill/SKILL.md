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
muster docs window     # every field of muster window --json, and --layout for where panes sit
muster docs harnesses  # what Muster gets from each harness: Claude Code, Codex, the rest
muster docs limits     # what this cannot do
muster --help
```

Read `muster docs agents` and `muster docs msg` before you start other agents. What follows is
only what goes wrong when you skip them.

## Before you reach for it

Every pane Muster makes has `muster` on its `PATH` and `$MUSTER_SOCKET` naming the window it is
drawn in, on this machine and on an SSH devenv alike, so every verb works from either. With no
window to talk to - a terminal outside Muster, or a pane whose window has since quit - `muster
window`, `pane read`, `pane send`, `pane wait`, `pane compact` and `muster msg` still answer, from the machine's own
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
  wakes you; do not sit at your prompt hoping they notice. On a devenv this reaches the person
  on the laptop.
- **Tell an agent something with `muster msg post --to <pane>`, not with `muster pane send`**,
  on this machine or another the window is attached to. A message arrives whole and wakes the
  agent once it is idle at an empty prompt; `pane send` types into the pane, and is for
  answering a prompt the agent is blocked on. `--urgent` reaches the agent while it works, for
  what should change what it is doing now rather than once it is done.
- **After posting, end your turn.** An answer wakes you. Exit 6 means nobody live heard the post
  and no answer is coming; a post that says `its prompt cannot be read` reached a harness the
  doorbell cannot ring. When you are woken, run the `muster msg read` the wake names before
  posting to that group again.
- **Several agents working one question together is a council**, and the `council` skill, which
  ships beside this one, covers taking part in one, directing it, and convening it.
- **Do not poll.** `muster pane wait --pane X --until idle,blocked` blocks until the agent gets
  there. A pane already idle answers at once, so after handing an idle agent work, wait
  `--until working` first. `waiting` is not `idle`: add it to hear of an agent waiting on its own
  build, and give `--timeout`. `--context 80` also ends the wait once the agent's context is that
  full.
- **Read a finished agent's report with `muster pane read --pane X --turn`**: what it printed
  since it last went to work, without the turn before or your brief. Do not guess `--rows`.
- **Check that your own adapter reports** before relying on what it gives: your state, your
  context for `pane compact` and `compact_at`, your session coming back after a restart.
  `muster window --json | jq -r '.panes[] | select(.pane == env.MUSTER_PANE) | .adapter'` says
  `reporting`, `silent` (your harness has an adapter that is not installed or not running),
  `none` (it has no adapter), or `null` before your first turn has ended. For `silent`, tell the
  person: `muster harness install <harness>` runs their harness's own plugin install, which
  changes what that harness loads, so it is theirs to run, and a session takes it up when restarted.
- **Compact a worker before it runs out of context**, rather than letting it hit the limit
  mid-task: `muster pane compact --pane X keep <what the summary must keep>`. It is typed once
  the agent is idle, never mid-turn. To compact yourself, run `muster pane compact <focus>` with
  no `--pane` and end your turn; the compaction runs once it has ended.
- **Do not take the keyboard.** `pane new` leaves focus where it is, which is right: the person is
  reading something. `--focus` and `muster focus` are for an agent that needs them.
- **Name every pane you make**, `--name '🤖 A'`, so a person can tell your agents apart.
- **`muster tab rename` without `--tab` renames the tab the person's keyboard is in**, not yours:
  name it from the `tab` on your own row of `muster window --json`.
- **Fix a layout with `muster pane move`, never by closing a working agent.**

Muster imposes no workflow. Panes, states, names and messages are primitives.
