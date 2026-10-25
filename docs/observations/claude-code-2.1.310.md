# Claude Code 2.1.310

What Claude Code 2.1.310 does when it compacts an idle session by itself, which told two makers
their idle helpers had gone idle without posting.

Unlike the other files here, this one is not drawn from a recording. On 2026-10-25 the code
bundled in the installed binary (`~/.local/share/claude/versions/2.1.310`, read with `strings`)
was read for the path that prints the notice below. Section 2 lists what is still not recorded,
and how to record it.

## 1. An idle compaction runs `PreCompact` with trigger `auto`

Seen 2026-10-24 on Muster 0.18.1-dev: a Claude Code helper, idle since its last turn, printed
`Compacted while idle, before the prompt cache expired · turn off with "idleCompaction": false in
settings.json` about an hour later. Its context ring went to 0%, and 58 s later its maker was told
it had gone idle without posting since it went to work. A second idle helper did the same that day.

In the bundle, that notice is appended once the session's own compaction returns. The compaction
is started with no origin, which makes its trigger `auto`, and it runs the `PreCompact` hooks with
`trigger: "auto"` before it writes the summary. `PreCompact` matchers take `manual` and `auto`.
Claude Code's own compaction when the context fills mid-turn also runs `PreCompact` with `auto`.

The idle compaction is turned on by a server-side experiment, not by anything in the session. It
fires at about 90% of a one-hour prompt-cache lifetime after the last request, and only for a
session holding at least 200,000 tokens; `CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS` sets that bar, down
to 100,000 at the least. `"idleCompaction": false` in settings turns it off.

## 2. Not recorded

- What the screen shows while it compacts, and whether Muster's screen rules read that as
  working before or after the `PreCompact` report lands. The incident shows they read it as
  working.
- Whether `PostCompact`, `SessionStart` with source `compact`, or `Stop` follow it.

Recording it needs a session over 100,000 tokens, started with
`CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS=100000` and enrolled in the experiment, in a Muster pane with
`extras/claude-code` installed, left idle about 55 minutes. Muster's run log, and `muster pane read`
taken around the notice, would answer both questions.

## What this decides for Muster

`extras/claude-code` answers `PreCompact` with matcher `auto` by reporting `--compacting`. The
daemon takes the turn that compaction shows as for a chore, as it takes a compaction Muster typed,
whether the report comes before the screen shows the turn begin or after (MIP-7, section 4). A
compaction in the middle of a turn somebody typed, or of one begun long before, leaves that turn
as it was.
