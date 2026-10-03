# Reading a window

`muster window` answers what a window is showing at one moment: its daemons, its tabs, its
panes, and what the agent in each pane is doing. An agent has no eyes, so this is how it finds
out what it did.

    tab 1  t1w3r07bsd  ~/src/muster  on screen
      ▸ 1  p1w3r07bsd  unknown   2h  ~/src/muster
        2  p1w3r0ab2n  working  12m  🤖 A · reading AGENTS.md  64% context  2 sub-agents
    tab 2  t1w3r0h4kp  the build
        3  p1w3r0cd4x  blocked  40m  🤖 B  (hidden)
        4  p1w3r0ef6y  waiting   8m  🤖 C · on the full gate  31% context  (hidden)

    local  connected
      this machine · started by Muster · 4 panes in ~/src/muster
      /Users/you/.muster/daemon/release.sock

After a pane's label come what its agent says about itself and anything else worth a glance:
how full its context is, its sub-agents, `(cannot read the screen)` when Muster's rules have
stopped reading the agent, `(bell)` for a bell nobody has looked at since, and a program's own
progress, `(40% done)` or `(progress failed)`. A waiting agent's second line is what it waits on.
The model and the cost are in `--json`.

The tabs come first because that is what the window is: it holds an ordered list of them and
shows one. The machines follow rather than heading the list, because a tab can hold panes on more
than one - so a pane's row says which machine it is on, and only while more than one is attached.

Beside each state is how long the pane has been in it, in the largest whole unit: `45s`, `12m`,
`3h`, `2d`. `▸` marks the pane the window's keyboard is on. `(hidden)` means the pane exists and the window
is not showing it, which is ordinary: a tab that is not on screen still holds its panes, and they
are still running.

`--json` answers the same thing as one flat list of panes, which is what filtering wants:

    muster window --json | jq -r '.panes[] | select(.state == "blocked") | .pane'

## With no window

When no window answers, this machine's daemon does (`muster docs overview`, "With no window"):

    no window answered; the muster-daemon at /home/you/.muster/daemon/release.sock holds:
    tab    t1w3r07bsd
           p1w3r07bsd  unknown    muster
           p1w3r0ab2n  working    🤖 A · reading AGENTS.md  64% context

    devenv  connected
      this machine · already running · 2 panes in /home/you/src/muster
      /home/you/.muster/daemon/release.sock

The same rows, less what only a window has: no places, no `▸`, no `(hidden)`, and no time in a
state, which a window counts from when it first saw the pane. `done` is still a finish nobody has
seen, as the daemon records it. In `--json`, `answered_by` is `"daemon"`; `name`, `keyboard`,
`showing` and every `since` and `rect` are null; `regions` is empty; every `place` is 0 and every
`on_screen` false.

## Other windows

A tab belongs to exactly one window, and `tabs[]` and the rows above are this window's own. The
tabs every other window holds come after them, under a heading for each window: `window 4321
(window-2)` for one that is open, and `window-2 (closed)` for one that is not - a closed window
keeps its tabs, and its agents are still running. Those rows carry no numbers, because the numbers
are that window's.

    window-2 (closed)
    tab    t1w3r0mn2q  the other build
            p1w3r0pq7r  blocked  2h  🤖 C  (hidden)

