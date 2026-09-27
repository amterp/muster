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

## A pane restored after a daemon restart cannot say which window it is in

A daemon that restarts brings each pane back under its name, with a fresh shell, and gives it
`$MUSTER_PANE` but not `$MUSTER_SOCKET`: that names a window, and the window that asked for the
pane may be long gone. So `muster` run inside a restored pane knows which pane it is and not
which window to tell, and finds a window the way a command outside every pane does (below). With
one window open that is the right one; with two, pass `--socket`.

## A pane is not told which tab it is in

`$MUSTER_PANE` says which pane a command is running in; there is no `$MUSTER_TAB`, because
nothing has to tell a tab which tab it is. So a script that means "the tab I am in" reads it
out of the window rather than out of its environment:

    muster tab rename --tab "$(muster window --json | jq -r \
      --arg me "$MUSTER_PANE" '.panes[] | select(.pane == $me) | .tab')" 'the build'

`muster tab rename` with no `--tab` is not that. It means the tab the window's keyboard is in,
which is what a chord means and is a different tab whenever the keyboard is somewhere else.

## A read stops at 4 MiB

`muster pane read` answers with everything a pane still holds, as far back as its scrollback goes,
up to 4 MiB of text: the most the daemon puts in one answer. A pane holding more comes back as its
newest 4 MiB, and `truncated` in the `--json` answer is the only thing that says the oldest rows
were left out. A caller that reads the text and not that flag will conclude it has seen the whole
pane.

`--rows N` is a count of rows the pane printed, and it is answered by Muster rather than by the
daemon: every read asks for the whole history and the last N are taken here. That costs the
pane's history on the wire per read, which is the price of the flag meaning what it says - a
daemon counts rows of the *grid*, so the blank space under a quiet pane is rows to it, and asking
it for a small number would buy those and answer with nothing at all.

## A send that exits 0 was taken, not necessarily received

`muster pane send` exits 0 when the daemon took the request. That is not the same as the
program in the pane having received the text, and two things routinely make them differ.

A terminal in **canonical mode** - anything reading stdin without a line editor of its own,
`cat` and a shell script's `read` among them - accepts a line of at most 1024 bytes including
its terminator, and **discards a longer one whole** rather than cutting it. The screen still
echoes the first thousand-odd characters, so a pane read afterwards looks like a message that
arrived and stopped. It did not arrive at all. This is the receiving terminal's limit rather
than Muster's or the daemon's, and a program that has taken the terminal for itself, which every
agent harness has, is not held to it. An interactive shell is not affected - readline and zle run
in raw mode.

And a **harness that reads the text as a paste** may leave it unsubmitted. `--enter` presses
Return, and whether Return submits is the harness's to decide: Claude Code has been measured
taking 1583 bytes on one line as `[Pasted text #2]` and sitting there until a person pressed
Return. Muster cannot fix that from here and will not special-case one harness.

`--confirm` is what to reach for when it matters. It reads the pane back after the send and
exits non-zero if what was sent is not on it, so a discarded line becomes a refusal rather than
a success. A send is taken before the pane can have drawn it, so the read is retried for up to
a second rather than taken once: a pane that has already drawn the text answers immediately,
and only a genuine miss waits the second out. What it proves is **arrival, not submission**: a pane
draws the text whether it has been submitted or is sitting in an input box, so nothing readable
from out here separates those. A harness that folds a long paste into a placeholder draws
neither, which reads as unconfirmed - the honest answer, since a caller that cannot see its
message has not confirmed anything.

Newlines are safe to send. Muster hands the text to the daemon on the verb it encodes against
the pane's live modes, so a multi-line message reaches a harness fenced as one paste rather
than as a submission per line.

`--file` and `-` drop every trailing newline, as command substitution does, and keep the ones
inside the text. A request carries at most 1 MiB, so a larger file is refused with exit 1 before
anything is sent; send a line pointing at the file instead. Only a hyphen standing alone reads
stdin, which leaves one way to send a lone hyphen: `printf - | muster pane send -`.

## A non-zero exit does not always mean nothing happened

