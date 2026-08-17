# What this cannot do

## A pane on another machine cannot reach the window

`$MUSTER_SOCKET` is a unix socket path on the machine the window is running on, so Muster sets
it only in panes held by a daemon on that machine. A pane on an SSH devenv is told nothing, and
a program in it correctly concludes it is not in a window it can drive. It can still be
addressed by name from a local pane, and `muster window` still describes it.

## A pane Muster did not create cannot say which pane it is

A pane made by another herdr client, and a pane herdr restored after a daemon restart, both have
Muster names and can be addressed by anybody. Neither has anything in its environment: herdr
rebuilds a restored pane with no launch environment at all. So `muster` run inside one falls back
to the pane the window's keyboard is on, which is usually not the pane you meant. Pass `--pane`.

## Tabs cannot be addressed

`muster window` names a tab by its place and its label and gives no id, because the only id a
tab has is its daemon's and that means nothing on another machine. Focus a pane in the tab
instead, which brings the tab on screen.

## Closing with no --pane closes the pane you are in

`muster pane close` acts on the pane it is running in unless told otherwise, like every other
command here - which kills the shell that ran it. It is listed here because it is the one
command whose default destroys something.

## Outside a pane, two open windows are ambiguous

Each window listens on its own socket, named after its process. A caller inside a pane reaches
the right one because `$MUSTER_SOCKET` says which. A caller outside every pane has nothing to go
on, so with two windows open `muster` refuses and names the sockets that answered. Pass
`--socket` to pick one.

## The window's answer is a mirror

Everything `muster window` reports is Muster's picture of each daemon rather than the daemon's
own answer. `daemons[].state` says how much of that picture to trust.
