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

Panes outlive the window: quitting Muster leaves the daemons running, and the agents in their
panes keep working.

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

The difference is that nothing tells a tab which tab it is. There is no `$MUSTER_TAB`, and a tab
is named the first time a daemon mentions it rather than before it is made. A tab holding panes
on two machines is still one name: which of each machine's tabs it means is Muster's to know, and
nothing outside Muster ever needs to. To act on the tab a
script is sitting in, read the name out of `muster window`, where every pane says which tab holds
it - see `muster docs limits`.

`muster tab focus` needs a name, because there is no "the tab I am in" to fall back on.
`muster tab rename` without one means the tab the window's keyboard is in, which is what the
menu item means.

## Machine names

A machine's name is the `id` of its `[[daemon]]` block, and `local` when your config names
none. `muster window` prints it over that machine's tabs, and `--json` carries it on every pane
as `daemon`.

`pane new` and `tab new` take `--daemon ID`, which says *where* rather than what to grow from -
so it cannot be given beside a `--pane`, and it ignores `$MUSTER_PANE` rather than sending the
pane you are sitting in back to the machine you were leaving:

    muster pane new --daemon devenv --run claude

On a machine already showing panes this splits the one that machine's region has the keyboard
on. On a machine showing nothing it opens the first pane there, which is the only reason the
flag exists: a pane's name is already a complete address, so the machine is worth naming only
when you have no pane on it.

That is the state a devenv is in the day you name it in your config, and the state your own
machine is in the moment you close its last pane. The window fills such a machine on its own
as soon as it says it holds nothing, so most of the time there is nothing to do; `--daemon` is
how a script says it outright, and how you ask again if a daemon refused.

## Which window

`$MUSTER_SOCKET` names the window a pane is drawn in, and Muster sets it in every pane it
creates on this machine. Without it, `muster` looks for listening windows under `~/.muster/state`.
Anything that changes something refuses rather than guessing if more than one answers; a question
answers for all of them, headed by which window each answer is about. `--socket PATH` names one
outright.

`muster window list` says which windows are listening under this `MUSTER_HOME`, marking the one
this command is running in. A window launched with a home of its own is not in that list and is
reached by spelling out its socket. `muster window new` opens another and prints the socket that
reaches it, so the next line of a script is `muster --socket "$W" pane new --run claude`.

`muster window reopen` brings back the window you closed, and prints its socket the same way.
The two differ in one thing: a window you ask for starts on tabs of its own and remembers them
under an arrangement nothing has ever held, and this one takes the most recent arrangement no
live window is holding.

A window is a process. That is why each one has its own socket named after its pid, and why
making one starts an app rather than asking a running one for it - the case `window new` exists
for includes there being no window to ask.

Names are not a window's: two windows on one machine call the same pane the same thing, because
the names are written down where both can read them. Showing is a window's, and that is the
backend's limit rather than Muster's - one client may hold a terminal, so a pane one window is
drawing is a pane another cannot draw at the same time. A window you ask for therefore opens on
tabs of its own rather than onto what the other one is showing.

There is no move that carries a pane to another window today, and none that carries it to another
machine ever: a pane is a process and it lives where it lives.

Muster puts `~/.muster/bin` at the front of the `PATH` of every pane it makes, which is why
`muster` is there to run at all. That directory holds a link to the command belonging to the
running app, refreshed at every launch. A login shell rebuilds `PATH` from your profile
afterwards and can move it, so front is what Muster asks for rather than a guarantee.

Little rides on which copy wins. Every one of them finds the window through `$MUSTER_SOCKET`,
so a Homebrew `muster` inside a pane drives that pane's window exactly as the app's own does;
what differs is the build, and only while the two are different versions.

Outside a pane it is whatever your own `PATH` finds. A Homebrew install puts one there
pointing into `/Applications`; from a build of your own, add `~/.muster/bin` to your `PATH`.

## Output

Plain output is for a person to read. `--json` answers the same thing for a program, and colour
goes to a terminal only - a pipe, a file, or `NO_COLOR` gets none.

## Exit codes

| code | meaning                                       |
| ---- | --------------------------------------------- |
| 0    | it happened                                   |
| 1    | the window refused, and said why on stderr    |
| 2    | the command line was wrong                    |
| 3    | there was no window to ask                    |

1 and 3 mean different things to a script. A refusal will be refused again; no window may only
mean Muster is not open yet.
