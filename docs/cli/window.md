# Reading a window

`muster window` answers what a window is showing at one moment: its daemons, its tabs, its
panes, and what the agent in each pane is doing. An agent has no eyes, so this is how it finds
out what it did.

    local  connected
      tab 1  ~/src/muster  on screen
      ▸ 1  p1w3r07bsd  unknown  ~/src/muster
        2  p1w3r0ab2n  working  🤖 A · reading AGENTS.md
        3  p1w3r0cd4x  blocked  🤖 B  (hidden)

`▸` marks the pane the window's keyboard is on. `(hidden)` means the pane exists and no region
is showing it, which is ordinary: a tab that is not on screen still holds its panes, and they
are still running.

`--json` answers the same thing as one flat list of panes, which is what filtering wants:

    muster window --json | jq -r '.panes[] | select(.state == "blocked") | .pane'

## panes[]

One entry per pane every followed daemon holds, on screen or not.

- `pane` - its name, and what to pass to `--pane`.
- `place` - where it sits in the window's whole pane order, counting from one across every
  daemon and every tab. The number ⌘1 to ⌘9 name.
- `daemon` - which machine holds it.
- `tab` - the `place` of the tab it is in.
- `label` - what to call it to somebody who did not open it: the name somebody gave it, or
  failing that its directory and the harness detected in it.
- `given_name` - the name somebody gave it, empty when nobody has.
- `subtitle` - what its agent is working on, empty when there is nothing worth a second line.
- `state` - `working`, `blocked`, `idle`, `done` or `unknown`.
- `on_screen` - whether a region is showing it right now.
- `keyboard` - whether the window's keyboard is on it.

## tabs[] and keyboard

`tabs[]` carry `daemon`, `place`, `label`, `given_name` and `on_screen`. A tab has no id here,
because the only id a tab has is its daemon's, which means nothing on another machine. To bring
a tab on screen, focus a pane in it.

`keyboard` at the top level is the name of the pane the window's keyboard is on, or `null` when
no pane has it.

## Agent states

`working`, `blocked`, `idle` and `done` come from the harness running in the pane. `unknown` is
the ordinary answer for a pane running a plain shell, and also for a pane whose harness could
not be read: an agent Muster failed to read is not an agent that finished.

`done` is decided per window rather than per daemon, because a daemon cannot see which window
somebody has looked at.

## daemons[]

`daemon`, `state`, and `detail`. State is `connected`, `stale` or `disconnected`, and `detail`
says why for the two that are not `connected`.

Read this before acting on the rest. Everything above comes from Muster's picture of each
daemon, and an hour-old picture looks exactly like a current one without it.
