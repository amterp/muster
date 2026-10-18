# Running agents from a pane

An agent in a Muster pane can make panes, start agents in them, and tell them what to do. It
needs nothing but `$MUSTER_PANE`, `$MUSTER_SOCKET` and `$MUSTER_DAEMON_SOCKET`, which Muster
already set.

Make a pane below this one, running an agent, called something a person can pick out:

    A=$(muster pane new --down --run claude --name '🤖 A')

The agent's session is called what the pane is called, too, where its harness allows it - Claude
Code and Codex do, through `/rename` - and a session renamed in its harness renames the pane
(`muster docs harnesses`).

`pane new` prints the id of the pane it made - `p1w3r0ab2n` - once the window has heard of it,
which is what makes the next line possible. `--json` adds the callsign its daemon drew it,
`{"pane": "p1w3r0ab2n", "callsign": "otter"}`, which is what to tell another agent or the
person to call it:

    B=$(muster pane new --down --run claude --name '🤖 B')
    muster msg post --to "$A" --file parser-brief.md
    muster msg post --to "$B" 'read AGENTS.md, then write the tests'

A message to a pane reaches the agent in it however long it is, and whenever the agent is ready:
it is rung once it is idle at an empty prompt, reads the message whole with `muster msg read`,
and can answer with a message of its own that wakes you. Claude Code and Codex are rung so far,
and never while a dialog is open (`muster docs limits`); `muster docs harnesses` says what Muster
gets from each harness. `muster docs msg` is all of it. `pane send`, below, is the keyboard: for
answering a prompt, not for telling an agent something.

Neither split moved the keyboard. Making a pane is not the same act as looking at one, and an
agent opening three panes should not drag somebody's cursor through all three. `--focus` asks
for it.

`--run` hands the command to the daemon with the split, and the new pane's shell runs it as it
starts rather than having it typed in, so a program still starting up cannot miss it. That is why
the command travels with the split instead of arriving as a `pane send` afterwards. When the
command exits, the pane drops to a shell.

`--cwd` says where the new pane starts, and a relative path means what it means everywhere else:
relative to the directory `muster` is running in. `muster pane new --cwd ../other-worktree` from
one worktree opens a pane in its sibling. With no `--cwd` the pane starts where the pane being
split is, which is what the chord does.

## Watching what they do

    muster window --json | jq -r '.panes[] | select(.state == "blocked") | "\(.pane) \(.label)"'

That is one look. To find out when an agent finishes, wait on it rather than looking again:

    muster pane wait --pane p1w3r0ab2n --until idle,blocked --timeout 600

It blocks until the pane's agent is idle or blocked, prints the pane and its state -
`p1w3r0ab2n  done` - and exits 0. `idle` is also met by `done`, which is an idle nobody has looked
at, and not by `waiting`: an agent that ended its turn to wait on a gate it started has not
finished, so the wait goes on until it has. A Claude Code whose turn ended with background shells
still running is waiting on them, since a shell that ends starts its next turn, but for 10 minutes
at most: the line saying so stays for as long as a shell lives, so after that the pane reads idle,
its turn finished, which is what a dev server left running comes to. Add `waiting` to the list to
hear of a wait too.
`--timeout` gives up with exit 5, and leaving it off waits for as long as it takes. Give
`--pane` more than once to wait for the first of several agents.

An agent that has just started reads `unknown` for about three seconds, until Muster has read
its screen, so a wait started straight after `pane new --run` answers with what the agent opened
on: `blocked` at Claude Code's folder-trust dialog, which `muster pane send --key down --enter`
answers, or `working` for an agent started with a first prompt, never an `idle` from before its
screen counted.

A wait also ends when the daemon holding its pane stops answering - a devenv over a dropped VPN -
because nothing the agent does reaches the window until the daemon is back. It exits 4 at once
rather than sitting out `--timeout`, since 5 would say the agent is still working and nobody can
tell. Waiting changes nothing, so run it again once `muster window` shows that daemon `connected`.

