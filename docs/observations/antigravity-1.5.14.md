# Antigravity CLI 1.5.14

The screen recordings at 100×30 and 60×26 are replayed in
`corpus/conformance/agent-detection-agy.json`. The recorded hook and status inputs in
`corpus/agy-1.5.14/reports.jsonl` run through the adapter in a scratch Muster daemon.

## Composer and dialogs

The active composer starts with `>` between two full-width rules. The transcript can show the
same prompt above it. In auto mode before the first turn the composer shows a mode hint in
ordinary terminal cells. At 60 columns agy shortens the hint with three dots. The recorded slash
menu leaves the composer drawn but takes Return for its selected command. A draft can start
with a blank line and keep its remaining lines after Esc interrupts a turn.

Folder trust and tool approval both take Return as an answer. The approval screen says
`Requesting permission for:` and `Run this command?`. Esc closes it and restores the composer.
While approval is showing, the status line can report `tool_use` with
`tool_confirmation_pending: true`. The pending field takes precedence.

A tracked background `sleep` remained in the footer as `1 task(s) · /tasks` after the turn.
`Stop` sent `fullyIdle: false` and `continuationActive: false`, while the status line sent
`agent_state: "idle"` and `task_count: 1`. The composer was empty and usable above a separate
task panel. When the job ended, agy started another invocation without input and reported its
completion. Muster reads the composer as idle and reports an inferred wait on the background
tasks while that footer is visible.
[Antigravity task manager](https://antigravity.google/docs/subagents?tab=cli) ·
[Antigravity schedules](https://antigravity.google/docs/commands/schedule/)

## Session commands and reports

Pasting `/rename Muster Capture` and pressing Return once renames the session. The status line
subsequently supplies `conversation_title` with the current conversation ID. One Return on
`/compact` starts compaction, with or without a focus message. Esc cancels an active compaction
and restores its composer. `/clear` redraws an empty
composer immediately. The prior ID can remain in status samples until the next prompt, when
`SessionStart` reports the new ID.

Starting `agy --conversation <id>` with the scratch conversation's ID showed its transcript
and an empty composer. An unknown ID printed a warning and opened a fresh composer. The
manifest passes an explicit ID and drops any earlier conversation selector or initial prompt.

The hook JSON supplies the workspace through `cwd`. The hook process itself runs in the
directory containing its configuration. `PreInvocation` supplies the conversation on both a
new turn and a resumed conversation. `Stop` reports idle only when `fullyIdle` is true and
`continuationActive` is false. Compaction and deliberate hook termination can both produce
`TERMINAL_CUSTOM_HOOK`. `lastUserInput` names the requested `/compact`. The Stop payload cannot
establish that the command completed. Muster reports a clear when a new `SessionStart` replaces
a known conversation in the same foreground launch.

Muster takes the absolute transcript path from the hook's `transcriptPath` and binds it to that
report's conversation and directory. The status line's `transcript_path` on 1.5.14 points into
the desktop app's directory.

In a real agy PTY, the status command shared agy's foreground process group. Each hook ran
in its own group, with agy's foreground group still readable from the controlling terminal.
The probe observed this process arrangement on macOS.

## Recording

Run the capture tool in a scratch directory with an existing agy login. The tool accepts a
folder trust dialog to get to its composer. `--only draft` takes no model turns. The other
phases use Gemini 3.8 Flash (Low). Raw captures can contain account details, workspace paths
and text from installed hooks. Review and redact those values before adding a fixture.

```sh
python3 tools/detection-capture.py --harness agy --size 100x30 <scratch>
qm run -- cargo run -q -p muster-detect --example render_capture -- <scratch>
```

To capture resume without sending a prompt, add `--conversation <id>` and reuse the scratch
workspace that made that conversation. Render the first recording before reusing its directory.