Exit 4 is a request that was taken and never answered. Either the window never answered, or it did
and said its daemon never answered it - the window gives a daemon ten seconds, and a machine loaded
enough carries out the request and answers after that. Both ways the request is on the far side, so
whatever was asked for may already have happened and only the reply went missing - which is why it
is a code of its own rather than filed under 3, "there was no window to ask", or 1, a refusal.
Retrying a 4 is how a pane receives the same instruction twice, and it has: an agent driving other
agents got one timeout on a message that had arrived, resent, and left the receiving harness with
six copies of one instruction to reconcile.

A `pane new` that exits 4 may still have made its pane. The window names a pane in the request
that makes it, so a pane that was made has its name from the start: `muster window` lists it
under that name, and the pane has the same one in its own `$MUSTER_PANE`.

What to do instead of making the request again: `muster window` shows whether it happened, and
`muster pane read --pane X` shows what is on a pane. What proves a request did *not* happen is
only exit 3, where nothing was dialled at all.

A daemon does not answer a send, so a `pane send` exits 4 only when the window itself did not
answer: the window queues the text on its input connection to the pane's daemon and answers at
once. Exit 0 says the text was queued for
a daemon the window was connected to, which is not proof that it arrived - a connection that
drops a moment later loses what was still queued on it. Exit 1 says nothing was queued, and the
message says why: the window is not connected to that daemon right now, or the daemon has stopped
reading. So a send that exited 1 is safe to send again once `muster window` shows the daemon
`connected`, and `--confirm` is what turns a 0 into proof, by reading the text back off the pane.

A window is slow to answer for reasons that have nothing to do with the request - a loaded
machine makes every one of them slower, and a devenv is a round trip away - so a 4 says more
about the moment than about the command.

## A directory for a pane on another machine has to be spelled out

`--cwd` takes a relative path and resolves it against the directory `muster` is running in,
which is a directory on this machine. So `--cwd ../other` beside `--daemon devenv` is refused
rather than resolved: the pane is going somewhere this command cannot see the filesystem of, and
often somewhere running another operating system. Give that machine's own absolute path.

The gap this leaves is `--pane`, which addresses a pane by name and does not say which machine
holds it - working that out would cost a round trip before the request. So a relative `--cwd`
beside a `--pane` that lives on a devenv is resolved against a local directory the far machine
has never had, and the daemon refuses to start a pane in a directory it cannot use, so the pane
is not made. Name the directory absolutely whenever the pane is not on this machine.

A path is tidied lexically, so `..` steps are worked out without asking the filesystem. That
matches what a shell's own `cd ..` does, and differs from `realpath` where a symlink is in the
way.

## There is no search

`muster` cannot search a pane. The window can, from `cmd+f`, but that is Ghostty's own search
running in the pane's surface, which lives in the window and which nothing out here can reach.
What `muster` has instead is `muster pane read`, the pane's history as the daemon holds it, and a
caller can search that text itself.

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

## Reattaching takes the pane from whatever is drawing it

`muster pane reattach` asks for a bridge, and a bridge asked for after the first one takes the
pane over rather than being refused. That is the whole point when the thing holding it is a
bridge whose connection died without the daemon noticing yet. But the daemon lets one bridge
draw a pane at a time and does not distinguish, so whatever else holds it loses it: a Muster
window that has not yet heard the pane's tab moved away from it, say. That one says so and stops
drawing the pane; nothing is lost.

What it cannot do is anything about the machine. It asks this window for a bridge and reaches no
daemon, so a pane on a devenv you cannot currently reach gets a bridge that fails the same way the
last one did. It also does not restart an agent: a pane whose program exited is showing an ended
process, not a dead bridge, and a fresh bridge draws exactly the same thing.

A pane no machine this window follows holds is refused rather than counted, because there is no
request on its way that could make the name right a moment later.

## A closed window comes back as the latest, or through one of its tabs

`muster window reopen`, and Reopen Closed Window in the menu, bring back the most recent window
no live window is holding. A window keeps its tabs while it is closed - its agents are still
running - so whichever window comes back, it comes back to its own tabs.

To bring back a particular one, go to one of its tabs. `muster window` lists a closed window's
tabs under its name, `window-2 (closed)`, and `muster tab focus <TAB>` naming one of them reopens
that window onto it; so does clicking a notification about an agent there. A blocked agent in a
closed window's tab is announced by the window that was in front most recently.