A wait on a pane that is or goes `stuck` ends refused, with exit 1, saying which program stopped
reading its input and how much it left unread: it gets nowhere until the program reads again, and
nothing a wait does can make it. `muster pane restart`, below, ends it and resumes its session. That is true of a wait started on a pane already stuck, so to
hear it read again, look at `muster window` rather than waiting. `--until stuck` waits for a pane
to get stuck instead, and is answered by it.

`--until` is a condition rather than an event, so a pane already there answers at once. After
handing an idle agent work, wait for it to start before waiting for it to stop:

    muster msg post --to p1w3r0ab2n --file brief.md
    muster pane wait --pane p1w3r0ab2n --until working --timeout 60 &&
      muster pane wait --pane p1w3r0ab2n --until idle,blocked

A wait can also end on how full an agent's context is, which is when an orchestrator wants to
compact a worker rather than let it run out:

    muster pane wait --pane p1w3r0ab2n --until idle --context 80

It ends once the agent says its context is at least 80% full, or once it is idle, whichever comes
first. A wait given `--context` prints the context the agent last said beside the state, whichever
ended it - `p1w3r0ab2n  working  83% context` - and `--json` adds it as `context_used`. Only a harness whose adapter reports its context can meet `--context` (`muster docs
harnesses`), and a fresh session may not have said it yet, so a wait on a pane that has not says so
on stderr as it begins and goes on waiting for the rest of what it was asked.

To follow several agents at once, watch the window instead:

    $ muster window --watch
    p1w3r07bsd  unknown  ~/src/muster
    p1w3r0ab2n  working  🤖 A
    p1w3r0cd4x  working  🤖 B
    p1w3r0ab2n  blocked  🤖 A

A line for every pane as it stands, then a line each time any of them changes state or closes,
until you stop it. Each line is written when the change happens, so a `while read` loop or a
`grep --line-buffered` acts on it straight away, and nothing that happens between two looks is
missed. `--json` makes each line an object carrying `since`; `muster docs window` has the shape.

A daemon that stops answering gets a line of its own, `devenv  stale`, and another,
`devenv  connected`, when it is back. Nothing about its panes arrives in between, so their last
lines are the last the window heard.

`blocked` is an agent waiting on somebody. Answer it by name:

    muster pane send --pane p1w3r0ab2n 'yes, go ahead' --enter

`--enter` presses Return. Without it the text sits on the pane's prompt, which is what you want
when a person should read it before it runs. On its own, `muster pane send --pane p1w3r0ab2n
--enter` presses Return and nothing else, which takes whatever a dialog has highlighted.

Text is refused when the agent's prompt already holds something, a person's draft or a line left
unsent, since the next Return would submit the two as one prompt: a `/clear` sent there runs as
words at the end of the draft. The refusal quotes what the prompt holds and nothing is typed.
Clear the prompt first, or send with `--append` to add to it. Keys, Return alone and a lone
space with `--enter`, which submits a Claude Code prompt where Return alone does not, are never
refused this way, and nor is a pane whose prompt the daemon cannot read: a dialog, a menu, a
shell.

A dialog or a menu - a folder-trust question, a plan to approve, an approval prompt - reads keys
and ignores text, so a `2` or an arrow sent as text changes nothing there. `--key` presses one:

    muster pane send --pane p1w3r0ab2n --key down --enter
    muster pane send --pane p1w3r0ab2n --key esc

A key is spelled as a keybinding in the config file is: `up`, `down`, `left`, `right`, `enter`,
`esc`, `tab`, `space`, `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`, `f1` to `f12`,
a letter or a digit, with `ctrl+`, `alt+`, `shift+` or `cmd+` in front (`ctrl+c`). Give `--key`
once per key. A send goes out in one order wherever its flags are written: the text, then each
key in the order given, then Return. Each key is pressed on its own, a moment after the last, so
the dialog has drawn what one key did before it reads the next. Read the pane first: an option's
place in the list, and which one starts highlighted, are the harness's, and a list can wrap from
its last option to its first.

A key reaches a program outside a dialog too, encoded as a terminal encodes it for that program's
modes. `--key shift+tab` is how one agent cycles another Claude Code's permission mode:

    muster pane send --pane p1w3r0ab2n --key shift+tab

