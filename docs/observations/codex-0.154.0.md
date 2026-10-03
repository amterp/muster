# Codex 0.154.0

What Codex does that Muster acts on: the hooks it fires and when, what its screen shows at its
composer, what its sandbox lets a command reach, and what it asks before a session starts.
MIP-5 cites it for Codex's adapter.

Measured 2026-10-03 on macOS 26.4.1 / arm64, with codex-cli 0.154.0 from the Homebrew cask
logged in to a ChatGPT account, running gpt-5.6-luna at low reasoning effort, in a 100x30
pseudo-terminal driven by scratch scripts built on `tools/detection-capture.py`. The transcripts
are `corpus/codex-0.154.0/*.txt`. The screens are in `corpus/conformance/agent-detection-recorded.json`
and `agent-prompt.json`, recorded by `tools/detection-capture.py --harness codex`.

## 1. Its hooks are Claude Code's, with `Interrupt` for Esc

Codex 0.154.0 has hooks on by default, configured the way Claude Code's are: a JSON file of
events, each a list of groups with an optional `matcher` and command hooks (`hooks.txt`). A
project's `.codex/hooks.json` loads once the folder is trusted, a plugin carries its own, and `-c
hooks.<Event>=[...]` passes them on the command line. A hook runs only once it is trusted, which
`/hooks` records, or with `--dangerously-bypass-hook-trust`. A hook's input carries
`session_id`, `transcript_path`, `cwd`, `model` and `permission_mode`, and its environment is the
codex process's.

- A turn fires `UserPromptSubmit` and ends with `Stop`.
- A tool call fires `PreToolUse` and, once it returns, `PostToolUse`.
- An approval prompt fires `PermissionRequest`.
- Esc ends a turn with `Interrupt`, and no `Stop` follows. Declining an approval with Esc does
  the same.
- A message typed and sent with Return while Codex works is held "to be submitted after next tool
  call"; Esc then sends it at once, firing `Interrupt` and then a new `UserPromptSubmit`.
- There is no `Notification`.
- `SessionStart` fires with the session's first prompt, not when Codex opens, and its `matcher`
  matches its `source`, as Claude Code's does.
- What a `SessionStart` hook prints reaches the model, as plain text or as
  `hookSpecificOutput.additionalContext`.

No turn was seen ending any other way. Whether an API error or a dropped stream fires `Stop`,
`Interrupt` or nothing was not measured.

## 2. Its composer is the last caret, above a footer

Codex draws the request that started each turn with the composer's caret, `›`, and the composer
below everything, then a blank line and a footer: the model and the folder. An empty composer
shows a suggestion drawn faint. While Codex works the composer stays, under "• Working (Ns • esc
to interrupt)", and its title starts with a braille spinner; typing there shows "tab to queue
message" and "N% context left" in place of the footer. An approval prompt draws its options with
the same caret, `› 1. Yes, proceed (y)`, and its title says "Action Required", animating as it
waits. In the raw output Codex places each word with a cursor movement, so a phrase is not a run
of bytes.

## 3. Its sandbox refuses a Unix socket

In the `workspace-write` sandbox, a command connecting to a Unix socket outside the workspace is
refused with "Operation not permitted" (`sandbox.txt`). With `sandbox_workspace_write.network_access
= true` it connects. The command's environment says `CODEX_SANDBOX=seatbelt` and carries the
session's id as `CODEX_THREAD_ID` and `CODEX_SESSION_ID`. Hooks are not run in the sandbox: in the
same sandbox, hooks wrote outside the workspace.

## 4. A trust question, an update dialog, and two marketplaces

A folder Codex has not seen gets a question before the session starts, and answering it writes the
folder into `~/.codex/config.toml` (`setup.txt`). Passing the folder's trust as an inline table,
`-c 'projects={"<folder>"={trust_level="trusted"}}'`, skips the question and writes nothing; the
dotted form, `projects."<folder>".trust_level`, does not. With an update available Codex first
opens a dialog that takes the next keys typed, and a digit picks an option; `-c
check_for_update_on_startup=false` skips it.

In a folder holding both `.agents/plugins/marketplace.json` and Claude Code's
`.claude-plugin/marketplace.json`, `codex plugin marketplace add` reads the first; with only
Claude Code's it offers Claude Code's plugin.

## What this decides for Muster

- Codex's adapter reports state through hooks, as Claude Code's does, from the same daemon verb:
  `UserPromptSubmit` and `PostToolUse` working, `PermissionRequest` blocked, and `Stop` and
  `Interrupt` idle, `Interrupt` because nothing else ends a turn Esc ended (section 1).
- The doorbell reads Codex's composer in a region of its own, the last caret down to the blank
  line above the footer, and nothing else on screen (section 2). A ring, one paste and a Return,
  is sent as a prompt (`hooks.txt`).
- A sandboxed Codex cannot run `muster` without the sandbox's network; its hooks can (section 3).
  `extras/codex/README.md` says so.
- `extras/` carries Codex's marketplace beside Claude Code's, so each harness is offered only its
  own plugin (section 4).
- The live tier and the capture script trust their scratch folder with the inline table and skip
  the update check, so a run leaves nothing in `~/.codex/config.toml` (section 4).
