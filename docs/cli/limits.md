# What this cannot do

## A pane on another machine cannot reach the window

`$MUSTER_SOCKET` is a unix socket path on the machine the window is running on, so Muster sets
it only in panes held by a daemon on that machine. A program in an SSH devenv pane correctly
concludes it is not in a window it can drive. That pane can still be addressed by name from a
local pane, and `muster window` still describes it.

`$MUSTER_PANE` *is* set over there, so such a pane knows which pane it is and has no way to say
so. Closing this means forwarding the endpoint over the ssh master Muster already opens, and
putting a `muster` on the far machine for it to reach - the command is built from this repo and
nothing ships a Linux one.

## A pane Muster did not create cannot say which pane it is

A pane made by another herdr client, and a pane herdr restored after a daemon restart, both have
Muster names and can be addressed by anybody. Neither has anything in its environment: herdr
rebuilds a restored pane with no launch environment at all. So `muster` run inside one falls back
to the pane the window's keyboard is on, which is usually not the pane you meant. Pass `--pane`.

## A pane is not told which tab it is in

`$MUSTER_PANE` says which pane a command is running in; there is no `$MUSTER_TAB`, because
nothing has to tell a tab which tab it is. So a script that means "the tab I am in" reads it
out of the window rather than out of its environment:

    muster tab rename --tab "$(muster window --json | jq -r \
      --arg me "$MUSTER_PANE" '.panes[] | select(.pane == $me) | .tab')" 'the build'

`muster tab rename` with no `--tab` is not that. It means the tab the window's keyboard is in,
which is what a chord means and is a different tab whenever the keyboard is somewhere else.

## A pane in a tab nothing is showing cannot be split

`muster pane new` refuses with "the daemon local is not showing that pane or tab in this window"
when the pane it would split is in a tab no region has on screen. Splitting, focusing, zooming and
resizing all ask the window which region holds the pane, and a pane in a background tab has none.

Renaming, sending, moving and making a tab do not ask, so those reach a pane wherever it is. The
way through is `muster focus` on the pane first, which brings its tab on screen - at the cost of
taking the keyboard away from whatever somebody was doing.

## There is no search

`muster` cannot search a pane. The window can, from `cmd+f`, and reading only the last thousand
rows of a pane while saying nothing about the rest - so a match further back reads as no match at
all. Putting that into this surface would mean promising it, and the promise is one Muster cannot
currently keep.

## Closing with no --pane closes the pane you are in

`muster pane close` acts on the pane it is running in unless told otherwise, like every other
command here - which kills the shell that ran it. It is listed here because it is the one
command whose default destroys something.

## Outside a pane, two open windows are ambiguous

Each window listens on its own socket, named after its process. A caller inside a pane reaches
the right one because `$MUSTER_SOCKET` says which. A caller outside every pane has nothing to go
on, so with two windows open `muster` refuses and names the sockets that answered. Pass
`--socket` to pick one.

## A zoom with nothing to zoom still succeeds

`muster zoom` in a tab holding one pane exits 0 and changes nothing. A single pane already fills
its region, so there was nothing to hide and nothing went wrong; the run log names the daemon's
own reason at info level if you want to see it. A change a daemon would not make does exit
non-zero with what it said on stderr, so this is the one answer that reads like a refusal in the
log and is a success on purpose.

## The window's answer is a mirror

Everything `muster window` reports is Muster's picture of each daemon rather than the daemon's
own answer. `daemons[].state` says how much of that picture to trust.
