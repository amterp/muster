# Codex 0.167.2

Measured 2026-11-13 on macOS arm64 with Codex 0.167.2, using isolated homes and a local Responses
stub. No model or user account was contacted by these probes. Live-tier runs used the real login
and `gpt-6.1-terra` at low effort, without changing the user's configuration or trust state.

## 1. Plugin overrides need an inline table

`corpus/codex-0.167.2/plugin-overrides.txt` records an installed local `muster-codex` plugin whose
SessionStart hook writes a marker. The user configuration enables it. A project hook also records
its events, and the local provider finishes one short turn.

With no override, the plugin writes its marker. The dotted override
`-c 'plugins."muster-codex@muster".enabled=false'` still lets the plugin run on this version.
The inline-table override `-c 'plugins={"muster-codex@muster"={enabled=false}}'` suppresses the
plugin while the project's hooks continue to run. Neither command changes the stored setting.

The live tier therefore disables the installed Muster plugin with the inline table for each
pane whose project hooks it selects. Its hookless pane reads state only from the screen, while
its hooked pane runs this checkout's project hooks. It keeps the user's HOME for the login.

The baseline live tier passed eight checks and failed the hookless comparison because the
installed plugin reported that pane's working and idle states. The idle-message round trip that
failed on 0.166.0 passed on 0.167.2 before any code change here.

## 2. Stop hooks finish before Codex accepts `/clear`

`corpus/codex-0.167.2/stop-hooks.txt` records a project Stop hook held after the local provider
finishes its reply. Codex immediately retains its working title spinner; its `Running hook`
line appears afterward. A bracketed paste of `/clear`, followed by a separate Return, prints
`'/clear' is disabled while a task is in progress.` Once the Stop hook returns, the title loses
its spinner and another `/clear` clears the conversation. The refusal remains in history until
that clear, even though Codex is already ready for commands.

Muster's adapter reports idle at the beginning of Stop. That report previously outranked the
working screen, so a pane could read done while a later Stop hook still prevented commands.
The Codex manifest now opts into `defer_idle_while_working`: the detector keeps the report but
sets it aside while an explicit working signal remains. Once the signal ends, the same report
counts. The recorded screens are also cases in `agent-detection-recorded.json`.

This is an opt-in because another harness can retain a working title for background agents
while its main session is idle. The change does not alter report precedence for those harnesses.

## 3. `/new` still reports no lifecycle event before its first prompt

`corpus/codex-0.167.2/new-conversation.txt` and `new-conversation-hooks.jsonl` repeat the 0.166.0
probe in a scratch Git repository. Project hooks record SessionStart, SessionEnd,
UserPromptSubmit, Stop, and Interrupt. The first local-provider turn reports one session ID.
`/new` opens the worktree question, and choosing the current worktree clears the composer. No
hook runs between the old turn and that empty new composer. The title stays the same. The first
new prompt reports a different ID through SessionStart with `source: startup`.

A hook-only reader therefore still has the previous ID before that prompt. Restarting by the
last reported ID cannot identify the empty new composer. The empty screen alone does not
establish a lifecycle event: clearing a terminal can remove history without changing sessions.
No resume behavior is changed from this recording; a fix needs a reliable command or lifecycle
signal before the new prompt, beyond the hook signals measured here.
