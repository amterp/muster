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

## 4. Reconstructed wheel bursts did not freeze this version

`corpus/codex-0.167.2/wheel-bursts.txt` records a bare 100x60 PTY with no Muster daemon or
adapter. A local Responses provider returns 160 numbered lines to give Codex scrollback. With
that transcript idle, the probe writes 220 identical `ESC[<64;40;50M` wheel-up reports at cell
40,50, then repeats with 2,200 reports. Each write finishes, Ctrl-End returns to the bottom, and
a new prompt reaches the provider and finishes. Another 220-report burst arrives while the
provider holds a turn's response; that turn finishes after release, and a subsequent prompt
also works. Five user turns complete. No input freeze reproduced.

`wheel-input.json` records the exact inputs, their output offsets, and accepted byte counts.
For a wheel entry, decode `hex` and repeat it `repeat` times; the other entries hold their whole
input. The 220-report burst is 2,640 bytes, and the 2,200-report burst is 26,400 bytes. The probe
writes nonblocking and drains PTY output while writing, so terminal output backpressure cannot
block the probe itself before it observes the result.

These bursts reconstruct the shape described in the 0.162.1 report; the original captures on
the devenv were not supplied for this measurement. This is a macOS measurement, while that
report was on Linux. It establishes responsiveness for these inputs on 0.167.2, not that every
wheel-related freeze is fixed. No wheel coalescing or other input behavior changes follow from
this result.

## 5. Named keys answer startup dialogs

`corpus/codex-0.167.2/dialog-keys.txt` records this checkout's CLI driving a fresh Codex through
its daemon, with an isolated home and Git project and an unreachable local provider. No model is
contacted. At folder trust, `pane send --key 1` leaves the dialog open; a following `--key enter`
accepts it. At hooks review, `--key down` moves to `Trust all and continue`, and `--key enter`
accepts it. These are keystrokes, rather than pasted text.

The live tier's existing shared-app-server check now uses the real CLI to accept hooks review:
`--key 2 --key enter` in one pane and `--key down --key enter` in the other. It waits for the
dialog to disappear, then requires the trusted hook to run. This checks the selected action as
well as the screen change, without another model turn or another ignored test.

## 6. Return submits shell commands once shell mode is open

`corpus/codex-0.167.2/shell-return.txt` records `pane send --key shift+1` opening shell mode,
followed by a pasted `printf` command with `--enter`. The command runs and writes its scratch
marker. A second command, pasted after reopening shell mode, writes no marker until a separate
`pane send --key enter`; that key runs it and writes a different marker. The commands contact
no model. A bare-PTY comparison also runs its shell command with kitty-protocol Enter
(`ESC[13u`), before any raw CR is sent.

Pasting a complete line beginning with `!` does not enter shell mode: the pane used for the
preliminary probe queued it as a follow-up prompt. Entering the mode with an actual exclamation
keypress is necessary for this measurement. The Return failure reported on 0.160.0 did not
reproduce on 0.167.2, so no input encoding changes follow from it.

## 7. The model migration dialog needs an answer

`corpus/codex-0.167.2/model-migration.txt` records a fresh isolated home configured for
`gpt-5.5`, with the unmodified catalog from `codex debug models --bundled`. Its upgrade entry
points to `gpt-6-sol` and supplies the migration copy. A fake API key in the scratch `auth.json`
gets past sign-in; no prompt or model request is sent. Default features show `Try new model`
and `Use existing model`, as does explicitly enabling `features.model_migrations`. A fresh
configuration for `gpt-6.1-terra` goes straight to its composer instead.

Both dialog selections use the composer's caret, so the previous detector read them as idle.
The manifest now reads both option labels as a visible blocker. Down changes the selection but
leaves the dialog blocked. Return on `Use existing model` closes it and leaves an idle composer.
All three recorded screens are conformance cases. No new manifest key or engine version is
needed.

## 8. Nested exec has the same hook source, but its report is refused

`corpus/codex-0.167.2/nested-exec.txt` and `nested-exec-hooks.jsonl` record an interactive
session and `codex exec` launched from its shell mode. Both use a scratch home with SessionStart
registered in `config.toml`, and a local provider answers both turns. Both hooks receive
`source: startup`, their own session and transcript IDs, and the same inherited `MUSTER_PANE`.
The permission modes differ in this run, which reflects the invocations rather than identifying
interactive versus exec mode.

The hook calls the shipped SessionStart report flags. Muster accepts the interactive report
and refuses the nested report because its nearest Codex is outside the pane agent's process
group. Window snapshots before and after the child retain the interactive session ID and resume
command. The nested ID never becomes the pane's resume target. No hook-input filter or new
production guard is needed: process attribution already handles this case once Muster has
identified the pane's agent and group. This measurement does not cover a report that arrives
before that identification.
