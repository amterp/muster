# Reading a window

`muster window` answers what a window is showing at one moment: its daemons, its tabs, its
panes, and what the agent in each pane is doing. An agent has no eyes, so this is how it finds
out what it did.

    local  connected
      tab 1  t1w3r07bsd  ~/src/muster  on screen
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
- `tab` - the name of the tab it is in, and what to pass to `muster tab`. A name rather than a
  place, so that one read is enough to act on: this is how a pane finds its own tab, since
  nothing in its environment says which one holds it.
- `label` - what to call it to somebody who did not open it: the name somebody gave it, or
  failing that its directory and the harness detected in it.
- `given_name` - the name somebody gave it, empty when nobody has.
- `subtitle` - what its agent is working on, empty when there is nothing worth a second line.
- `state` - `working`, `blocked`, `idle`, `done` or `unknown`.
- `on_screen` - whether a region is showing it right now.
- `keyboard` - whether the window's keyboard is on it.

## tabs[] and keyboard

`tabs[]` carry `tab`, `daemon`, `place`, `label`, `given_name` and `on_screen`.

- `tab` - its name, and what to pass to `muster tab focus` and `muster tab rename --tab`. Muster's
  own name, unique across every machine the window is showing, so it needs nothing beside it.
- `place` - where it sits in the window's whole tab order, counting from one across every daemon.
  What `next_tab` walks. No chord names it - ⌘1 to ⌘9 number panes.
- `label` - what to call it to somebody who did not open it. `given_name` is what somebody typed,
  empty when nobody has.
- `on_screen` - whether a region is showing it. Not the same question as its panes being on
  screen: a zoomed tab is on screen while all but one of its panes are not.

`keyboard` at the top level is the name of the pane the window's keyboard is on, or `null` when
no pane has it.

## regions[]

The columns the window is divided into, left to right, and JSON only. A person reading the plain
output has the window in front of them; a script arranging one has neither that nor any other way
to tell which tabs sit side by side - `tabs[].on_screen` says a tab is showing somewhere and not
where.

- `region` - Muster's name for the column.
- `daemon` - which machine's tab it is showing.
- `tab` - the tab it is showing.
- `pane` - the pane in it the keyboard feeds while this region is focused. Empty while the daemon
  has not yet said what is in that tab.
- `weight` - its share of the window's width, relative to the other regions. A weight rather than
  a fraction, so three untouched columns read as `1, 1, 1` rather than as three thirds. Divide by
  the sum to get a width.
- `keyboard` - whether this is the region the window's keyboard is in.

This is the one part of the arrangement Muster owns outright rather than mirrors from a daemon: no
daemon knows the other one exists, so nothing else in this answer implies it.

## Agent states

`working`, `blocked`, `idle` and `done` come from the harness running in the pane. `unknown` is
the ordinary answer for a pane running a plain shell, and also for a pane whose harness could
not be read: an agent Muster failed to read is not an agent that finished.

`done` is decided per window rather than per daemon, because a daemon cannot see which window
somebody has looked at.

The state column is coloured: `working` blue, `blocked` yellow, `done` green. `idle` and
`unknown` are left plain, because they are the resting answer and the row already prints the
word. It is the same legend the window itself paints, where `blocked` is orange - the sixteen
colours a terminal has hold no orange, and yellow is the nearest slot.

## daemons[]

`daemon`, `state`, and `detail`. State is `connected`, `stale` or `disconnected`, and `detail`
says why for the two that are not `connected`.

Read this before acting on the rest. Everything above comes from Muster's picture of each
daemon, and an hour-old picture looks exactly like a current one without it.