The tab names there work from here: a request naming one is carried to the window that holds it,
and `muster tab focus` on a closed window's tab reopens that window onto it, as `muster window reopen
window-2` does. `muster tab move --window` takes a window's name, or the pid of a Muster with one
window open.

Under `--json`, `name` is this window's own name, and `other_windows[]` carries `window` (its
name), `pid` (null once it has closed) and its `tabs[]`, each with `tab`, `daemons`, `label`,
`given_name` and `panes[]` of `pane`, `daemon`, `label` and `state`.

## More than one window

Inside a pane this always answers about that pane's own window, because `$MUSTER_SOCKET` says
which one that is. Outside every pane, with several windows listening, it answers for all of
them - naming none is what "what is everything doing" means, and a question has nothing to be
ambiguous about.

Each answer is then headed by the window it is about: `window 39103 (window-1)`, the pid the
socket is named after and the window's own name, with the socket path beside it for `--socket`.
The open windows' tabs are each under their own heading already, so a closed window is the only
other one listed, once, after them all. Under `--json` the answer becomes `{"windows": [...]}`,
each entry carrying its `socket`, its `window`, and the ordinary fields below - so
`.windows[].panes[] | select(.state == "blocked")` reads across every window - with
`other_windows[]` narrowed to the closed ones.

With one window listening, both shapes are exactly what they are above. `--socket PATH` narrows
to one at any time.

## panes[]

One entry per pane every followed daemon holds, on screen or not.

- `pane` - its name, and what to pass to `--pane`.
- `place` - where it sits in the window's whole pane order, counting from one across every
  daemon and every tab. What `muster focus --place` takes; ⌘1 to ⌘9 name tabs first instead.
- `daemon` - which machine holds it.
- `tab` - the name of the tab it is in, and what to pass to `muster tab`. A name rather than a
  place, so that one read is enough to act on: this is how a pane finds its own tab, since
  nothing in its environment says which one holds it.
- `label` - what to call it to somebody who did not open it: the name somebody gave it, or
  failing that its directory and the harness detected in it.
- `given_name` - the name somebody gave it, empty when nobody has.
- `subtitle` - what its agent is working on, empty when there is nothing worth a second line.
- `state` - `working`, `blocked`, `waiting`, `idle`, `done` or `unknown`.
- `reported` - whether the state is the agent's own report rather than what Muster read off its
  screen.
- `unreadable` - whether Muster's rules have stopped reading this agent's screen. Its state is
  then only what the agent reports, and `unknown` while it has reported nothing: said rather than
  guessed.
- `facts` - what the agent says about itself, `null` while it has said nothing: `context_used`
  (0 to 100), `subagents`, `model`, `cost_usd`, `waiting` (what it ended its turn to wait on) and
  `other`, its own keys. `context_used`, `model`, `cost_usd` and `waiting` are `null` until the
  agent says them, since an agent that never reported its context has not used none of it;
  `subagents` is `0` and `other` is `{}` until then.
- `progress` - what a program in the pane says of its progress, `{"state", "percent"}`, or `null`.
  `state` is `running`, `error`, `indeterminate` or `paused`; `percent` is `null` when the program
  gave none.
- `rang` - whether a program in the pane rang the bell and nobody has looked at the pane since.
- `since` - when the agent last changed state, in seconds since the epoch to the millisecond, so
  `now - .since` in jq is how long it has been in it. Two reads that say `working` with the same
  `since` are one turn; a different `since` is a finish and a new turn in between. Looking at a
  `done` pane does not move it: the agent has been resting since it finished. `null` from a window
  older than this field.
- `on_screen` - whether the window is showing it right now.
- `keyboard` - whether the window's keyboard is on it.
- `rect` - where it sits, as fractions of the window: `x` and `y` from the top left, `width` and
  `height` of it. `null` for a pane the window is not drawing, which is every pane in a background
  tab and every pane a zoom is covering. Fractions and not points, because the answer is the same
  at any window size and there is nothing to convert; every machine's part of the tab is measured
  in the same space, so two panes that came from two daemons can still be compared. Two panes
  share a row if their `y` and `height` cover the same band - the same arithmetic `muster focus
  --left` is decided by.

`rect` is how a script checks an arrangement instead of trusting it:

    muster window --json | jq -r '.panes[] | select(.rect) | "\(.pane) \(.rect.width)"'

## tabs[], showing and keyboard

`tabs[]` are the tabs this window holds, and carry `tab`, `daemons`, `place`, `label`,
`given_name` and `on_screen`.

- `tab` - its name, and what to pass to `muster tab focus`, `muster tab rename --tab`,
  `muster tab move --tab` and `muster pane move --tab`. Muster's own name, unique across every
  machine and every window, so it needs nothing beside it - from any window.
- `daemons` - the machines it holds panes on, in the order their parts sit on screen. One for
  almost every tab; two for one somebody has grouped with `muster pane move --tab`. Plural because
  a tab does not belong to a machine - which machine holds a pane is on the pane.
- `place` - where it sits in the window's tab order, counting from one. What `next_tab` walks. The
  number ⌘1 to ⌘9 name, once the window holds more than one tab.
- `label` - what to call it to somebody who did not open it. `given_name` is what somebody typed,
  empty when nobody has.
- `on_screen` - whether this is the tab the window is showing. Exactly one carries it. Not the
  same question as its panes being on screen: a zoomed tab is on screen while all but one of its
  panes are not.

`showing` at the top level is the name of the tab the window is on, or `null` when it is showing
none. `keyboard` is the name of the pane the window's keyboard is on, or `null` when no pane has
it.

## regions[]

The parts of the tab on screen, left to right, one per machine holding panes in it - so one entry
for almost every tab. JSON only. A person reading the plain output has the window in front of
them; a script arranging one has neither that nor any other way to tell how wide each machine's
part is or which order they sit in.

- `region` - Muster's name for the part.
- `daemon` - which machine's panes it is showing.
- `pane` - the pane in it the keyboard feeds while this region is focused. Empty while the daemon
  has not yet said what is in the tab.
- `weight` - its share of the tab's width, relative to the other regions. A weight rather than a
  fraction, so two untouched parts read as `1, 1` rather than as two halves. Divide by the sum to
  get a width.
- `keyboard` - whether this is the region the window's keyboard is in.
- `zoomed` - whether one pane is filling it rather than the tab's whole tree. Nothing else in the
  answer says so: a zoom's hidden panes read `on_screen: false` exactly like the panes of a tab
  in the background, and `pane` above is the one still drawn.
- `layout` - how this part splits. A leaf is `{"pane": "p1w3r07bsd"}`; a divider is
  `{"axis": "columns"|"rows", "ratio": 0.5, "first": …, "second": …}`, where `columns` puts its
  children side by side and `rows` stacks them - so a row of panes is a `columns` split. `ratio`
  is the first child's share of what the divider divides. `null` while the daemon has not yet said
  how the tab is arranged, which is ordinary rather than a failure and is a different answer from
  a part holding no panes. Resolved for a zoom, like `zoomed` above: a zoomed part is one leaf,
  because that is what is drawn.

`rect` on a pane and `layout` here answer different halves of the same question. `rect` is where
everything ended up, which is what checks a resize; `layout` is the shape that put it there, which
is what names the divider somebody would move.

Which tab they divide is `showing` above rather than a key on every row, because they all show the
same one. This is the one part of the arrangement Muster owns outright rather than mirrors from a
daemon: no daemon knows the other one exists, so nothing else in this answer implies it.

## The layout

`muster window --layout` draws every tab this window holds as boxes, one per pane, where the
window puts them and in proportion, whether or not the tab is on screen:

    tab 1  t1w3r07bsd  ~/src/muster  on screen
    ┌───────────────────────────────────┬──────────────────────────────────┐
    │ ▸ p1w3r07bsd                      │ p1w3r0ab2n                       │
    │ ~/src/muster                      │ 🤖 A                             │
    │ unknown · 118x40                  │ working · 119x19                 │
    │                                   │                                  │
    │                                   │                                  │
    │                                   ├──────────────────────────────────┤
    │                                   │ p1w3r0cd4x                       │
    │                                   │ 🤖 B                             │
    │                                   │ blocked · 119x20                 │
    │                                   │                                  │
    └───────────────────────────────────┴──────────────────────────────────┘

Each box names its pane and says its label, what its agent is doing and how big its terminal is,
in columns by rows; with more than one machine attached, which one it is on. `▸` is the pane the
keyboard is on. A zoomed tab says `zoomed on` the pane filling it and draws that pane alone,
filling the tab, since that is what is on screen and the size its program sees; the panes behind
the zoom are named on a line under it, `behind the zoom: p1w3r0ab2n`. The drawing is as wide as the terminal, and boxes too small for their text
are cut rather than dropped, so every pane is named even where a ratio cannot be drawn exactly.

A pane's size is the one its program last saw, as its daemon holds it: what it is drawn at, or
for a pane in a tab behind the one on screen, the size it was last drawn at, which is still what
wraps its output. A daemon too old to say leaves the size out, and the box gives the pane's
share of the tab instead, `50% x 70%`. Sizes in points are not offered, because only the app
knows them and they would change with the font.

`--layout` asks every daemon for its panes' sizes, which the ordinary read does not, so it is a
flag rather than the default. With `--json` it adds three keys, and nothing else changes:

- `tabs[].regions` - each tab's parts, in the shape `regions[]` below has, with one difference:
  `layout` is the tab's whole tree even when it is zoomed, because this says how the tab is
  laid out and a zoom covers that without changing it. `zoomed` and `pane` say which pane fills it.
  On a zoomed tab `frame` and `cells` therefore describe different things: `frame` is each pane's
  place in that tree, and `cells` is what its program sees, which for the zoomed pane is the whole
  tab and for the panes behind it the size they had before the zoom.
- `panes[].frame` - where the pane sits in its tab's arrangement, as fractions of the tab, in
  `rect`'s terms, whether or not the tab is on screen.
- `panes[].cells` - `{"cols", "rows"}`, or `null` when its daemon could not say.

Another open window's tabs are drawn too, under its heading, and its panes in
`other_windows[].tabs[].panes[]` carry `frame` and `cells` the same way: a frame is a place in
the pane's own tab, whichever window holds it.

Without `--layout` these keys are absent rather than null, so a size nobody asked for is never
read as one nobody knows. A closed window's tabs are listed and not drawn, and its panes' `frame`
is null: how their machines' parts sit side by side is that window's to say, once it is open. With no window answering, the daemon's tabs are
drawn from its own trees, one part each, and `frame` is null.

    muster window --layout --json | jq -r '.panes[] | "\(.pane) \(.cells.cols)x\(.cells.rows)"'

## Agent states

`working`, `blocked`, `idle` and `done` come from the harness running in the pane. `waiting` is an
idle agent that said it ended its turn to wait on work it started itself, a gate or a build: it
has not finished and nobody is holding it up, and `facts.waiting` says on what. It lasts until
the agent ends a later turn without saying it again, or somebody prompts it. `unknown` is
the ordinary answer for a pane running a plain shell, and also for a pane whose harness could
not be read: an agent Muster failed to read is not an agent that finished.

`done` is an agent that finished while nobody was looking. The daemon keeps the finish, so a
window opened later still says `done`, and it lasts until a window that has the keyboard shows
the pane - which clears it for every window - or until the agent works, waits on somebody, or
says it is waiting on its own work. A daemon that restarts forgets it, so a script should not expect a `done` to outlive one.

The state column is coloured: `working` cyan, `blocked` yellow, `done` green, `waiting` blue. `idle` and
`unknown` are dimmed, because they are the resting answer and the row already prints the word. It is the same legend the window itself paints, where `blocked` is orange - the sixteen
colours a terminal has hold no orange, and yellow is the nearest slot.

**These are fixed, and the window's are not.** `[colors] agent_*` repaints the window; this
answer keeps the terminal's sixteen whatever that file says, so `muster window` reads the same on
anybody's machine. What it names is a slot rather than a pixel, so repainting `[colors] palette`
does move what you see here - that is your terminal's own vocabulary, which every program in it
shares.

## Watching instead of reading

`muster window --watch` keeps answering. It prints a line for every pane as it stands, then a line
each time a pane's agent changes state or a pane closes, and runs until it is stopped:

    p1w3r07bsd  unknown
    p1w3r0ab2n  working
    p1w3r0ab2n  blocked
    p1w3r0cd4x  closed

Pane name first, so `grep --line-buffered p1w3r0ab2n` follows one agent. Every pane is watched,
including panes made after the watch began. A line is printed when a pane's state or its `since`
changes, so an agent reporting the state it is already in prints nothing.

A daemon gets a line when it stops answering and another when it is back, with its name where a
pane's would be:

    devenv  stale
    devenv  connected

Nothing about its panes arrives between the two. A daemon that is not `connected` when the watch
starts gets its line first, ahead of every pane, because what the window holds for its panes is a
guess. A watch on a window whose daemons are all answering prints only panes.

Under `--json` each line is an object: `{"pane", "daemon", "state", "since"}` for a state, with
`since` as in `panes[]`, `{"pane", "daemon", "closed": true}` for a pane that went, and
`{"daemon", "state", "detail"}` for a daemon, as in `daemons[]` - the one line with no `pane`.

The watch holds one connection to one window, so outside a pane with several windows open it
refuses until `--socket` names one. It ends with exit 3 if the window quits under it.
`muster pane wait` is the same watch narrowed to named panes and ended by a state, or with exit
4 when one of their daemons stops answering; `muster docs agents` has both.

## daemons[]

One entry per machine this window is attached to. A machine the config names that is still being
attached has none yet, and with nothing attached at all the plain answer says so.

- `daemon` is the machine's name, the `id` of its `[[daemon]]` block or `local`.

- `state` is `connected`, `stale` or `disconnected`, and `detail` says why for the two that are
  not `connected`. Read this before acting on the rest: everything above comes from Muster's
  picture of each daemon, and an hour-old picture looks exactly like a current one without it.
- `host` is where it runs, empty for this machine.
- `socket` is the path this window reaches it on. Over SSH that is the near end of the forward
  rather than the path over there, because it is the one you could dial from here.
- `started_by_muster` says whether this window started the daemon or attached to one that was
  already answering. The second is ordinary and is the one worth knowing: a Muster launched
  today adopts a daemon started yesterday if it is still answering, so what is in it may
  predate the window.
- `panes` and `directories` say how much it holds and where.

The last three are here so that ending a daemon is a decision about what it holds rather than about
its age. Age picks the wrong process: of twenty daemons measured on one machine, when Muster ran
herdr, the one holding somebody's live agent was neither the oldest nor the youngest.

# The daemons on this machine

`muster window` is about one window. It cannot say which daemons are on this machine that no
window is attached to, and those are the ones that accumulate: measured on one machine when
Muster ran herdr, twenty daemons alive, nineteen holding nothing, and one holding somebody's live
agent.

    muster daemons

    answering · 3 pane(s) in ~/src/muster, ~/src/rad · this window
      /Users/you/.muster/daemon/release.sock
    answering · holding nothing
      /Users/you/.muster/daemon/dev-3f9a0c41be27.sock

    End one with: kill $(lsof -t <socket>.lock)

Every row is a daemon **Muster started**, checked by dialing its socket rather than believed
from the file. A daemon Muster adopted is somebody else's to account for; `muster window` names
it while this window is using it, and Muster has no standing to tell you what it holds after
that.

- `socket` is the path the daemon listens on, the one its row prints.
- `state` is `answering`, `silent`, `gone` or `herdr`. `answering` replied when it was dialed.
  `silent` has a socket file nothing answers on, which is a daemon that ended without tidying up.
  `gone` has no socket file left, and it is the one case Muster cannot resolve for you: a daemon
  whose socket path was deleted out from under it is still running and unreachable, and looks
  identical to one that ended. `herdr` is the daemon a Muster from before it had one of its own
  started, still listening: its panes run on, no window of this Muster shows them, and it is not
  asked what it holds. Its row says how to end it, with `kill $(lsof -t <socket>)`, because it
  keeps no `.lock` file.
- `panes` and `directories` say what an answering daemon holds. This is the row that decides
  anything - a count of zero is a daemon you can end and lose nothing.
- `attached_here` says whether the window answering is using it. A window can only speak for
  itself, so `false` means "not this window" rather than "nothing". With more than one window
  open, pass `--socket` to hear one window's answer; `muster docs limits` says why.
- `started` is when Muster started it. It is there to be recognised, not sorted by: age is
  exactly what picks the wrong process.

**Nothing here ends a daemon, and nothing ever will.** A process holding somebody's live agent
is the wrong thing to reap on a schedule, in a tool whose promise is that agents outlive the
app. The census exists so that ending one is deliberate.

`remembered` in the `--json` answer is `false` when Muster has nowhere to write records, and
then the empty list means nothing was written down rather than that the machine is clean.
