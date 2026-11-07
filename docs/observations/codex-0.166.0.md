# Codex 0.166.0

A saved conversation ID does not identify Codex's visible composer after `/new`. A notification
stored by `codex queue` can also survive its message being read and start a turn on resume.

Recorded 2026-11-05 on macOS 26.4.1 / arm64, with codex-cli 0.166.0, no shared app-server, and the
checkout's adapter and messaging hooks. A local Responses stub answered every request, so no
model was contacted. The daemon was built at `5ebc6c78`. Files under `corpus/codex-0.166.0/`
contain selected raw hook, terminal, and daemon records. Scratch paths are replaced with
`$PROBE_ROOT`; session IDs and timestamps are unchanged. The hook environment inherited a
launcher session ID, so hook identity below comes from JSON input, not that environment.

## 1. `/new` leaves the previous conversation loaded

`new-timeline.jsonl`, `hooks.jsonl`, and `daemon-delivery.jsonl` record this sequence:

1. Session `01a1ac0d-324f-7d40-bbc4-cdeff0c05d2e` finished a turn and reported its ID.
2. `/new`, then "Use current Git worktree," opened an empty composer. SessionStart did not run,
   and Muster still named that previous ID.
3. At 14:12:31.086Z the daemon handed message #4's wake to a command. At 14:12:38.451Z the old
   session's UserPromptSubmit hook ran; its messaging hook read #4. The new composer stayed empty.
4. A typed prompt at 14:12:45.216Z started session `01a1ac0d-d7fa-7212-ab22-261fd6c30236`.
   Its SessionStart reported the new ID. A later queued wake for #5 reached that conversation.

Both conversations ran under parent PID 35451. The existing process-group guard cannot tell them
apart. The first empty composer in this recording also runs through the detector's recorded
screen conformance cases.

`active-timeline.jsonl` records a separate attempt while a Responses request was held open.
Codex printed `'/new' is disabled while a task is in progress.` Releasing the provider finished
the same turn. This run did not create another conversation while work was in progress.

## 2. A queued notification can replay after its message was read

`resume-timeline.jsonl` records a clean exit and resume after messages were read. No extra turn
started. A second, controlled sequence held the daemon's real queue command before it stored a
notification:

1. A post created message #7, then a read as the pane consumed it while the command was held.
2. Ctrl+D exited Codex. Releasing the command stored the notification for the exited session;
   the daemon recorded command delivery at 14:17:20.394Z.
3. Resuming that conversation started a turn at 14:17:21.142Z for #7's wake. Its messaging hook
   had nothing to deliver. A final explicit read returned `nothing unread`.

No typed fallback caused this replay. It demonstrates a possible source of stale resume wakes;
it does not establish the sequence behind the earlier Opal and Jade reports. The installed
`codex queue --help` offered no cancellation command.

## What this decides for Muster

The owner chose typed-only Codex wakes on 2026-11-05, reversing the queue route in MIP-5. A typed
wake reaches the visible composer and creates no stored queue copy. A draft now holds delivery
until sent or cleared. Urgent posts to a working Codex still join its current turn.

Muster does not remove notifications already stored by an earlier version. A turn they start in
an old conversation can still consume the pane's messages through its hooks, and end a sender's
wait, without the visible conversation seeing them. Start a fresh Codex process to isolate a
new assignment. Before a new conversation's first prompt, restarting by the last reported ID
also resumes the previous conversation.

## 3. Clearing and compacting, as its hooks say them

Codex says a compaction as it finishes, and a clear only at the next prompt.

Recorded 2026-11-06 with `gpt-6.1-terra` at low effort, with no Muster daemon: a script drove Codex
in a PTY with project hooks that logged their JSON input. `context-hooks-asked.jsonl` is one session
by typed commands, and `context-hooks-automatic.jsonl` is one with
`model_auto_compact_token_limit=3000`, a sub-agent, and a `resume`. `context-hooks-fork.jsonl` is a
`/fork`.

| What happened | Hooks that ran | `session_id` |
|---|---|---|
| `/compact` | `PreCompact`, `PostCompact` at once; `SessionStart` with `source: compact` at the next prompt | same |
| Automatic compaction | `PreCompact`, `PostCompact`, `SessionStart` with `source: compact` | same |
| `/compact`, then Esc | `PreCompact`, `Interrupt` | same |
| `/clear` | nothing, until the next prompt: `SessionStart` with `source: clear` | new |
| `/new` | nothing, until the next prompt: `SessionStart` with `source: startup` | new |
| `/fork` | nothing, until the next prompt: `SessionStart` with `source: fork`, and the conversation carries on | new |
| `codex resume` | `SessionStart` with `source: resume` at the first prompt | same |

`/new` reads like a process starting. Only the session it replaces tells them apart: the process
that ran the old conversation is still running, and a new process follows an agent leaving the pane.
A `/compact` at the empty composer after `/new` still ran `PreCompact` and `PostCompact`, and the
screen said `Context compacted`.

A sub-agent's `PreCompact` and `PostCompact` carry its `agent_id` under its parent's `session_id`.
The parent's carry none.

`extras/codex` therefore reports a compaction from `PostCompact` unless the input holds `agent_id`,
and a clear from SessionStart's `source`, with `startup` counting when the agent held another
session. A clear is dated at the prompt after it, and a `/clear` or `/new` followed by no prompt
reports nothing.
