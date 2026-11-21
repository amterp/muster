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
`TERMINAL_CUSTOM_HOOK`. The adapter reports turn state and identity. Context-change receipts
need a way to bind them to the specific command or launch.

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
