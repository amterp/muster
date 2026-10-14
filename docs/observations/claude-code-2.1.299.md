# Claude Code 2.1.299

How Claude Code 2.1.299 draws a finished Bash call, which detection had been reading as the
spinner of a turn still at work.

Measured 2026-10-13 on macOS 26.4.1 / arm64, with Claude Code 2.1.299 in a 100x30
pseudo-terminal driven by `tools/detection-capture.py`. The transcript is
`corpus/claude-code-2.1.299/bash-row.txt`. The screens are the "claude 2.1.299" cases in
`corpus/conformance/agent-detection-recorded.json`.

## 1. A finished Bash call is one row that looks like a spinner

Once a Bash call finishes, Claude Code draws it as one row, `· $ <command>`, and a command wider
than the pane is cut off with an ellipsis: `· $ echo the quick brown fox ... the lazy do…`. The
spinner at the start of a turn, before it has a timer, is the same glyph and the same ellipsis
around a word: `· Brewing…`. Later spinner lines carry a timer, `✻ Thinking… (3s · ...)`, and the
finished call's row has none (`bash-row.txt`).

## What this decides for Muster

`claude.toml`'s `live_turn_working` reads a spinner line without a timer only when the text
before the ellipsis is words, as in `· Brewing…`. It had accepted any text there, so a turn that
ended within twelve lines of a long Bash call stayed `working`: the Stop hook's idle report went
unconfirmed and expired, and the doorbell, which waits for an agent to stop working, held every
message to that agent until something redrew its screen. On 2026-10-13 that held two agents'
messages for about forty minutes.
