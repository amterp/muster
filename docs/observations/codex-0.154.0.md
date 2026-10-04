# Codex 0.154.0

What Codex does that Muster acts on: the hooks it fires and when, what its screen shows at its
composer, what its sandbox lets a command reach, what it asks before a session starts, how its
sessions are named, what it does with a line typed while it works, what reaches its model from a
hook, how it counts its context, and what `codex queue` does.
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

A paste arriving the moment the composer first appears is taken as typed keys: the doorbell's
line, written as one bracketed paste and a Return 0 s after "Ask Codex" was drawn, left the line
in the composer with Codex's file search open on its last word, `@human+worker`, and was never
sent; every Return after it went to the file search. 0.3 s and 1 s later the same write was sent
as a prompt (`hooks.txt`). Typed as keys rather than pasted, the line is never sent, `@` or not.

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

## 5. `/rename` takes the name inline, and Codex names a session itself

`/rename <name>` typed at the composer, pasted with its Return in one write or typed as keys,
names the session before its first turn as well as after one, and the title becomes the name
(`rename.txt`). Each rename appends `{"id", "thread_name"}` to `~/.codex/session_index.jsonl`,
where the last line for an id is its name. A session nobody renames is named by Codex after its
first turn, from what was asked ("Reply with hi"), in the same file and the same form: nothing
there tells a name a person gave from one Codex chose.

## 6. A line typed while it works waits in the composer; its approval prompt ignores one

Codex's composer takes a paste while it works and holds it, and Return queues the line "to be
submitted after next tool call", drawn above a composer that is empty again; once the tool call
returns, the model is handed it (`urgent-ring.txt`). At its approval prompt a pasted line changes
nothing, letters `y` and `p` and all, and declining with Esc drops it; Return there takes the
highlighted option, "Yes, proceed". A line whose last word holds an `@` opens Codex's file search
on that word a moment after it is pasted, and a Return then goes to the search rather than sending
the line; one space after the word keeps the search shut.

## 7. A hook's output reaches the model as `additionalContext`, and a waiting `Stop` holds the session

A `PostToolUse` hook that exits 2 with words on stderr replaces the tool call's result with them,
"Blocked by hook", and the model takes the tool as failed. The same words as
`hookSpecificOutput.additionalContext` on stdout reach the model beside the result, and the same
holds for `UserPromptSubmit`, as for `SessionStart` (`hook-output.txt`). A `Stop` hook runs while
Codex shows "Working ... · Running hook", for as long as it runs, and what is typed meanwhile is
only queued; `{"decision":"block","reason":...}` starts the model again on the reason, with
`stop_hook_active` true on the next `Stop`. Codex has nothing like Claude Code's background
`asyncRewake`. Codex runs a hook's command in the user's shell, which on this machine is zsh.

## 8. Its context is counted past a 12,000-token baseline

Each model response writes a `token_count` event into the transcript, holding the last request's
`total_tokens` and the model's context window. Codex's own "N% context left" leaves the first
12,000 tokens out of both: 19,072 of 258,400 tokens is "97% context left", where a plain share of
the window would say 93% (`context.txt`).

## 9. `codex queue` starts a turn in a running session

`codex queue --thread <name or id> --message <text>` handed an idle session, running in a terminal,
a message it took as a new turn, with nothing typed into the terminal (`queue.txt`).

Measured further on 2026-10-04 (`queue.txt`):

- The session's id, as its hooks are handed it in `session_id`, names it as well as its name does.
- At an idle composer holding a draft, the queued message started a turn, and the draft was in the
  composer again once the turn ended.
- While Codex works, a queued message waits for the running turn to end and starts one of its
  own; it does not join the running turn, as a line typed at work does.
- At an approval prompt it did not answer the prompt, but it was never submitted either, even
  after the turn ended.
- An unknown thread exits 1, "No active session found". A session whose Codex has exited still
  exits 0, so a success says the message was stored, not that a running Codex took it.

## 10. An unreachable model keeps the turn open, retrying

With its model provider pointed at an address nothing listens on, Codex fired `UserPromptSubmit`
and then no hook for over three minutes, showing "Reconnecting... waiting for network" under its
working line with its title spinning (`api-error.txt`). What it fires once it stops retrying was
not reached.

## What this decides for Muster

- Codex's adapter reports state through hooks, as Claude Code's does, from the same daemon verb:
  `UserPromptSubmit` and `PostToolUse` working, `PermissionRequest` blocked, and `Stop` and
  `Interrupt` idle, `Interrupt` because nothing else ends a turn Esc ended (section 1).
- The doorbell reads Codex's composer in a region of its own, the last caret down to the blank
  line above the footer, and nothing else on screen (section 2). A ring, one paste and a Return,
  is sent as a prompt (`hooks.txt`), once Codex has finished drawing: the doorbell rings an idle
  agent only after half a second with nothing drawn (section 2).
- A sandboxed Codex cannot run `muster` without the sandbox's network; its hooks can (section 3).
  `extras/codex/README.md` says so.
- `extras/` carries Codex's marketplace beside Claude Code's, so each harness is offered only its
  own plugin (section 4).
- The live tier and the capture script trust their scratch folder with the inline table and skip
  the update check, so a run leaves nothing in `~/.codex/config.toml` (section 4).
- An urgent ring is typed into Codex's composer at work, and its Return pressed only once a
  second look finds the composer holding it alone (section 6), as for Claude Code; `codex.toml`'s
  working rule says where the composer is. A line typed into a prompt gets a space after it when
  its last word holds an `@`, as a wake's group name can.
- Codex's context and model are reported by its hooks, from the transcript's tail, counted as
  Codex counts them (section 8), and its hooks build their arguments so that zsh and sh agree
  (section 7).
- `extras/codex/messaging-hooks.json` hands what arrived to the model as `additionalContext`,
  after each tool call and as a turn starts, and asks for nothing at a turn's end, which would
  hold the session (section 7). Between turns Codex is rung, and the turn the ring starts begins
  with its messages.
- While Codex retries an unreachable model it reads working, by its title, which is what it is
  doing; no wait it declared earlier stands meanwhile, since `UserPromptSubmit` clears it
  (section 10).
- `codex queue` wakes an idle Codex without typing into its pane, by the session id its
  `SessionStart` hook reports, and leaves its draft alone (section 9). It is not used at an
  approval prompt, where the message would be lost, nor for an urgent post at work, which typing
  delivers into the running turn.
- A pane's name reaches the session as `/rename <name>` at an idle empty composer, which
  `codex.toml`'s `[session]` table says. The session's name does not reach the pane: the one place
  Codex keeps it cannot tell Codex's own name for a session from a person's, and taking Codex's
  would rename every pane after its first request (section 5).
