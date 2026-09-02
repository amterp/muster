# Reading a window

`muster window` answers what a window is showing at one moment: its daemons, its tabs, its
panes, and what the agent in each pane is doing. An agent has no eyes, so this is how it finds
out what it did.

    local  connected
      this machine · started by Muster · 3 panes in ~/src/muster
      /Users/you/.config/herdr/sessions/muster/herdr.sock
      tab 1  t1w3r07bsd  ~/src/muster  on screen
      ▸ 1  p1w3r07bsd  unknown  ~/src/muster
        2  p1w3r0ab2n  working  🤖 A · reading AGENTS.md
        3  p1w3r0cd4x  blocked  🤖 B  (hidden)

`▸` marks the pane the window's keyboard is on. `(hidden)` means the pane exists and no region
is showing it, which is ordinary: a tab that is not on screen still holds its panes, and they
are still running.

`--json` answers the same thing as one flat list of panes, which is what filtering wants:

    muster window --json | jq -r '.panes[] | select(.state == "blocked") | .pane'

## More than one window

Inside a pane this always answers about that pane's own window, because `$MUSTER_SOCKET` says
which one that is. Outside every pane, with several windows listening, it answers for all of
them - naming none is what "what is everything doing" means, and a question has nothing to be
ambiguous about.

Each answer is then headed by the window it is about: `window 39103`, the pid the socket is
named after, with the socket path beside it for `--socket`. Under `--json` the answer becomes
`{"windows": [...]}`, each entry carrying its `socket`, its `window`, and the ordinary fields
below - so `.windows[].panes[] | select(.state == "blocked")` reads across every window.

With one window listening, both shapes are exactly what they are above. `--socket PATH` narrows
to one at any time.

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
- `zoomed` - whether one pane is filling it rather than the tab's whole tree. Nothing else in the
  answer says so: a zoom's hidden panes read `on_screen: false` exactly like the panes of a tab
  in the background, and `pane` above is the one still drawn.

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

One entry per machine this window is attached to.

- `state` is `connected`, `stale` or `disconnected`, and `detail` says why for the two that are
  not `connected`. Read this before acting on the rest: everything above comes from Muster's
  picture of each daemon, and an hour-old picture looks exactly like a current one without it.
- `host` is where it runs, empty for this machine.
- `socket` is the path this window reaches it on. Over SSH that is the near end of the forward
  rather than the path over there, because it is the one you could dial from here.
  `HERDR_SOCKET_PATH=<socket> herdr server stop` ends that daemon and not the one beside it.
- `started_by_muster` says whether this window started the daemon or attached to one that was
  already answering. The second is ordinary and is the one worth knowing: a Muster launched
  today adopts a daemon started yesterday if it is still answering, so what is in it may
  predate the window.
- `panes` and `directories` say how much it holds and where.

Muster is the only thing that can answer the last three. You can ask a socket what it holds,
and you can ask the OS which process holds a socket, and nothing gets from one to the other -
herdr has no method that answers "which process are you". So pairing a herdr process with the
work inside it is Muster's to keep, because Muster either started the daemon or chose to attach
to it. Without it the choice is made on age, and age picks the wrong process: of twenty daemons
on one machine, the one holding somebody's live agent was neither the oldest nor the youngest.
