# What this cannot do

## With no window open, a pane has only its daemon

`$MUSTER_SOCKET` names the app that made a pane, and the pane outlives that app. When it has
quit and the same Muster is running again - relaunched - `muster` asks the new app instead, on
this machine and on an SSH machine alike, and the app answers from whichever window holds the
pane. With no app running at all, the pane's own daemon answers what it can in the window's
place: `muster window`, `pane read`, `pane send`, `pane wait` and `pane compact` work on that
machine's own panes (`muster docs overview`, "With no window"), and everything else waits for a
window.

On an SSH machine, a pane made by Muster 0.10.1 cannot tell one Muster's windows there from
another's, so with two installs forwarding to that machine its `muster` may reach the other's
window. A pane made since carries the install in the name it is told.

## A pane restored after a daemon restart cannot say which window it is in

A daemon that restarts brings each pane back under its name, with a fresh shell, and gives it
`$MUSTER_PANE` but not `$MUSTER_SOCKET`: that names an app, and the app that asked for the pane
may be long gone. So `muster` run inside a restored pane knows which pane it is and not which app
to tell, and finds one the way a command outside every pane does (below). With one install
running that is the right one; with two, pass `--socket`. The window is still the one holding the
pane's tab, since the pane says which pane it is.

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
pane. Rows the pane has already trimmed off the top of its history, at the scrollback limit
(`scrollback_bytes`, 10 MB by default), are gone for every read and do not set `truncated`.

`--rows N` is a count of rows the pane printed, counted back from the last row with anything on
it, so the blank space under a quiet pane is not rows to it. The daemon counts them and sends only
those, so a small count costs a small answer however much the pane holds. A daemon older than the
window sends the whole history instead, and the window takes the last N itself: the same answer,
at the cost of the history on the wire.

## `--turn` is placed by the screen, and says when rows have moved under it

`--turn` starts at the first row that no longer reads as the screen did when the agent went to
work. The daemon numbers a pane's rows from the start of its history, so a row keeps its number
as the history above it is trimmed at the scrollback limit, erased, or cleared, and a long turn
still starts where it did. Two things do move rows: a change of the pane's width rewraps them, and
a clear of the screen that keeps its history takes rows out from under it. Either can put the
read's start after the turn's real one. The daemon notices both - the pane's width, and the rows
above the screen no longer reading as they did - and the read then comes back with `truncated`
set, so a caller knows the top may be missing and can read `--rows` for more. So does a turn whose
first rows were trimmed along with the history, and a turn longer than 4 MiB, which comes back as
its newest 4 MiB.

The numbering is carried to a newer Muster's daemon that takes the pane over, and starts again
only where the pane's history does: after a daemon restart, or while a program holds the
alternate screen, which keeps no history. The daemon learns where a turn began as it sees the
agent go to work, and keeps it in memory: a pane taken over by a newer Muster has no turn to read
until its agent next goes to work, and a
daemon older than `--turn` refuses rather than answering with something else. So does a window
older than it, which says to read with `--no-window` until it is updated.

## Typing into a pane: a send that exits 0 was queued, not necessarily received

This is about the keyboard. To tell an agent something, post it a message
(`muster docs msg`): it arrives whole, and the post says whom it woke.

`muster pane send` exits 0 when the window queued the text for the pane's daemon, and the daemon
answers nothing about it (below). That is not the same as the program in the pane having received
the text, and two things routinely make them differ.

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
inside the text. A request carries at most 8 MiB, so a larger file is refused with exit 1 before
anything is sent; send a line pointing at the file instead. Only a hyphen standing alone reads
stdin, which leaves one way to send a lone hyphen: `printf - | muster pane send -`.

## A message wakes an agent in a pane only at an empty prompt Muster can read

`muster msg post` wakes an agent in a pane by typing one line and a Return into it, and the
daemon types only into a prompt it has just read as empty (`muster docs msg`). Three things
follow that a sender has to plan around.

**Only Claude Code and Codex are rung.** Reading an empty prompt needs a rule in the harness's
manifest, and only theirs have one (`muster docs harnesses`). An agent of another harness in a
pane is not woken: the post says `its prompt cannot be read`, and exits 6 if nobody else heard it.
Tell that agent with `muster pane send`, or have it run `muster msg read` on its own.

**An agent at a dialog is not rung until somebody answers it.** A permission prompt, a trust
dialog or a menu is not the prompt, and the doorbell never presses Return there. The post says
`rung once idle`, and the ring waits as long as the dialog does. `muster pane wait --until
blocked` or `muster window` says that it is waiting on you, and `muster pane send` answers it.
A draft left in the prompt holds the ring the same way, until it is sent or cleared.

**The check comes just before the write, not with it.** A dialog drawn, or a key pressed, in
between gets the line and its Return. And a screen that a harness's rule wrongly reads as its
prompt - a new dialog drawn above an unchanged prompt box - is rung as one. "woke" says the line
was typed, not that the agent read it; its reply is what says that.

## A non-zero exit does not always mean nothing happened

Exit 4 is a request that was taken and never answered. Either the window never answered, or it did
and said its daemon never answered it - the window gives a daemon ten seconds, and a machine loaded
enough carries out the request and answers after that. Both ways the request is on the far side, so
whatever was asked for may already have happened and only the reply went missing - which is why it
is a code of its own rather than filed under 3, "there was no window to ask", or 1, a refusal.
Retrying a 4 is how a pane receives the same instruction twice, and it has: an agent driving other
agents got one timeout on a message that had arrived, resent, and left the receiving harness with
six copies of one instruction to reconcile.

