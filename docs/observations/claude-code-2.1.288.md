# Claude Code 2.1.288

What Claude Code does with a line typed into its prompt box while it works, which is how an
urgent post reaches an agent in a pane mid-turn (MIP-4, section 6).

Measured 2026-10-03 on macOS 26.4.1 / arm64, with Claude Code 2.1.288 logged in to a Claude Max
account, by `crates/muster-daemon/tests/daemon/claude_code_doorbell.rs`
(`an_urgent_post_reaches_claude_code_mid_turn`, run by `./dev --claude-code`). The session runs
in a pane of a daemon the test started, bypassing permission prompts, and is given a command
that runs for forty seconds; ten seconds in, the test posts to it with `--urgent`. The
transcripts are `corpus/claude-code-2.1.288/urgent-ring-*.txt`, condensed from the session's own
log under `~/.claude/projects`. Section 3 was measured the same day in a pseudo-terminal instead
(`dialog-input.txt`). The screens of an agent at work are in
`corpus/conformance/agent-prompt.json`, recorded by `tools/detection-capture.py`.

## 1. A line typed while it works is taken into the running turn

The prompt box stays on screen while Claude Code works, and takes typing. Return queues what was
typed: the line is drawn above the box with `ctrl+enter to send now`, and the box shows a faint
`Press up to edit queued messages`. Once the tool call running then returns, Claude Code hands the
line to the model beside its result, as a system reminder: "The user sent a new message while you
were working: ... Address the message above as you continue this turn." Its log records the
queue's `enqueue`, then a `remove` with reason `absorbed_mid_turn`. Nothing is interrupted, and
no new turn starts. Both transcripts show it.

## 2. Whether the model acts on it is the model's

Told by the reminder to address the line before going on, and by the line to read it now, Sonnet
read the message as soon as the command returned, and then went on with its task
(`urgent-ring-sonnet.txt`). Haiku 4.5 ran its next command first, finished its task, and read
the message only when the doorbell rang it "still unread" once it was idle
(`urgent-ring-haiku.txt`). It did the same in seven runs, with and without "and nothing else"
in its task, and read before its turn ended in an eighth, under `./dev --claude-code`. An
earlier run by hand with the default model, Opus, acted on a queued line before its next
command.

## 3. Its permission dialog ignores the ring's line, and takes a Return as "Yes"

With the Bash permission dialog up, the terminal's bracketed paste mode stays on, and the urgent
wake line written into the pane changes nothing: as one bracketed paste the dialog is redrawn
with "1. Yes" still selected, and as plain keys in one write nothing is drawn at all. Neither
picks an option, though the line holds digits, nor opens the amend field or leaves text behind
(`dialog-input.txt`). Return at the dialog takes the selected option, which is "Yes". The
dialog fires `PreToolUse` and then `PermissionRequest` for the tool.

## What this decides for Muster

An urgent ring is typed into a working Claude Code's prompt box, under the doorbell's usual
guards, and counts as taken once the box is empty again (section 1). Its line is harmless at a
dialog that opens before it arrives, and its Return is not (section 3), so the Return is pressed
only once a second look finds the box holding the ring alone. It is the transport, not a
promise that the agent stops: the wake line asks the model to read now, and Claude Code's
reminder asks it to address the line, but a small model may finish first. The `./dev
--claude-code` check holds the transport to this version and prints which the model did.
