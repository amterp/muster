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

## A read stops a thousand rows back

`muster pane read` asks the daemon for a pane's recent rows, and herdr answers with at most a
thousand of them however many are asked for. Asking for more is not refused and does not fail - it
comes back with the same thousand - so `truncated` in the `--json` answer is the only thing that
says there was more. A caller that reads the text and not that flag will conclude it has seen the
whole pane.

The rows come off the bottom of the pane's grid rather than out of a log, which has a second
consequence: a small `--rows` on a pane sitting at a prompt can answer with nothing at all, because
the bottom rows of an idle terminal are blank. That is an honest answer rather than a failure, and
it is why `--rows` is a ceiling rather than a count.

## There is no search

`muster` cannot search a pane. The window can, from `cmd+f`, and reading only the last thousand
rows of a pane while saying nothing about the rest - so a match further back reads as no match at
all. Putting that into this surface would mean promising it, and the promise is one Muster cannot
currently keep.

## Closing with no --pane closes the pane you are in

`muster pane close` acts on the pane it is running in unless told otherwise, like every other
command here - which kills the shell that ran it. It is listed here because it is the one
command whose default destroys something.

## A second window is not reopened for you

`muster window new` opens one and it behaves like any other, but nothing brings it back after a
quit: only the window Muster comes back to remembers what it was showing, and the others are
gone. Their panes are not - those belong to the daemon and outlive every window - so what is
lost is the arrangement, not the work. Open another window and go to the tabs you want.

That one window owns the arrangement deliberately, and it is why a second window remembers
nothing rather than remembering badly: two windows writing one file meant whichever published
last decided what came back.

## A second window opens on tabs of its own

A window you asked for asks each machine for a workspace and opens onto that, rather than onto
whatever that machine last had focused. It has to: the focused tab is usually the one another
window is showing, only one client may hold a terminal, and a second window opened there paints
nothing at all.

So `muster window new` is not a way to look at the same agents twice. To reach a pane another
window is showing, go to it in that window - `muster --socket "$W" tab focus <TAB>` - rather
than opening a second one onto it.

## A name somebody typed does not cross windows straight away

Panes and tabs have the same names in every window, and a name somebody *gives* one takes longer
to arrive. The daemon announces a rename to nobody, so a second window learns it the next time it
asks the daemon what it holds rather than at the moment it happens. Muster's own names are not
affected: those are written down where every window reads them.

## Outside a pane, two open windows are ambiguous for anything that changes something

Each window listens on its own socket, named after its process. A caller inside a pane reaches
the right one because `$MUSTER_SOCKET` says which. A caller outside every pane has nothing to go
on, so with two windows open anything that changes something - `pane new`, `focus`, `zoom` -
refuses and names the sockets that answered. Pass `--socket` to pick one.

Questions do not refuse. `muster window` and `muster pane read` answer for every window that is
listening, because naming none of them is what "what is everything doing" means. Their output
grows a heading per window when more than one answers, and `--json` becomes `{"windows": [...]}`
with each window's ordinary answer inside - so `.windows[].panes[]` reads across all of them.
With one window open, both are exactly what they were.

## A zoom with nothing to zoom still succeeds

`muster zoom` in a tab holding one pane exits 0 and changes nothing. A single pane already fills
its region, so there was nothing to hide and nothing went wrong; the run log names the daemon's
own reason at info level if you want to see it. A change a daemon would not make does exit
non-zero with what it said on stderr, so this is the one answer that reads like a refusal in the
log and is a success on purpose.

## The window's answer is a mirror

Everything `muster window` reports is Muster's picture of each daemon rather than the daemon's
own answer. `daemons[].state` says how much of that picture to trust.