A `pane new` or `tab new` that exits 4 may still have made its pane. The window names a pane in
the request that makes it, so a pane that was made has its name from the start: the exit 4
message says that name, under `--json` as the error's `pane`, `muster window` lists the pane
under it, and the pane has the same one in its own `$MUSTER_PANE`. So a caller can look for the
pane, or end it with `muster pane close`, rather than make a second one.

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

## A pane put on another machine goes beside its part of the tab, not above or below it

`muster pane new --pane P --daemon devenv` cannot put the new pane above or below P when P's
tab has no panes on devenv yet. Devenv joins the tab as a new region beside the one holding P:
before it for `--left` or `--up`, after it for `--right` or `--down`. A tab lays its machines
out as regions side by side, and Muster keeps no split tree across them. Once devenv has a
region in the tab, later panes split within it on the side asked for.

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

## A closed window comes back by name, as the latest, or through one of its tabs

`muster window reopen`, and Reopen Closed Window in the menu, bring back the most recent window
no live window is holding, and `muster window reopen window-2` brings back that one. A window
keeps its tabs while it is closed - its agents are still running - so whichever window comes
back, it comes back to its own tabs.

A window is closed only when somebody closes it: Close Window, `cmd+shift+w`, its close button, or
`muster window close`. Closing the last one quits instead - `muster window close` refuses it - and quitting closes nothing - every window open at a quit, or
a crash, opens again at the next launch.

A particular closed window also comes back when you go to one of its tabs. `muster window` lists
a closed window's tabs under its name, `window-2 (closed)`, and `muster tab focus <TAB>` naming
one of them reopens that window onto it; so does clicking a notification about an agent there. A blocked agent in a
closed window's tab is announced by the window that was in front most recently.

Arrangements are kept for the last twenty windows. A tab whose window's arrangement has gone joins
the window in front, because nothing could reopen onto it any more.

## Every tab is in exactly one window

A window lists the tabs it holds and no others, and no tab is in two windows. Only one bridge may
draw a pane, so a tab two windows both listed would be one whose panes the second window took
from the first at a click. A tab made from a window is that window's. A tab no window holds joins
the window that was in front most recently, of the ones attached to that tab's machine.

So the agent list, ⌘1 to ⌘9 and `next_tab` are about this window's tabs only. The rest are under
their own window in `muster window`, and every verb still reaches them: a request naming a tab
another window holds, or a pane in one, is answered by that window, and `muster tab focus` brings
that window forward. So does clicking a notification, whichever window macOS
hands the click to. Questions are answered by whichever window was asked.

`muster tab move --tab <TAB> --window <WINDOW>` hands a tab to another window with every pane in it
still running, and without `--window` it brings the tab here. Move Tab to Window in the Tab menu does
the same, and so does dragging a tab's caption into another window's agent list. A window holding
a single tab draws no caption, so any of its rows drags that tab. A pane on its own cannot be
dragged to another window.

So `muster window new` is not a way to look at the same agents twice: the window you ask for holds
nothing until it makes a tab of its own.

Which window holds each tab is written in `~/.muster/state/<install>/holding/tabs.toml`, for the
next launch. Deleting it while Muster runs costs nothing, because the app writes the whole of it
back at the next change. Deleting it while Muster is not running costs the closed windows their
tabs, which then join the window in front.

## Outside a pane, a change that names nothing reaches the window in front

Every window of an app answers on one socket, so a caller outside every pane reaches the app and
nothing says which of its windows is meant. A change that names no tab or pane - `pane new` with
no `--pane`, `focus`, `zoom` - goes to the window in front, which after `muster window new` from a
terminal may not be the new window: macOS does not always bring a window forward while another app
is active. Name the window with `--window window-3`.

## Two installs under one home are two apps

A development build beside the release is a second app, listening on a socket of its own. A
caller inside a pane reaches its own app because `$MUSTER_SOCKET` says which. A caller outside
every pane has nothing to go on, so with both running a change that names no tab or pane refuses
and names the sockets that answered. Pass `--socket` to pick one. A change that does name one goes
to whichever app answers first, and is refused there if that app's daemon holds no such tab or
pane.

Questions do not refuse. `muster window` answers for every app that is listening, because naming
none of them is what "what is everything doing" means. Its output grows a heading per app when
more than one answers, and `--json` becomes `{"windows": [...]}` with each app's ordinary answer
inside - so `.windows[].panes[]` reads across all of them. With one app running, it is exactly what
it was.

`muster pane read` and `muster daemons` are asked of every app the same way, and cannot yet show
what more than one of them answered: each heading is followed by the words `a pane's text` or `a
list of daemons`, and `--json` puts them under `unreadable`. Pass `--socket` to read one app's
answer. `muster pane wait` and `muster window --watch` refuse outright with two apps listening,
and name the sockets.

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

A wait on `--context` is met only by what an agent's adapter reports. A harness with no adapter
never reports its context, and Muster cannot tell that apart from one that has not reported yet, so
such a wait is not refused: it says on stderr that the pane has not said its context, and goes on
waiting for whatever else it was asked, its `--until` states or its `--timeout`.

## A nested agent's reports are refused only when it runs apart

Every process started in a pane inherits `$MUSTER_PANE`, so an agent that the pane's agent starts -
a `claude -p` from Claude Code's Bash tool, a `codex exec`, an agent in a tmux server started
there - has hooks that report into the pane. Muster refuses such a report when the process that
sent it sits below an agent running outside the process group the pane's own agent runs in, which
is where Claude Code's Bash tool puts what it runs. It cannot tell a nested agent that stays in that
group from the pane's own, and takes its reports as before; nor can it tell anything about a
sender it cannot see, so it takes those too. `env -u MUSTER_DAEMON` in front of the nested command
keeps its hooks quiet whatever Muster can tell.
