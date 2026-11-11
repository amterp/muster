# Codex 0.167.0

What changed since 0.166.0 that Muster acts on, or was first recorded here: the lists Codex opens
from its composer, the footer that tells a question waiting on an answer from the composer, what
`/compact` does typed at the composer, and where `codex resume` takes its flags.

Measured 2026-11-10 on macOS 26.4.1 / arm64, with codex-cli 0.167.0 from the Homebrew cask, in a
100x30 pseudo-terminal, by `tools/detection-capture.py --harness codex-pickers`: a scratch
`CODEX_HOME` with a model provider nothing listens at, a fresh git repository trusted in its
config, and `--no-daemon`, so nothing reached a model or an account. The transcript is
`corpus/codex-0.167.0/pickers.txt`. The screens are the "codex 0.167.0" cases in
`corpus/conformance/agent-detection-recorded.json`.

## 1. `/new` asks where the new conversation runs, and the composer's caret marks the answer

In a git repository, `/new` with its Return opened a question before any turn had run:

      Where should the new conversation run?

    › 1. Use current Git worktree  Keep using the current working directory
      2. Create new Git worktree   Create a separate checkout of this repository in another directory

      enter select · esc back

Esc left it for the composer. The chosen option starts with the composer's caret, `› `, which
Muster's composer rule read as a prompt holding the draft "1. Use current Git worktree", so the
screen read idle. A pane in Muster 0.24.0 on a devenv read `done` at this question and would not
take a posted message, as reported in the tooling feedback; this recording ran no hooks.

## 2. Pickers a person opens draw the same footer

`/model` ("Select Model and Effort", eight models and a `↓`) and `/permissions` ("Update Model
Permissions", four options) end on the same `enter select · esc back`, with the same caret. The
startup dialogs 0.160.0 recorded end on footers of the same shape: `enter continue · esc quit`,
`enter confirm · esc skip`, `enter continue · esc skip` (`docs/observations/codex-0.160.0.md`,
section 3). The binary builds each footer from key hints at runtime, so `strings` does not list
the dialogs that draw one.

## What sections 1 and 2 decide for Muster

A footer of Return's hint and Esc's hint at the bottom of the screen reads blocked
(`selection_dialog` in `crates/muster-detect/manifests/codex.toml`), which covers `/new`'s question
and any list Codex asks with that this recording did not bring up. `/model` and `/permissions` are
read by their titles as screens to wait out, as Claude Code's `/model` menu is: someone at the
keyboard opened them, and the state stays what it was.

## 3. `/compact` compacts at once, and takes no focus

Recorded 2026-11-11 with `gpt-6.1-terra` at low effort, logged in, in the same kind of
pseudo-terminal, with no Muster in between (`corpus/codex-0.167.0/compact-and-resume.txt`).
`/compact` and its Return, written in one write at an empty composer after one turn, started a
compaction at once: `Compacting context` with `esc to interrupt`, the title's spinner, and nine
seconds later `Context compacted`. No completion list kept the Return.

`/compact keep the word pong` written the same way was not a compaction. Codex sent it to the
model as a message, which answered it, and the rollout recorded a user message rather than a
`compacted` record. Codex's `/compact` takes nothing to keep.

The rollout records the compaction, then 4 ms later a `token_count` whose `last_token_usage`
counts what the compaction left: 30,291 tokens of 475,000, where the turn before had counted
28,613. No turn runs, so neither `PostToolUse` nor `Stop` fires to read it. `PostCompact` does
fire (section 3 of `docs/observations/codex-0.166.0.md`). Until then, a hook reading the last
count sees the one before the compaction.

## 4. `codex resume` takes its options before the id, and runs with them

`codex resume --help` lists the options `codex` takes, before `[SESSION_ID] [PROMPT]`. The session
above, started with `-m gpt-6.1-terra -c model_reasoning_effort="low" -s read-only -a on-request`,
was resumed as `codex resume -m gpt-6.1-terra -s workspace-write -a never <id>`. It came back as
the same session, with its conversation and compaction drawn. Its `/status` said `Custom
(workspace with network access, never)`, the resume's permissions and not the first run's. Its
effort was the config file's `xhigh`, not the `low` it was started with, because the resume did
not repeat it. A session keeps none of the options it was started with, so a resume has to pass
them again.

## What sections 3 and 4 decide for Muster

Codex's manifest types `/compact`, with no focus, and `muster pane compact --pane <p> <focus>`
refuses a focus for Codex rather than sending it to the model. `extras/codex` answers
`PostCompact` a second time, reading the count that follows the last compaction in the rollout,
so the context Muster shows drops with the compaction rather than at the next turn. The resume
is `codex resume {args} {session}`, which carries the flags the agent was started with.
