# Running agents from a pane

An agent in a Muster pane can make panes, start agents in them, and tell them what to do. It
needs nothing but `$MUSTER_PANE` and `$MUSTER_SOCKET`, which Muster already set.

Make a pane below this one, running an agent, called something a person can pick out:

    A=$(muster pane new --down --run claude --name '🤖 A')

`pane new` prints the name of the pane it made - `p1w3r0ab2n` - once the window has heard of it,
which is what makes the next line possible:

    B=$(muster pane new --down --run claude --name '🤖 B')
    muster pane send --pane "$A" 'read AGENTS.md, then implement the parser' --enter
    muster pane send --pane "$B" 'read AGENTS.md, then write the tests' --enter

Neither split moved the keyboard. Making a pane is not the same act as looking at one, and an
agent opening three panes should not drag somebody's cursor through all three. `--focus` asks
for it.

`--run` waits for the new pane's shell to print a prompt before typing, so a program still
starting up does not miss the command. The wait belongs to the window rather than to the caller,
which is why the command travels with the split instead of arriving as a `pane send` afterwards.

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
at. `--timeout` gives up with exit 5, and leaving it off waits for as long as it takes. Give
`--pane` more than once to wait for the first of several agents.

A wait also ends when the daemon holding its pane stops answering - a devenv over a dropped VPN -
because nothing the agent does reaches the window until the daemon is back. It exits 4 at once
rather than sitting out `--timeout`, since 5 would say the agent is still working and nobody can
tell. Waiting changes nothing, so run it again once `muster window` shows that daemon `connected`.

`--until` is a condition rather than an event, so a pane already there answers at once. After
handing an idle agent work, wait for it to start before waiting for it to stop:

    muster pane send --pane p1w3r0ab2n 'read brief.md and start' --enter
    muster pane wait --pane p1w3r0ab2n --until working --timeout 60 &&
      muster pane wait --pane p1w3r0ab2n --until idle,blocked

To follow several agents at once, watch the window instead:

    $ muster window --watch
    p1w3r07bsd  unknown
    p1w3r0ab2n  working
    p1w3r0cd4x  working
    p1w3r0ab2n  blocked

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
when a person should read it before it runs.

Whether Return submits is the receiving harness's to decide, and exit 0 says the daemon took the
send rather than that the agent heard it. Where that matters, ask:

    muster pane send --pane p1w3r0ab2n 'yes, go ahead' --enter --confirm

`--confirm` reads the pane back and exits non-zero if what was sent is not on it. It costs a
round trip and it proves arrival rather than submission; `muster docs limits` is what it does
and does not catch.

Multi-line instructions are one send. The text reaches the harness as a single paste rather than
as a submission per line, so a brief with paragraphs in it arrives as a brief.

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
pane holds more than a read can reach.

How far back the window goes is the daemon's limit rather than a promise made here; see
`muster docs limits`.

## Rearranging what you made

Three `pane new --down` in a row give a column of four, which is rarely what somebody asking for a
grid meant. Moving fixes it without ending anything:

    muster pane move --pane p1w3r0ab2n --onto p1w3r07bsd

One verb, two outcomes, and which one you get depends on where the two panes already are. In the
same tab they trade places. In different tabs the pane joins the other's tab, immediately after it.
The window works that out from the panes rather than asking you to say, so a script that knows
where it wants an agent does not also have to know how it got there.

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

It prints the pane it made, not the tab, because the pane is what the next line needs. The tab
comes on screen whether or not the keyboard follows; `--focus` asks for the keyboard.

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

## A pane that has gone dark

A pane draws what a bridge feeds it, and a bridge dies with the connection carrying it. Muster
starts another one, and starts another after that if nothing dials - but it stops after a run of
bridges that each die on sight, because at that point something outside Muster is in the way and
spawning processes at it does not help. What is left is a pane that shows what it last painted,
takes no keystrokes, and has an agent running behind it perfectly happily:

    muster pane reattach --pane p1w3r07bsd

That asks for a bridge, and it is the only way back that keeps the agent. `muster pane close` also
gets the pane a fresh start, by ending what is running in it, which is rarely what somebody staring
at a stuck agent wants - and quitting Muster does the same thing to every pane at once.

The usual thing in the way is a herdr client from before: only one client may hold a terminal, and
one whose ssh died goes on holding it without noticing. The window says so on the pane's row in the
roster, with the command that releases it. Kill that, then reattach.

`--pane` is the pane the command is running in when you leave it out, so this also works typed into
a pane whose neighbour has gone quiet. It is safe on a pane that is working: the window builds a
fresh surface and the pane repaints.

## Moving around without a name

    muster focus --next
    muster focus --left
    muster focus --place 3

`--next` and `--previous` walk every pane the window is showing and wrap, so between them they
reach all of it. The four directions are geometric and do not wrap. `--place` takes the number
`muster window` prints beside each pane, which is the one `cmd+1` to `cmd+9` name.

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