Whether Return submits is the receiving harness's to decide, and exit 0 says the daemon took the
text and the keys rather than that the agent heard them. Where that matters, ask:

    muster pane send --pane p1w3r0ab2n 'yes, go ahead' --enter --confirm

`--confirm` reads the pane just before the send and again after it, and exits non-zero unless
the text shows on the pane more times than it did before, or the pane changed and still shows
it - so text the pane already showed does not confirm a send to a pane that did not change. A
send of keys or Return alone has no text to find, and is confirmed once the pane changes at all. It costs a round trip and it proves arrival rather than submission;
`muster docs limits` is what it does and does not catch. Exit 1 says the pane shows the text was
not taken. Exit 4 says the pane changed without showing it, as it does after an agent takes a slash
command, so read the pane before sending it again.

Multi-line text is one send. It reaches the harness as a single paste rather than as a
submission per line - though a harness may fold a long paste into a placeholder that waits for a
person, which is one reason a brief goes by `muster msg post` instead.

Text with quotes in it does not have to be quoted at all. `--file` sends what a file holds, and
`-` sends what arrives on stdin:

    muster pane send --pane p1w3r0ab2n --file brief.md --enter
    muster pane send --pane p1w3r0ab2n - --enter <<'EOF'
    it's slot 3's turn: read brief.md and say "ready" when you have
    EOF

Both drop the text's trailing newlines, the way `"$(cat brief.md)"` does, so a file's final
newline is not sent as part of the message; `--enter` is how you ask for Return. `--file` is the
one that works when stdin already carries the command itself, as it does when a command is piped
to a shell on another machine.

## Reading what they printed

`muster window` says what state an agent is in and what it says it is working on. Neither of those
is its output, and an agent that has stopped tells you it stopped rather than why:

    muster pane read --pane p1w3r0ab2n

The pane's text, newest row last, as far back as the window will go. `--rows 40` asks for the last
forty rows it printed, which is what checking on somebody wants. It is a count and not a ceiling:
a quiet pane sitting at a prompt still answers with its last forty rows, not with the blank space
under them. `--json` adds `rows` and `truncated` beside the text; `truncated` is how you learn
there is history the read did not reach, whether because you asked for fewer rows or because the
pane holds more than a read can reach. History the pane has already trimmed at its scrollback
limit is gone for every read, and does not count.

To collect what an agent reported when its turn ended, read the turn instead of guessing a
count:

    muster pane read --pane p1w3r0ab2n --turn

That is what the agent printed since it last went to work: from the first row that no longer reads
as the screen did at that moment, to the last row with anything on it. The turn before it is left
out, and so is the brief you sent, which the harness echoed before it started. A turn runs until
the agent is idle again, so a dialog it stopped at along the way does not start another, and nor
does a line Muster types into the agent for its own sake, such as the pane's name for the
session: the read still finds the agent's last answer. A wake for a message does start one, since
what the agent does about the message is its answer. A pane whose agent has not gone to work
since its daemon began watching it has no turn to read, and the read is refused saying so;
`--rows` still answers. `--turn` and `--rows` are one or the other.

How far back the window goes is the daemon's limit rather than a promise made here; see
`muster docs limits`.

## Compacting an agent

An agent near the end of its context can be compacted before it runs out, with what its summary
should keep:

    muster pane compact --pane p1w3r0ab2n keep the parser notes and the failing test

The daemon types the harness's own compact command at the agent's prompt - `/compact keep the
parser notes and the failing test` for Claude Code - once the agent is idle and its prompt is
empty, under the rules a message's ring is typed by, and never while it works. So an agent can
compact itself: `muster pane compact` from inside its own pane, with no `--pane`, runs once its
turn ends. Exit 0 says the daemon will type it, not that it has. A second compaction asked for
before the first is typed replaces it.

A compaction stops the agent: nothing starts its next turn. `--then` gives it one, a prompt typed
at its next idle, empty prompt once the compaction has run, so an agent compacts and carries on in
one command:

    muster pane compact keep the plan and the failing test --then "carry on with step 3 of the plan"

