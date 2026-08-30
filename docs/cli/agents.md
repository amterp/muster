# Running agents from a pane

An agent in a Muster pane can make panes, start agents in them, and tell them what to do. It
needs nothing but `$MUSTER_PANE` and `$MUSTER_SOCKET`, which Muster already set.

Make a pane below this one, running an agent, called something a person can pick out:

    A=$(muster pane new --down --run claude --name '🤖 A')

`pane new` prints the name of the pane it made - `p1w3r0ab2n` - which is what makes the next line
possible:

    B=$(muster pane new --down --run claude --name '🤖 B')
    muster pane send --pane "$A" 'read AGENTS.md, then implement the parser' --enter
    muster pane send --pane "$B" 'read AGENTS.md, then write the tests' --enter

Neither split moved the keyboard. Making a pane is not the same act as looking at one, and an
agent opening three panes should not drag somebody's cursor through all three. `--focus` asks
for it.

`--run` waits for the new pane's shell to print a prompt before typing, so a program still
starting up does not miss the command. The wait belongs to the window rather than to the caller,
which is why the command travels with the split instead of arriving as a `pane send` afterwards.

## Watching what they do

    muster window --json | jq -r '.panes[] | select(.state == "blocked") | "\(.pane) \(.label)"'

`blocked` is an agent waiting on somebody. Answer it by name:

    muster pane send --pane p1w3r0ab2n 'yes, go ahead' --enter

`--enter` presses Return, which is what submits the text. Without it the text sits on the pane's
prompt, which is what you want when a person should read it before it runs.

## Reading what they printed

`muster window` says what state an agent is in and what it says it is working on. Neither of those
is its output, and an agent that has stopped tells you it stopped rather than why:

    muster pane read --pane p1w3r0ab2n

The pane's text, newest row last, as far back as the window will go. `--rows 40` asks for the last
forty instead, which is what checking on somebody wants and what keeps a thousand rows off the
wire. `--json` adds `rows` and `truncated` beside the text; `truncated` is how you learn there is
history the read did not reach.

Two things to expect. The rows come off the bottom of the pane's grid, so a small `--rows` on an
idle pane can come back blank - the bottom of a terminal usually is. And how far back the window
goes is the daemon's limit rather than a promise made here; see `muster docs limits`.

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

Panes are not the only unit. A tab is the other way to make one, and the way to put work somewhere
that does not belong in this tab at all:

    C=$(muster tab new --run claude --name '🤖 C')

It prints the pane it made, not the tab, because the pane is what the next line needs. The tab
comes on screen whether or not the keyboard follows; `--focus` asks for the keyboard.

To change how much room a pane gets:

    muster pane resize --pane p1w3r07bsd --right 0.2

Saying nothing after the direction takes the same step a held-down chord takes. A fraction places
the divider outright, which is what a script wants: it cannot look at the result and press again.

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
