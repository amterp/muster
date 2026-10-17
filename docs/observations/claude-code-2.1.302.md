# Claude Code 2.1.302

The box Claude Code 2.1.302 leaves above its prompt once its feedback tool has drafted a bug
report, which held a pane's wakes for half an hour.

Seen 2026-10-16 on macOS, in a Claude Code 2.1.302 pane in Muster 0.16.0. The transcript is
`corpus/claude-code-2.1.302/feedback-draft.txt`, read back from the live pane with
`muster pane read` rather than recorded by `tools/detection-capture.py`, so it is the pane's text
and not the bytes Claude Code wrote. The screen is the "claude 2.1.302" case in
`corpus/conformance/agent-detection-recorded.json`.

## 1. A drafted bug report stays in a box above the prompt until answered

After a turn in which the session drafted a bug report with the feedback tool, Claude Code drew a
rounded box above the prompt: a first row `✻ Bug report drafted: <title>…`, a few rows of the
report cut off with an ellipsis, and a last row `1 to review · 2 to send · 0 to dismiss`. The box
is drawn at the left edge. It stayed while the session sat idle, from the turn's end at 01:35:41Z
for at least the half hour Muster's log covers.

The box takes digits: 2 sends the report.

The read ends at the prompt's caret, with no bottom rule or footer under it. Whether Claude Code
draws none while the box shows, or the read cut them off, is not established; a read of the same
pane two minutes earlier, before the box showed, had both.

A second box was seen the same evening, `✦ Heads up · ... 1: Learn more  2: Knew this already
0: Dismiss`, above another pane's prompt. A message posted to that pane rang it at once, so it is
not established that every such box holds wakes, and nothing here reads that one.

## What this decides for Muster

`claude.toml`'s `feedback_draft_box` reads the box, by its last row and its foot at the left edge,
as a screen to wait out: the state stays what it was, and the doorbell types nothing into the pane
while it shows, since a ring's text holds digits. The box quoted in a turn's output is indented
and leaves the session alone. The rule says what it `shows`, so a message held up behind the box
is told as "Claude Code's box for a bug report it drafted (1 to review, 2 to send, 0 to dismiss)
covers its prompt", where it was blamed on a draft.

Before the rule, the prompt under the box read as no prompt at all, and the doorbell held two
wakes for the 30 minutes, logging "its screen is not its prompt" 536 times.