Arrangements are kept for the last twenty windows. A tab whose window's arrangement has gone joins
the window in front, because nothing could reopen onto it any more. A window that crashed is
treated as closed the next time anything needs to know - it keeps its tabs the same way.

## Every tab is in exactly one window

A window lists the tabs it holds and no others, and no tab is in two windows. Only one bridge may
draw a pane, so a tab two windows both listed would be one whose panes the second window took
from the first at a click. A tab made from a window is that window's. A tab no window holds joins
the window that was in front most recently, of the ones attached to that tab's machine.

So the agent list, ⌘1 to ⌘9 and `next_tab` are about this window's tabs only. The rest are under
their own window in `muster window`, and every verb still reaches them: a request naming a tab
another window holds, or a pane in one, is carried to that window and answered there, and `muster
tab focus` brings that window forward. So does clicking a notification, whichever window macOS
hands the click to. Questions are answered by whichever window was asked.

`muster tab move --tab <TAB> --window <WINDOW>` hands a tab to another window with every pane in it
still running, and without `--window` it brings the tab here. Move Tab to Window in the Tab menu does
the same, and so does dragging a tab's caption into another window's agent list. A window holding
a single tab draws no caption, so that tab moves by the menu or the command. A pane on its own
cannot be dragged to another window.

So `muster window new` is not a way to look at the same agents twice: the window you ask for holds
nothing until it makes a tab of its own.

Which window holds each tab is written in `~/.muster/state/holding/tabs.toml`. Deleting it while
windows are open costs them nothing, because each one writes itself and its tabs back. It costs the
closed windows their tabs, which then join the window in front.

## Outside a pane, two open windows are ambiguous for a change that names nothing

Each window listens on its own socket, named after its process. A caller inside a pane reaches
the right one because `$MUSTER_SOCKET` says which. A caller outside every pane has nothing to go
on, so with two windows open a change that names no tab or pane - `pane new` with no `--pane`,
`focus`, `zoom` - refuses and names the sockets that answered. Pass `--socket` to pick one.

A change that does name one goes to whichever window answers first, which carries it to the window
holding that tab. `tab move` names enough when it gives both `--tab` and `--window`.

Questions do not refuse. `muster window` and `muster pane read` answer for every window that is
listening, because naming none of them is what "what is everything doing" means. Their output
grows a heading per window when more than one answers, and `--json` becomes `{"windows": [...]}`
with each window's ordinary answer inside - so `.windows[].panes[]` reads across all of them.
With one window open, both are exactly what they were.

## A zoom with nothing to zoom still succeeds

`muster zoom` in a tab holding one pane exits 0 and changes nothing you can see. A single pane
already fills the tab, so there was nothing to hide and nothing went wrong. A change a daemon
would not make does exit non-zero with what it said on stderr.

## The window's answer is a mirror

Everything `muster window` reports is Muster's picture of each daemon rather than the daemon's
own answer. `daemons[].state` says how much of that picture to trust.

## `since` starts when the window first saw the pane

A daemon's events carry no time, so Muster stamps a pane's `since` when it hears the agent change
state. A pane whose agent was already working before this window opened says it has been working
since the window first saw it. A window that loses a daemon and reconnects stamps any pane whose
state changed during the gap with the moment it got back, since nothing says when in the gap it
changed. Two windows can therefore give one pane two different `since` values.

## A watch hears nothing about a stale daemon's panes

`muster window --watch` and `muster pane wait` are told what the window is told. While a daemon is
`stale` - a dropped VPN, a devenv restarting - the window hears nothing about its panes, so neither
does a watch. A watch prints a line when the daemon goes stale and another when it is back, and a
wait on one of its panes exits 4, however briefly the daemon was gone. When the daemon comes back,
a pane that changed during the gap is heard as one change, stamped with the moment the window got
back, and anything it did in between - a finish followed by new work - is not heard at all. If the
window quits, a watch ends with exit 3.

`pane new` and `tab new` print a pane's name once the window has heard of the pane, so the next
command can name it. A wait on a name no pane has is refused at once.
