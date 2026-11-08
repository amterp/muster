+++
id = "ui-redesign-2026-11"
title = "Redesigning the agent list, messages and pane chrome"
date = 2026-11-08
+++

## What prompted it

On 2026-11-08 the owner said the agent list and the rest of Muster's UI had grown without anyone
choosing their look, and named what got in his way: the state dot is hard to see, a pane's
directory is missing, and the chord's first digit is greyed out almost to nothing. He asked for
mock-ups on a Lectern page, `muster-ui-redesign` (archived after version 3, which holds every
board).

A screenshot of his window that day showed six problems in the agent list:

- Every state is the same 7 px dot, so a blocked agent weighs as much as an idle one, and working's
  cyan sits beside a callsign drawn in the blue accent.
- The context ring at each row's edge reads as a loading spinner.
- A named pane's row has no directory: `RosterPane` carries only a label, which holds the
  directory only for a pane nobody named.
- The second line repeats the harness's terminal title ("✳ RAD" under "ferret RAD").
- The chord's tab digit is drawn in the tertiary label color, the pane digit in the secondary.
- Messages lists nine `@human+<callsign>` groups, one with anything unread.

## Options weighed

For the agent list:

- **A. Signal.** Today's rows in today's order, two lines each: the whole chord as two keycaps, a
  state icon with a shape per state, blocked and stuck rows tinted with a colored edge, the
  directory's last part on line two, a context bar in place of the ring. Needs a 280 px list.
- **B. Needs you first.** A strip at the top with one card per agent that wants the person and
  what it asked, then every agent on one line with only its pane digit.
- **C. Ledger.** One line per agent in aligned columns with the state spelled out; fits fifteen
  agents, needs 400 px, and cuts names soonest.

The owner chose A with B's strip, both foldable, and every tab foldable.

The page also drew pane headers carrying each pane's chord, state and directory, a restyled group
conversation and banner, and gaps with rounded corners between panes. The owner disliked the space
the gaps cost; the gaps were a styling choice rather than something headers need, so they were
dropped. He asked whether the mock-ups' palette meant Muster would take on a theme of its own; the
answer recorded here is that it will not, and the new surfaces take their colors from settings
that already exist.