The prompt is one line of at most 512 bytes, as a focus is; a longer brief belongs in a file the
prompt names. A later compaction replaces a prompt still waiting along with the compaction, and a
compaction the daemon gives up typing, because something was typed into the pane after it or a
dialog opened over it, takes its prompt with it. A compaction still waiting, and its prompt, go when
the agent leaves its pane, a restart included: neither is typed into the agent that comes next,
even one resuming the same session.

While a compaction or its prompt waits, `muster window` says so on the agent's row and what it
waits for: `(compaction queued, waiting for its 1 sub-agent and its turn to end)`. `pane compact`
says the same when the agent is not at its prompt. A harness can hold its turn open until its
sub-agents finish, as Claude Code does on "Waiting for 1 background agent to finish", and the
compaction waits for that too.

What follows the command is a focus, one line, for a harness that takes one. A harness that gives
no way to compact it from its prompt, or takes no focus when given one, is refused with the reason
(`muster docs harnesses` has which), and so is a pane running no agent.

The window asks the same, with no focus: Compact Agent in the Pane menu, in a pane's and an agent
row's right-click menu, and on the `compact_pane` action, which ships with no chord.

With `compact_at = 80` in the config file, Muster also compacts an agent once it reports its
context 80% full, once each time it crosses that line. It is off unless the file says so
(`docs/configuration.md`). `pane wait --context`, above, is the way to choose the moment yourself.

## Restarting an agent

    muster pane restart --pane p1w3r0ab2n
    muster pane restart --pane p1w3r0ab2n --env HTTPS_PROXY=http://proxy:3128

This ends the agent in the pane and brings its session back in the same pane, by running what a
daemon restart would: the `resume` on its row of `muster window --json`, which is its harness's
resume with the flags the agent was started with (`muster docs harnesses`). It is the way out for
an agent that is `stuck`, and it works on any agent whose session can be resumed, so a dozen
agents can pick up a changed proxy without losing their conversations. `--env` sets a variable
for the resumed agent alone, and can be given more than once.

The daemon first throws away what waits for the agent to read, in its own queue and in the
terminal, since a stuck agent leaves kilobytes of it and its shell would run whatever is left as
a command. It refuses anything typed into the pane until it is done. Then it asks the agent's
processes to end, kills them if they have not after five seconds, waits for the pane's shell to
draw its prompt, and types the resume there. It answers once the resume is typed, a few seconds
later, saying what it typed:

    p1w3r0ab2n  ended claude and typed: 'claude' '--model' 'opus' '--resume' 'b1c2d3'

Then wait for the agent as for any you have just started, with `muster pane wait --until idle`.
A message waiting for the agent, or posted to it while it restarts, waits for the resumed one and
rings it once it is idle. A compaction queued for it does not: `pane compact` asks again.

A pane whose agent has reported no session is refused with nothing ended, because there is
nothing to resume; a harness's adapter is what reports the session. So is an agent whose harness
has no resume. A restart that stops short says where it left the pane, and the command that
resumes the session by hand.

The window asks first, naming the pane and showing the command: Restart Agent… in the Pane menu,
in a pane's and an agent row's right-click menu, on the `restart_agent` action, which ships with
no chord, and as the Restart… button on the notice a stuck agent raises. The CLI does not ask,
since an agent running it has already decided.

## Rearranging what you made

Three `pane new --down` in a row give a column of four, which is rarely what somebody asking for a
grid meant. Moving fixes it without ending anything:

    muster pane move --pane p1w3r0ab2n --onto p1w3r07bsd

One verb, two outcomes, and which one you get depends on where the two panes already are. In the
same tab they trade places. In different tabs the pane joins the other's tab, immediately after it.
The window works that out from the panes rather than asking you to say, so a script that knows
where it wants an agent does not also have to know how it got there.

To say exactly where, name a side of the other pane:

    muster pane move --pane p1w3r0ab2n --onto p1w3r07bsd --down

`--left`, `--right`, `--up` and `--down` put the pane on that side of the `--onto` pane, the two
sharing the space it had, half each, in whichever tab it is in. That is how a side by side pair
becomes one above the other, and it is what dropping a pane on another pane's edge asks for.

Both panes have to be on the same machine. A pane is a PTY its daemon owns, so there is no move
that carries one from a laptop to a devenv - `muster window` says which daemon holds each.

