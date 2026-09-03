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

## A read stops a thousand rows back

`muster pane read` asks the daemon for a pane's recent rows, and herdr answers with at most a
thousand of them however far back the pane goes. Asking for more is not refused and does not fail -
it comes back with the same thousand - so `truncated` in the `--json` answer is the only thing that
says there was more. A caller that reads the text and not that flag will conclude it has seen the
whole pane.

`--rows N` is a count of rows the pane printed, and it is answered by Muster rather than by the
daemon: every read asks for the whole thousand and the last N are taken here. That costs a thousand
rows on the wire per read, which is the price of the flag meaning what it says - herdr counts rows
of the *grid*, so the blank space under a quiet pane is rows to it, and a small number sent over
the wire used to buy those and answer with nothing at all.

## A send that exits 0 was taken, not necessarily received

`muster pane send` exits 0 when the daemon took the request. That is not the same as the
program in the pane having received the text, and two things routinely make them differ.

A terminal in **canonical mode** - anything reading stdin without a line editor of its own,
`cat` and a shell script's `read` among them - accepts a line of at most 1024 bytes including
its terminator, and **discards a longer one whole** rather than cutting it. The screen still
echoes the first thousand-odd characters, so a pane read afterwards looks like a message that
arrived and stopped. It did not arrive at all. This is the receiving terminal's limit rather
than Muster's or the daemon's: both carry ten thousand bytes into a program that has taken the
terminal for itself, which every agent harness has (`observations/herdr-0.8.0.md` section 25).
An interactive shell is not affected - readline and zle run in raw mode.

And a **harness that reads the text as a paste** may leave it unsubmitted. `--enter` presses
Return, and whether Return submits is the harness's to decide: Claude Code has been measured
taking 1583 bytes on one line as `[Pasted text #2]` and sitting there until a person pressed
Return. Muster cannot fix that from here and will not special-case one harness.

`--confirm` is what to reach for when it matters. It reads the pane back after the send and
exits non-zero if what was sent is not on it, so a discarded line becomes a refusal rather than
a success. It costs a round trip, and what it proves is **arrival, not submission**: a pane
draws the text whether it has been submitted or is sitting in an input box, so nothing readable
from out here separates those. A harness that folds a long paste into a placeholder draws
neither, which reads as unconfirmed - the honest answer, since a caller that cannot see its
message has not confirmed anything.

Newlines are safe to send. Muster hands the text to the daemon on the verb it encodes against
the pane's live modes, so a multi-line message reaches a harness fenced as one paste rather
than as a submission per line.

## There is no search

`muster` cannot search a pane. The window can, from `cmd+f`, and reading only the last thousand
rows of a pane while saying nothing about the rest - so a match further back reads as no match at
all. Putting that into this surface would mean promising it, and the promise is one Muster cannot
currently keep.

## Closing with no --pane closes the pane you are in

`muster pane close` acts on the pane it is running in unless told otherwise, like every other
command here - which kills the shell that ran it. It is listed here because it is the one
command whose default destroys something.

It reaches a pane wherever it is, including one in a tab the window is not showing. That used to
be refused, and the rule stopped being usable when a window came to show one tab at a time: every
pane but the handful on screen is in a background tab, so refusing them would refuse nearly every
`--pane` a script could name. What it still refuses is a pane in a session this window is not
attached to.

`muster tab close` is the same verb one level up and follows the same rule. It ends every pane
in the tab in one request, and it reaches any tab the window holds. With no `--tab` it closes the
tab the keyboard is in, which is what the menu item means.

## Only the last window you closed comes back

`muster window reopen`, and Reopen Closed Window in the menu, bring back the most recent window
no live window is holding. Every window keeps its own arrangement under
`~/.muster/state/windows/`, so closing one leaves something to come back to, and reopening it
twice in a row gets you the one before it.

What there is no way to say is *which* one, past that. The records are kept for the last twenty
windows and nothing lists them or names them; a script that wants a particular arrangement back
can point a launch at it with `MUSTER_STATE`.

## A second window opens on tabs of its own

A window you asked for asks for a workspace and opens onto the tab that comes back, rather than
onto one that was already there. It has to: the tabs a machine already holds are the ones another
window is showing, only one client may hold a terminal, and a second window opened onto one paints
nothing at all. Those tabs are still listed and `muster tab focus` still reaches them - what the
rule stops is Muster choosing one uninvited.

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
the tab, so there was nothing to hide and nothing went wrong; the run log names the daemon's
own reason at info level if you want to see it. A change a daemon would not make does exit
non-zero with what it said on stderr, so this is the one answer that reads like a refusal in the
log and is a success on purpose.

## The window's answer is a mirror

Everything `muster window` reports is Muster's picture of each daemon rather than the daemon's
own answer. `daemons[].state` says how much of that picture to trust.
