# Claude Code 2.1.288

What Claude Code does with a line typed into its prompt box while it works, which is how an
urgent post reaches an agent in a pane mid-turn (MIP-4, section 6), what its dialogs do
with what is typed at them, and how its session is named.

Measured 2026-10-03 on macOS 26.4.1 / arm64, with Claude Code 2.1.288 logged in to a Claude Max
account, by `crates/muster-daemon/tests/daemon/claude_code_doorbell.rs`
(`an_urgent_post_reaches_claude_code_mid_turn`, run by `./dev --claude-code`). The session runs
in a pane of a daemon the test started, bypassing permission prompts, and is given a command
that runs for forty seconds; ten seconds in, the test posts to it with `--urgent`. The
transcripts are `corpus/claude-code-2.1.288/urgent-ring-*.txt`, condensed from the session's own
log under `~/.claude/projects`. Sections 3 to 5 were measured the same day in a pseudo-terminal instead
(`dialog-input.txt`, `plan-dialog.txt`, `rename.txt`), and section 6 the day after
(`background-agent.txt`). The screens of an agent at work are in
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

## 4. Plan mode's dialog asks "would you like to proceed?", and fires the permission hooks

Plan mode ends in a dialog asking whether to go ahead with the plan: "Claude has written up a plan
and is ready to execute. Would you like to proceed?", over numbered options whose first is a
"Yes" (`plan-dialog.txt`). It is a permission prompt for the `ExitPlanMode` tool: `PreToolUse`,
then `PermissionRequest`, then a `Notification` of type `permission_prompt` fire as it opens, so
Muster's hooks report the session blocked. Its question is not the "Do you want to proceed?" of a
tool's permission prompt, and in a pane narrower than it the question wraps inside its last
words.

## 5. `/rename` names the session, and the statusline hears it on its next run

`/rename <name>` typed at an empty prompt box, as one bracketed paste and a Return in one write,
or as keys, names the session and leaves the box empty: the name is drawn at the right end of the
rule above the box, and the title becomes the name after its usual mark (`rename.txt`). No menu
kept the Return. The statusline command is handed the name as `session_name`, absent until a
session has one, but a rename does not run the statusline again: it hears the name on its next
run, which a `refreshInterval` setting of 2 brought within two seconds. Two sessions given the
same name at once both kept it, though the binary carries a rule giving a session another name
when a live one holds it; it did not apply to `/rename`.

## 6. A background agent leaves the session idle, not waiting

A session that starts a background agent with the Agent tool and ends its turn goes idle at its
prompt box while the agent runs, and takes a ring there as any idle session does; the agent's
result arrives later as a turn of its own (`background-agent.txt`). It never drew "Waiting for 1
background agent to finish", the line `claude.toml`'s `background_agents_working` rule reads,
which came with herdr's manifests; that rule is left as it is, with no prompt, since no screen of
it could be recorded to read one from.

## What this decides for Muster

An urgent ring is typed into a working Claude Code's prompt box, under the doorbell's usual
guards, and counts as taken once the box is empty again (section 1). Its line is harmless at a
dialog that opens before it arrives, and its Return is not (section 3), so the Return is pressed
only once a second look finds the box holding the ring alone. Detection reads plan mode's
dialog as blocked by its own rule, matching its question across a wrap (section 4); before, a
narrow pane without the hooks read it idle. It is the transport, not a
promise that the agent stops: the wake line asks the model to read now, and Claude Code's
reminder asks it to address the line, but a small model may finish first. The `./dev
--claude-code` check holds the transport to this version and prints which the model did.

A pane's name reaches the session as `/rename <name>`, typed at an idle empty prompt as a ring
is, which `claude.toml`'s `[session]` table says (section 5). The session's name reaches the pane
through the statusline in `extras/claude-code`, which reports `session_name` on every run; a
`refreshInterval` is what makes a rename typed in Claude Code reach the pane in seconds rather than
at its next message.