`--pane` is the pane the command is running in when you leave it out, like everywhere else, so an
agent can ask to be put beside another one without looking up its own name.

The second destination is a tab, named rather than pointed at through one of its panes:

    muster pane move --pane p1w3r0ab2n --tab t1w3r07bsd

**This is the one move that may cross machines**, and it is how a tab comes to hold a laptop pane
beside a devenv one. The pane does not go anywhere - it is a process and stays on its machine -
what changes is which tab it is in, and a tab is Muster's own unit rather than a daemon's. Where
in the tab it lands is not said and not asked: `--onto` is the flag that orders one pane against
another, and that one needs both in one tree.

The third is a tab of its own, which is what pulling one busy agent out of a four-way split means:

    muster pane move --pane p1w3r0ab2n --new-tab --name "the build"

One request, and nothing is created and destroyed on the way. The three destinations exclude each
other, and `--name` only goes with `--new-tab`; the tab is unnamed without it and the window
numbers it. The keyboard stays where it is in all three, because putting a pane somewhere is
arranging the window rather than going somewhere.

Panes are not the only unit. A tab is the other way to make one, and the way to put work somewhere
that does not belong in this tab at all:

    C=$(muster tab new --run claude --name '🤖 C')

It prints the pane it made, not the tab, because the pane is what the next line needs. The tab is
made behind the one on screen, listed in the agent list, and the keyboard stays
where it was; `--focus` brings it on screen with the keyboard in it, as `cmd+t` does.

To change how much room a pane gets:

    muster pane resize --pane p1w3r07bsd --right --by 0.2

Saying nothing but the direction takes the same step a held-down chord takes. `--by` places the
divider outright, as a share of what it divides, which is what a script wants: it cannot look at
the result and press again.

To stop working shares out at all:

    muster pane resize --pane p1w3r07bsd --equalize

Every pane in the tab comes out the same size, however deeply the splits are nested, in one
command. `--row` narrows it to the panes beside that one and `--column` to the panes above and
below it. None of the three takes a number, which is the point of them: the tab already says how
many panes hang off each divider, and turning that into ratios is the part an agent cannot do by
looking.

`muster window --json` says whether it landed. Every pane on screen carries a `rect` with its
share of the window, and `regions[].layout` says how the tab splits - `muster docs window` has
both.

## Out of sight

An agent that runs helpers it talks to over `muster msg` can keep them out of the window, and out
of the notifications:

    W=$(muster pane new --hidden --quiet --run claude --name '🤖 worker')

`--hidden` makes the pane in a tab of its own that no window shows, on the machine of the pane it
was split from. Nothing draws it, and its agent runs as any other does: `pane send`, `pane read`,
`pane wait`, `pane compact` and `muster msg` reach it by name. `muster window` lists it after the
window's own tabs, under `hidden`, and the agent list has it under a Hidden caption, which a click
folds down to a count and the most urgent state among its panes. A pane made beside a hidden one
with `pane new` is in that hidden tab too, so a hidden agent's own helpers are hidden with it.
`--hidden` takes no side and no `--focus`, since it is not a split and nothing shows it.

A pane already open is hidden and brought back by name:

    muster pane hide --pane p1w3r0ab2n
    muster pane show --pane p1w3r0ab2n --focus

`pane hide` with no `--pane` hides the pane it runs in. If that was the last pane of its tab the
tab closes, and its name with it; the pane keeps its own. `pane show` brings back the hidden tab
holding the pane, whole, into this window, so no pane moves between tabs. A pane you hid sits alone
in its hidden tab, so it comes back in a tab of its own, after the others, rather than where it
was, which may have been rearranged or closed since; `pane move` takes it anywhere from there. The
keyboard goes to it only with `--focus`. Going to a hidden pane any other way - its row, a notification, the chord to
whatever is asking, `muster focus` - brings it back first. Hiding a hidden pane changes nothing
and exits 0, and so does showing one that is not hidden, which with `--focus` goes to it.

Hidden is its daemon's fact, not a window's, so a hidden pane stays hidden across the app quitting,
another window opening and the daemon restarting, and a devenv's hidden pane is hidden in every
window attached to that devenv.

