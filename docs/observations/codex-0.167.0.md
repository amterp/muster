# Codex 0.167.0

What changed since 0.166.0 that Muster acts on: the lists Codex opens from its composer, and the
footer that tells a question waiting on an answer from the composer.

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

## What this decides for Muster

A footer of Return's hint and Esc's hint at the bottom of the screen reads blocked
(`selection_dialog` in `crates/muster-detect/manifests/codex.toml`), which covers `/new`'s question
and any list Codex asks with that this recording did not bring up. `/model` and `/permissions` are
read by their titles as screens to wait out, as Claude Code's `/model` menu is: someone at the
keyboard opened them, and the state stays what it was.
