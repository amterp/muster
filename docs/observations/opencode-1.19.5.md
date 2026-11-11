# OpenCode 1.19.5

What OpenCode 1.19.5 does when Muster would compact it or resume it: what `/compact` needs typed
at its prompt box, and what `opencode --session` runs with.

Measured 2026-11-11 on macOS 26.4.1 / arm64, with opencode 1.19.5 installed by npm into a scratch
folder, running `opencode/big-pickle` with no credential in the environment and its data, config,
state and cache folders in scratch, in a 100x30 pseudo-terminal driven by scripts built on
`tools/detection-capture.py`. Each line was written as Muster types one, a bracketed paste and its
Return in one write, and what OpenCode took was read from its own database as well as its screen.
The transcript is `corpus/opencode-1.19.5/compact-and-resume.txt`.

## 1. `/compact` runs only on a second Return

`/compact` and its Return in one write, at the empty box after one turn, left the command in the
box under the list of commands it matches, `/compact` and `/review`, and started nothing. The
Return was taken by the list. A second Return, alone, ran the compaction: a summary drawn under a
`Compaction` rule, and the footer's context down from 22.6K (11%) to 566 (0%).

`/compact keep the word zebra` written the same way matched no command, so no list took the
Return, and it went to the model as a message: the database holds it as text, with no compaction.
OpenCode's `/compact` takes nothing to keep.

## 2. `--session` resumes a session with the options given beside it

`opencode -m opencode/big-pickle --agent plan --session <id>`, after a session started as
`opencode -m opencode/big-pickle` had ended, drew that session's conversation and compaction
summary, and the box's foot named the `Plan` agent where the first run's had named `Build`. The
session runs with what the resume is given, so a resume has to pass the options again.

## What this decides for Muster

Muster types a compaction as one line with one Return, so OpenCode's manifest names no `compact`:
typed that way it would sit in the box, and with a focus it would be sent to the model. Its
`resume` is `opencode {args} --session {session}`, which carries the flags the agent was started
with, its model and agent among them.