`--quiet`, or later

    muster pane quiet --pane p1w3r0ab2n
    muster pane quiet --pane p1w3r0ab2n --off

makes a pane raise nothing for you: no notification when it blocks or finishes or its program asks,
and nothing for the chord to whatever is asking to land on. Its state still shows on its border,
its row and in `muster window`, which marks it `(quiet)`. A message its agent addresses to you
still notifies, because addressing you was the agent's choice. Quiet and hidden are independent: a
hidden pane that is not quiet still notifies, and going to it brings it back.

The window has both too: Hide Pane and Quiet Pane in a pane's right-click menu, an agent row's,
and the Pane menu, on the `hide_pane` and `quiet_pane` actions, which ship with no chord. A hidden
pane's row offers Show Pane.

## An agent's helpers

An agent that starts agents for its own work makes each one its helper:

    W=$(muster pane new --helper --run claude --name '🤖 worker')

`--helper` makes the new pane a helper of the pane the command runs in, its maker, and the pane's
daemon keeps that on its record for good, beside hidden and quiet. A helper is made hidden and
quiet, in its maker's directory, and a maker's hidden helpers share one hidden tab: made with the
first of them, each next one added as a column as wide as the rest, and named for the maker as it
was called then, `🤖 integrator's helpers`. `muster window` lists each helper under its maker
rather than under `hidden`, and `--json` gives every pane a `maker`, so an agent finds its own
helpers with

    muster window --json | jq -r '[.panes[], .hidden[].panes[]] | .[] | select(.maker == env.MUSTER_PANE) | .pane'

`--shown` splits a helper beside its maker instead, as `pane new` does any pane, and it is still a
helper. `--notify` lets it notify you as any pane does instead of making it quiet. A hidden helper
takes no side and no `--focus`, for the reason `--hidden` takes none. `--helper-of <pane>` names
another pane as the maker, by name or id, and splits from that pane unless `--pane` says otherwise.
Outside every pane `--helper` is refused, since there is no pane for it to mean.

A maker's helpers come forward together and go back together:

    muster pane show --helpers-of integrator
    muster pane hide --helpers-of integrator

`--helpers-of` takes the maker by name or id. `pane show` brings every hidden tab holding one of
its helpers into this window, just after the tab holding the maker, or after the others when the
maker's tab is not in this window, with the helpers arranged as they were, and the
keyboard goes to the first of them only with `--focus`; a helper made while they are on screen
joins them there. A helpers' tab that only Muster has named is named again then, for the maker as
it is called now; a tab you renamed keeps your name. `pane hide` sends back every tab on screen
that holds nothing but the maker's helpers, so a helper you moved into a tab beside other panes
stays there, and `pane hide --pane` hides it alone. Showing
helpers already on screen and hiding helpers already hidden change nothing and exit 0; a pane with
no helpers is refused. Showing one helper with `pane show --pane`, or going to it from its row, a
notification or the chord, brings back the tab it shares with the others in the same way, and the
keyboard lands on the helper you asked for.

The agent list lists a maker's hidden helpers under its row rather than under Hidden, folded into
one line, `3 helpers · 1 blocked`, whose dot is the most urgent state among them; a click opens it,
and a helper's own helpers fold under it in turn. No chord reaches them, as none reaches a hidden
pane, and a helper in a tab on screen keeps its row and its number there. The maker's right-click
menu, and the line's, offer Show Helpers and Hide Helpers. A helper whose maker has closed stays
under Hidden, since there is no row to list it under.

A helper is quiet because its maker hears from it instead of you. When a helper becomes blocked or
stuck, or goes idle without having posted to its maker since it last went to work, and stays so
for 10 seconds, its daemon tells the maker once, in a message from `@muster` that says why and
what answers it (`muster docs msg`), whether or not the helper is quiet. A helper that posts its
report and then goes idle is not reported, nor is one waiting on its own work. While its maker's
pane holds no agent, because the agent exited to its shell or the helper was made from a plain
shell, a quiet helper asks for you as if it were not quiet, since nothing there hears it.

