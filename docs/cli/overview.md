# muster

`muster` drives a Muster window from a script or an agent. Every command sends the same request
a keystroke sends, so anything a person can do to a window from the keyboard can be done from
here.

`muster --help` has the grammar. This says what the words mean.

## What a window holds

A window holds an ordered list of tabs and shows one of them, the way tabs work everywhere else.
A tab holds a tree of panes, and panes are where programs run.

A daemon owns the panes on one machine. One window can show panes from several daemons - a
laptop beside an SSH devenv - and a tab can hold panes from more than one of them, side by side.
Those side-by-side parts are the tab's *regions*, one per machine with panes in it, and most tabs
have exactly one. `muster pane move --tab` is what puts a second machine in a tab.

Every tab is in exactly one window. A window lists its own tabs and no others, and `muster tab
move` hands one to another window. Any command works from any window all the same: one naming a
tab or pane another window holds is answered by that window.

Panes outlive the window: quitting Muster leaves the daemons running, and the agents in their
panes keep working - and the window keeps its tabs, so reopening it comes back to them, and
`muster` in a pane made before the relaunch reaches the window open now.

## Pane names

A pane's name looks like `p1w3r07bsd`. Muster mints it rather than borrowing the daemon's own
id, and it is unique across every machine a window shows - so a name is a complete address and
needs nothing beside it.

Every command that acts on a pane takes `--pane`. Leaving it out means the pane the command is
running in, read from `$MUSTER_PANE`, and failing that the pane the window's keyboard is on.

Muster sets `$MUSTER_PANE` in every pane it creates. A pane created by something else has a
name and can be addressed by anybody, but has nothing in its environment - see `muster docs
limits`.

## Tab names

A tab's name looks like `t1w3r07bsd`. The same registry mints it, on the same terms: unique
across every machine, so `muster tab focus t1w3r07bsd` needs nothing beside it. The leading
letter says which noun, so a tab's name can never be mistaken for a pane's.

The difference is that nothing tells a tab which tab it is: there is no `$MUSTER_TAB`. Muster
names a tab before it is made and passes the name in the request that makes it, as it does a
pane's. A tab holding panes on two machines is still one name, held on both daemons. To act on
the tab a
script is sitting in, read the name out of `muster window`, where every pane says which tab holds
it - see `muster docs limits`.

`muster tab focus` needs a name, because there is no "the tab I am in" to fall back on.
`muster tab rename` without one means the tab the window's keyboard is in, which is what the
menu item means, and `muster tab move` without one means the tab the window is showing.

## Machine names

A machine's name is the `id` of its `[[daemon]]` block, and `local` when your config names
none. `muster window` lists each machine after the tabs, with its state, and puts the name at
the end of every pane's row once more than one machine is attached. `--json` carries it on
every pane as `daemon`.

`pane new` and `tab new` take `--daemon ID`, which says *where* rather than what to grow from -
so it ignores `$MUSTER_PANE` rather than sending the pane you are sitting in back to the machine
you were leaving:

    muster pane new --daemon devenv --run claude

On a machine already showing panes this splits the one that machine's region has the keyboard
on. On a machine showing nothing it opens the first pane there, which is the only reason the
flag exists: a pane's name is already a complete address, so the machine is worth naming only
when you have no pane on it.

That is the state a devenv is in the day you name it in your config, and the state your own
machine is in the moment you close its last pane. The window fills such a machine on its own
as soon as it says it holds nothing, so most of the time there is nothing to do; `--daemon` is
how a script says it outright, and how you ask again if a daemon refused.

Beside `--pane`, `--daemon` puts the new pane on that machine in the named pane's tab, which is
what right-clicking a pane and picking a machine does:

    muster pane new --pane p1w3r07bsd --daemon devenv --run claude

If the tab already has panes on devenv, the new pane splits the one devenv's region has the
keyboard on, on the side you asked for. If it has none, devenv joins the tab as a new region
beside the pane's: to its left for `--left` or `--up`, to its right for `--right` or `--down`,
because a tab lays its machines side by side and Muster keeps no split tree across them.

## Which window

Every window of an app is a window of one process, which listens on one socket. `$MUSTER_SOCKET`
names it, and Muster sets it in every pane it creates, on an SSH machine as well as this one. Over
there it is a path on that machine, which the app's ssh connection carries back, so every verb
works from a devenv pane exactly as it does here. Without it, `muster` looks for the app listening
under `~/.muster/state`.

