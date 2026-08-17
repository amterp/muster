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

## Asking to be looked at

    muster focus

With no argument this focuses the pane the command is running in: its tab comes on screen and
the keyboard lands there. It is the one thing in this vocabulary that reaches for a person's
attention rather than for a pane.

## No workflow is implied

The commands above are an example of what these primitives allow, not a way of working Muster
expects. Muster provides panes, states, names and this surface, and has no opinion about how you
run your agents.