A helper and its maker can be on different machines, made with `--daemon` or `--helper-of`. The
maker is told on its own machine, which needs a window attached to both, since that is what links
them. While the maker's machine is not answering, a quiet helper asks for you as if it were not
quiet, since nobody else can act.

The relation is to the maker's pane, so an agent resumed in the same pane keeps its helpers. When
the maker's pane closes, its helpers go on running, hidden, and stop being quiet, since nobody is
left to hear from them, and you are told once, in a message from `@muster` naming each one and how
to bring them forward: `pane show --helpers-of` with the closed maker's id. They stay their maker's
helpers. A maker on another machine closing is seen by the window, which tells the helpers'
daemon; one that closed while no window was open is told of when one opens and finds the maker on
no machine.

A helper is a pane started for its maker's own work. An agent you ask to start a peer, one that
takes part of the work at its own level and answers to you, starts it with plain `muster pane
new`. Muster never infers the relation, so a pane made from inside another pane is nobody's
helper unless `--helper` said so.

## A pane that has gone dark

A pane draws what a bridge feeds it, and a bridge dies with the connection carrying it. Muster
starts another one, and starts another after that if nothing dials - but it stops after a run of
bridges that each die on sight, because at that point something outside Muster is in the way and
spawning processes at it does not help. What is left is a pane that shows what it last painted
and has an agent running behind it perfectly happily. What you type still reaches that agent; you
just cannot see what it does with it:

    muster pane reattach --pane p1w3r07bsd

That asks for a bridge, and it keeps the agent. `muster pane close` also gets the pane a fresh
start, by ending what is running in it, which is rarely what somebody staring at a stuck agent
wants. Quitting Muster ends no agent - Quit and Close Sessions is the one quit that does - and the
window that opens next gives every pane a bridge again, so quitting and reopening is a reattach of
every pane at once.

The usual thing in the way is the connection to the pane's machine: a devenv that is down, or an
ssh that has not come back. The window says so on the pane's row in the roster. Once the machine
answers again, reattach: the new bridge takes the pane over from whatever still held it.

`--pane` is the pane the command is running in when you leave it out, so this also works typed into
a pane whose neighbour has gone quiet. It is safe on a pane that is working: the window builds a
fresh surface and the pane repaints.

## Moving around without a name

    muster focus --next
    muster focus --left
    muster focus --place 3
    muster focus --asking
    muster focus --back

`--next` and `--previous` walk every pane the window is showing and wrap, so between them they
reach all of it. The four directions are geometric and do not wrap. `--place` takes the number
`muster window` prints beside each pane and goes to that pane, bringing its tab on screen. It is
not a press of `cmd+1` to `cmd+9`, whose meaning depends on the press before it.

`--asking` goes to the pane most urgently asking for somebody, which is what `cmd+shift+a` does:
`blocked` first, then a program's notification, then `done`, and within each the pane that asked
first. It prints the pane it went to, and nothing when nothing is asking, which still exits 0.
A recognized agent's own notifications are its state asking twice, so they do not count.

It reaches a pane in any window: one in another open window brings that window forward, and one
in a closed window's tab opens that window again onto it, as clicking the pane's notification
does. Going to a pane counts as looking at it only while Muster is in front, so run it again for
the next one there; with another app in front, it goes back to the same pane.

`--back` goes to the pane the keyboard was on before and `--forward` goes again, through the same
history the mouse's back and forward buttons and `cmd+opt+[` and `cmd+opt+]` walk. Every pane the
keyboard lands on is in it, however it got there, so a step back can change tabs or machines, and
a pane that has closed is stepped over. Like `--asking`, it prints the pane it went to, and nothing
at either end of the history, which still exits 0.

Tabs step too, on their own axis - `muster tab focus --next` reaches the tabs behind whatever is on
screen.

## Asking to be looked at

    muster focus

With no argument this focuses the pane the command is running in: its tab comes on screen and
the keyboard lands there. It is the one thing in this vocabulary that reaches for a person's
attention rather than for a pane.

## No workflow is implied

The commands above are an example of what these primitives allow, not a way of working Muster
expects. Muster provides panes, states, names and this surface, and has no opinion about how you
run your agents.
