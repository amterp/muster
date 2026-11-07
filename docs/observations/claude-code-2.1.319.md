# Claude Code 2.1.319

Claude Code tells a `/clear` by SessionStart's `source`, and a finished compaction by `PostCompact`,
but a sub-agent's compaction fires the same hooks with the same input as its parent's. Only the
transcript the compaction landed in tells them apart.

Recorded 2026-11-06 on macOS 26.4.1 / arm64, using Haiku 5.5, with no Muster daemon: a script drove
Claude Code in a PTY with hooks and a statusline that logged their JSON input.
`corpus/claude-code-2.1.319/context-hooks-asked.jsonl` is one session by typed commands, and
`context-hooks-automatic.jsonl` is one with `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE=1`, which compacts on
nearly every prompt, then a `--resume`. `compact-boundary.jsonl` holds the boundary records from
their transcripts. Summaries are removed, some fields dropped, and paths replaced with `$PROBE_ROOT`,
`$USER_HOME`, `$PROBE_SCRATCH` and `$PROBE_PROJECT`. `tests/daemon/context_changes.rs` plays the
records through the shipped hooks.

## 1. `/clear` starts a new session

`SessionEnd` ran with `reason: clear`, then `SessionStart` with `source: clear` under a new
`session_id`, 149 ms later. The statusline's `session_name` carried over unchanged. A `--resume`
ran `SessionStart` with `source: resume` under the same id.

## 2. A compaction that finishes runs three hooks, and one that does not runs one

A `/compact` ran `PreCompact`, then `SessionStart` with `source: compact`, then `PostCompact`, all
under the same `session_id`. An automatic one, mid-turn, ran the same three with `trigger: auto`.
This answers what `claude-code-2.1.310.md` left open, for a compaction at the start of a turn.

`/compact` on the empty conversation after a `/clear`, and a `/compact` cancelled with Esc, each ran
`PreCompact` and nothing after it. So `PreCompact` does not say a compaction happened.

Compaction's internal helper also fired `SubagentStop` with no `SubagentStart`, as
`claude-code-2.1.318.md` recorded, so the sub-agent count is off by then and cannot judge section 3.

## 3. A sub-agent's compaction reads as its parent's

With a sub-agent running, three compactions ran `SessionStart` and `PostCompact`. All three carried
the parent's `session_id` and `transcript_path` and no `agent_id`. The middle one's `compact_summary`
described the sub-agent's work, and its `compact_boundary` record is in the sub-agent's own
transcript, under `<session>/subagents/`, and not in the parent's.

The parent's two boundaries are stamped 40 ms and 25 ms after their `PostCompact` hooks logged, and
every compaction of the session's own left one. Whether Claude Code holds that record back until a
synchronous hook returns was not measured.

## 4. `session_name` does not say who chose the name

A session's name is absent until its first turn. A turn answered "ok" then read `session_name: Ok`,
written by Claude Code, and `/rename probe-name` replaced it. The statusline's input has no other
field that differs between the two. The transcript does tell them apart, as `ai-title` and
`custom-title` records, which nothing here reads.

## What this decides for Muster

`extras/claude-code` reports a clear from SessionStart's `source`, and a compaction from `PostCompact`
only once the session's own transcript records it, in a hook that runs asynchronously. A sub-agent's
compaction then reports nothing, after a five-second wait. A name a pane takes from `session_name`
is marked as taken, and the mark cannot say which of section 4's two wrote it.
