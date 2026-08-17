# muster

`muster` drives a Muster window from a script or an agent. Every command sends the same request
a keystroke sends, so anything a person can do to a window from the keyboard can be done from
here.

`muster --help` has the grammar. This says what the words mean.

## What a window holds

A window shows one or more regions, side by side. Each region shows one tab, and a tab holds a
tree of panes. Panes are where programs run.

A daemon owns the panes on one machine. One window can show panes from several daemons - a
laptop beside an SSH devenv - and each daemon has its own tabs. Panes outlive the window:
quitting Muster leaves the daemons running, and the agents in their panes keep working.

## Pane names

A pane's name looks like `p1w3r07bsd`. Muster mints it rather than borrowing the daemon's own
id, and it is unique across every machine a window shows - so a name is a complete address and
needs nothing beside it.

Every command that acts on a pane takes `--pane`. Leaving it out means the pane the command is
running in, read from `$MUSTER_PANE`, and failing that the pane the window's keyboard is on.

Muster sets `$MUSTER_PANE` in every pane it creates. A pane created by something else has a
name and can be addressed by anybody, but has nothing in its environment - see `muster docs
limits`.

## Which window

`$MUSTER_SOCKET` names the window a pane is drawn in, and Muster sets it in every pane it
creates on this machine. Without it, `muster` looks for a listening window under
`~/.muster/state`, and refuses rather than guessing if more than one answers. `--socket PATH`
names one outright.

Muster puts `~/.muster/bin` on the `PATH` of every pane it makes, which is why `muster` is
there to run at all. That directory holds a link to the command belonging to the running app,
refreshed at every launch. Add it to your own `PATH` for terminals outside Muster.

## Output

Plain output is for a person to read. `--json` answers the same thing for a program, and colour
goes to a terminal only - a pipe, a file, or `NO_COLOR` gets none.

## Exit codes

| code | meaning                                       |
| ---- | --------------------------------------------- |
| 0    | it happened                                   |
| 1    | the window refused, and said why on stderr    |
| 2    | the command line was wrong                    |
| 3    | there was no window to ask                    |

1 and 3 mean different things to a script. A refusal will be refused again; no window may only
mean Muster is not open yet.
