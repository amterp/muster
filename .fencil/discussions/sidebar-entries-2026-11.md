+++
id = "sidebar-entries-2026-11"
title = "Taller agent list entries, a narrower list, and helpers as pills"
date = 2026-11-09
+++

## What prompted it

On 2026-11-09, a day after direction A's two-line rows landed (`ui-redesign-2026-11`), the owner
asked for two changes to the agent list: entries taller, so the list could be narrower than its
280 pt default, and a pane's helpers as compact vertical pills under it, like the Foreman app, in
one flat list that includes helpers of helpers. He asked for mock-ups on a Lectern page,
`muster-sidebar-entries` (version 8 holds every board), and answered its questions in the page's
threads.

Measured at 200 pt, direction A's row left its first line 64 pt for the callsign and name, because
two keycaps and the state icon sat left of the text, and it dropped the directory.

Later the same day he said the working icon, a ring three quarters closed that did not move,
"looks like it's supposed to be spinning, but it doesn't", and asked for alternatives.

## Options weighed

For the entry, three ways to give the text the list's width:

- **Stack.** The chord on a line of its own above the callsign.
- **Hang.** The chord and icon hang in a narrow left column beside three lines of text.
- **Edge.** Only the icon sits left of the text; the chord moves to the right end of line one.

The owner chose Edge and asked for a fuller mock with long names and panes on other machines.
That mock added a task line, and four ways to fill it for a pane with both a name and a harness
title: A the name, B the title, C both on one line, D both on two lines. He chose A.

For helpers: pills in the order the helpers were made, or most urgent first as Foreman sorts
them; and whether an entry with no helpers keeps an empty line so the list never moves.

For the working icon, seven candidates: A today's ring, B a play triangle, C a ring with a small
gap that turns slowly, D a ring with a pulsing dot, E a bead going round a faint ring, F three
dots brightening in turn, G a half-filled ring that never moves. The owner did not want the play
triangle ("makes [me] feel like I need to press play") and asked for motion that is less
eye-catching. C was given two controls on the page, its gap and its seconds per turn.

## What came of it

- `agent-list-rows`: Edge, the task line and the machine tag.
- `sidebar-default-width`: 200 pt.
- `helper-pills`: pills on the maker's entry, in creation order.
- `working-icon`: the turning ring.
