# Claude Code 2.1.301

The tip box Claude Code 2.1.301 leaves under a turn's done line, and whether it hides the prompt
from Muster's doorbell. It does not.

Seen 2026-10-17 on macOS, in a Claude Code 2.1.301 pane sitting idle in Muster 0.16.0. The
transcript is `corpus/claude-code-2.1.301/tip-box.txt`, read back from the live pane with
`muster pane read` rather than recorded by `tools/detection-capture.py`, and scrubbed: the session
belonged to another project, so its output, the tip's text, the session's name and the statusline
are replaced, and only the frame is as drawn. The screen is the "claude 2.1.301" cases in
`corpus/conformance/agent-detection-recorded.json` and `corpus/conformance/agent-prompt.json`.

## 1. The tip box is drawn above the prompt box, not over it

After a turn, Claude Code sometimes draws two rows at the left edge between the done line and the
prompt box's top rule: `✦ Heads up · ` followed by one line about the turn's work, and
`  1: Learn more   2: Knew this already   0: Dismiss`. It stays while the session sits idle. The
prompt box under it is drawn whole: top rule, caret, bottom rule and footer.

Whether a digit pressed at an empty prompt answers the box, rather than going into the prompt, is
not established; nothing here pressed one. A ring's text starts with `[`, so by its first digit
the prompt is no longer empty.

## What this decides for Muster

Nothing in `claude.toml` reads the box, deliberately. `live_prompt_box` reads the screen as idle
with an empty prompt, because the box is outside the region between the prompt box's two rules,
and the doorbell rings it: on 2026-10-16 a message posted to a pane showing this box was rung at
once (`docs/observations/claude-code-2.1.302.md`). A rule reading the box as a screen to wait out
would hold every wake for as long as a tip stays undismissed, which is until a person answers it.

So this box does not explain the wakes an idle Claude Code pane held on 2026-10-17 with "its screen
is not its prompt": Muster 0.16.0 gave that reason for every screen whose prompt it could not read,
and its log does not say which one held those.
