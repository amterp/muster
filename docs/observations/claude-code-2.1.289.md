# Claude Code 2.1.289

What Claude Code does with `/compact` typed into its prompt box at idle, which is how Muster
compacts an agent (MIP-5, section 10), and with `--resume` after a daemon restart.

Measured 2026-10-04 on macOS 26.4.1 / arm64, with Claude Code 2.1.289 logged in to a Claude Max
account, by hand in a Muster pane. The transcript is `corpus/claude-code-2.1.289/compact.txt`.

## 1. `/compact <focus>` compacts the session, and the prompt box empties at once

`/compact keep the word pong` typed at an empty prompt box, as one bracketed paste and a Return,
started compacting: within three seconds the box was empty and "Compacting conversation…" drawn
above it, and once idle the transcript showed the command and "Compacted", with the context the
statusline reports down from 6% to 0%. No menu kept the Return. What follows the command is what
the summary should keep, so the focus goes after it.

The prompt box stays on screen, empty, while it compacts, so a second look a second after typing
finds either an agent at work or an empty box; both mean the line was taken.

## 2. A session resumed after a daemon restart keeps its id and its conversation

Seen by `./dev --claude-code`'s resume check
(`crates/muster-daemon/tests/daemon/claude_code_live.rs`), with Claude Code started as
`claude --setting-sources project --model haiku --plugin-dir extras/claude-code` and one turn
taken. The SessionStart hook reported the session's id, and the restarted daemon ran
`claude --setting-sources project --model haiku --plugin-dir <path> --resume <id>`: every flag
went along, since each takes a value `resume_values` lists. The resumed session drew the first
turn's prompt above its prompt box, and detection read it as Claude Code. Four seconds after it
started, the id the daemon had saved was still the original one. The session-id hook runs on
every SessionStart, including a resume's, so Claude Code 2.1.289 keeps the id on `--resume`, and
a second restart resumes the same conversation.

## What this decides for Muster

`claude.toml`'s `[session]` table says `compact = "/compact {focus}"`, typed at an idle empty
prompt as a rename is, and taken once the agent is at work or its box is empty again
(`crates/muster-daemon/src/messages/compacts.rs`). What a line typed while Claude Code works does
(`claude-code-2.1.288.md`, section 1) does not matter here: Muster never types a compaction at
work, so it does not depend on Claude Code queueing one.