The socket says which Muster, and the window is decided after that: `--window NAME` when it is
given; inside a pane, the window holding that pane's tab; outside every pane, the window in front.
A request naming a tab or a pane is answered by the window holding it, whichever that is. So from a
plain terminal, `muster window new` prints the new window's name and `muster --window window-3 pane
new` makes a pane there.

Only one app runs per install under one home: launching Muster again, with `open -n` or from
Finder, hands what it was asked to do to the app already running. Two installs - a development
build beside the release - are two apps with a socket each. With both listening, a change that
names its tab or pane goes to either, a change that names nothing refuses rather than guessing, and
`muster window` answers for both, headed by which app each answer is from. `muster pane read`
answers from the app holding the pane, and `muster daemons` answers for both. `--socket PATH` names one
outright.

`muster window list` lists the open windows under this `MUSTER_HOME`, marking the one this command
is running in, and `--closed` the closed ones. A Muster launched with a home of its own is not in
that list and is reached by spelling out its socket. `muster window new` asks the running app for
another window, waits for it to open, and prints its name: `window-3`. Its first tab is a new one
on the first machine here; `--daemon devenv` asks that machine for it instead, and `--tab
t1w3r07bsd` opens the window onto a tab that already exists, taking it from whichever window held
it.

`muster window reopen` brings back the window you closed last, and `muster window reopen
window-2` a particular one, each printing the name the same way. The two verbs differ in one
thing: a window you ask for holds nothing until it makes a tab of its own, and remembers it under
an arrangement nothing has ever held; a reopened window comes back to its own arrangement and the
tabs it kept. Going to one of its tabs, `muster tab focus <TAB>`, reopens a closed window too.

`muster window close window-2` closes a window as its close button does, and prints its name once
it is closed; with no name it closes the window this command is about. The last window open is
refused rather than closed, because closing it would quit Muster - `cmd+q` does that.

With no app running, `window new` and `window reopen` start one. Quitting is not closing: every
window open when Muster quits, or crashes, opens again at the next launch, and only a window you
close stays closed.

Names are not a window's: every window calls the same pane the same thing, because a name is the
daemon's. Tabs are a window's, and that follows from the daemon's rule rather than the window's -
one bridge may draw a pane, so a pane one window is drawing is a pane another cannot draw at the
same time. Which window holds each tab is written down in the install's state directory,
`~/.muster/state/<install>/holding/tabs.toml`, for the next launch to read.

A pane moves to another window with its tab, `muster tab move --tab <TAB> --window <WINDOW>`, and
never to another machine: a pane is a process and it lives where it lives.

Muster puts `~/.muster/bin` at the front of the `PATH` of every pane it makes, which is why
`muster` is there to run at all. That directory holds a link to the command belonging to the
running app, refreshed at every launch. A login shell rebuilds `PATH` from your profile
afterwards and can move it, so front is what Muster asks for rather than a guarantee. A profile
that drops it altogether, as Debian's does, still leaves `muster` at the end of the `PATH`,
which Muster appends once the profile has run; that is how a pane on an SSH machine finds it.

Little rides on which copy wins. Every one of them finds the app through `$MUSTER_SOCKET`, so
a Homebrew `muster` inside a pane drives that pane's window exactly as the app's own does;
what differs is the build, and only while the two are different versions.

Outside a pane it is whatever your own `PATH` finds. A Homebrew install puts one there
pointing into `/Applications`; from a build of your own, add `~/.muster/bin` to your `PATH`.

## With no window

Four verbs work with no window at all: `muster window` (and `window --watch`), `pane read`, `pane
send` and `pane wait`. What agents are doing and what a pane printed are the daemon's to know, and
a window only relays them, so when no window answers these ask the daemon holding the panes: the
one `$MUSTER_DAEMON_SOCKET` names, which every pane has, and otherwise this install's. That is
what makes them work on an SSH devenv nothing forwards a window to, and in a pane whose window has
quit. They answer as a window would: the same text, the same `--json`, the same exit codes, the
same `--confirm` read-back and the same wait. Panes are named as a window names them.

They fall back only when there is no window to ask: `$MUSTER_SOCKET` names one that does not
answer, or, with it unset, none is listening. A Muster named with `--socket` that is not there is
refused, and so are two apps with nothing saying which, and a window named with `--window`. `--no-window` asks the daemon even
with a window open.

`muster window` says when the daemon answered: its first line names the daemon, and `--json`
carries `"answered_by": "daemon"`, which a window's answer never does. The daemon has no places,
no keyboard and nothing on screen, so those are left out rather than made up; a tab has no label
unless somebody gave it one, and the machine is called by its host name. The other three print
exactly what they print through a window, since their answer means the same thing either way.
With no window there is no keyboard, so a pane is named with `--pane` or `$MUSTER_PANE`.

Every other verb needs a window, and says so. Making, splitting, arranging, resizing, zooming and
moving panes and tabs is laying out a window's tabs, which a window composes from its regions;
focus, the asking chord, font, the sidebar and reload are about what a window shows; which window
holds a tab is a window's record; and `muster daemons` marks which daemons that window is using.
`pane rename` and `pane close` are the daemon's to do and could work without a window, but do not
yet.

## Output

Plain output is for a person to read. `--json` answers the same thing for a program, and colour
goes to a terminal only - a pipe, a file, or `NO_COLOR` gets none. A refusal goes to stderr
either way, under `--json` as `{"error": "..."}`, so stdout holds the answer or nothing.

A few verbs have no page of their own, because `--help` says all there is: `muster pane rename`
names a pane, `muster zoom` fills a region with one pane and puts the others back, `muster tab
close` closes a tab, `muster reload` reads the config again, `muster sidebar` shows or hides the
agent list, `muster font larger`, `smaller` and `reset` size the text of the pane the keyboard is
on, and `muster completions <shell>` prints a completion script. `muster docs` lists these pages.

## Exit codes

| code | meaning                                       |
| ---- | --------------------------------------------- |
| 0    | it happened                                   |
| 1    | the window refused, and said why on stderr    |
| 2    | the command line was wrong                    |
| 3    | there was no window to ask                    |
| 4    | it was taken and never answered               |
| 5    | a wait ran out before its pane got there      |

They differ in whether the request happened, which is what decides whether to send it again.
A refusal will be refused again; no window may only mean Muster is not open yet, so 3 is the
one to retry on. 5 is `pane wait` giving up, and waiting changes nothing, so waiting again is
harmless.

**4 is the one not to.** The request reached a window, or the daemon behind it, and only the
answer went missing, so whatever was asked for may already have happened - a `pane send` retried on
a 4 is how a pane receives the same instruction twice. `muster pane read` says what a pane has on
it, and `pane send --confirm` asks the window itself rather than guessing out here.

`pane wait` exits 4 as well, when the daemon holding its pane stops answering: the window took the
wait and cannot say whether the pane got there. A wait changes nothing, so this 4 is safe to retry,
once `muster window` shows the daemon `connected` - before that, the wait ends the same way at
once.
