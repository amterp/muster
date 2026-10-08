# Codex 0.160.0

What changed since 0.154.0 that Muster acts on: where Codex runs a session's commands and hooks,
what keeps them in the session, the dialogs it shows before a session starts, and what it does with
a plugin marketplace added through a symlink.

Measured 2026-10-07 on macOS 26.4.1 / arm64, with codex-cli 0.160.0 from the Homebrew cask, in
100x30 pseudo-terminals. Every run had its own scratch `CODEX_HOME` with a model provider nothing
listens at, so none asked for a login unless it was meant to, and none reached a model. The
transcripts are `corpus/codex-0.160.0/*.txt`. The screens are the "codex 0.160.0" cases in
`corpus/conformance/agent-detection-recorded.json`, recorded by `tools/detection-capture.py
--harness codex-startup`, `codex-update` and `codex-signin`.

## 1. Every session's commands and hooks run in one shared app-server

With its defaults, the first `codex` on a machine starts `codex app-server --listen unix://
--managed-daemon`, the codex binary copied into `CODEX_HOME`, as its own child in a session and
process group of its own. Every later `codex` with the same `CODEX_HOME` joins it rather than
starting one. The app-server runs the commands and hooks of all of them: a `!` command typed into
the second session and both sessions' hooks were its children, all with the first session's
`MUSTER_PANE`. When the first session exits, the app-server stays, reparented to launchd, with
that session's environment still. It ignores SIGTERM; `codex app-server daemon stop` ends it
(`app-server.txt`).

So whatever runs under the app-server names the first session's pane, whichever session it is
for. The process tree cannot tell them apart either: while the first session runs, the app-server
is its child for every session's work. A `!` command's environment names its own session in
`CODEX_THREAD_ID`; a hook's environment names none.

The issue that reported this saw it "per session" on a Linux devenv, the app-server a child of the
pane's codex. That is this picture with one session running.

## 2. What keeps a session out of it

`[features] daemon_auto_start = false` in `config.toml`: no app-server is started, and each
session's commands and hooks are children of its own codex with its own `MUSTER_PANE`. It does not
reach an app-server already running: a session started after the setting, beside one, still
joined it. `codex --no-daemon` runs one session "without the shared background server, even if it
is already running", and a codex started with any flag on its command line also ran without it,
which Codex records as `incompatible_option` (`app-server.txt`).

Codex has its own command for the setting: `codex features disable daemon_auto_start` writes it
and exits 0, also when it is already off, and `codex features enable daemon_auto_start` writes it
back to `true`. A Codex that does not know a flag exits 1, "Unknown feature flag". What needs the
app-server, such as `codex agents`, refuses to run without it.

## 3. Its dialogs before a session starts mark their choice with the composer's caret

A fresh git repository asks "Trust this folder?" (`1. Trust and continue / 2. Quit`), which
replaced 0.154.0's "Do you trust the contents of this directory?". Hooks it has not seen bring up
"Hooks need review" (`1. Review hooks / 2. Trust all and continue / 3. Continue without trusting`).
A version it knows was released brings up "Update available · 0.160.0 → <version>" (`1. Update
now (runs brew upgrade --cask codex) / 2. Skip / 3. Skip until next version`), with no "Press
enter to continue" as 0.154.0's had, and Esc leaves an "Update available!" banner above the
composer. A home with no login and no other provider shows a sign-in screen, "Sign in with
ChatGPT" to "Provide your own API key" (`startup-dialogs.txt`).

Each of the first three marks its chosen option `› 1. ...`, with the caret Codex draws at its
composer. The binary also holds "Try new model" and "Use existing model", which none of these runs
brought up.

## 4. A plugin is a copy, recorded by the marketplace's resolved path

`codex plugin marketplace add <dir>` records `<dir>` resolved, so `~/.muster/extras`, a symlink to
a version's adapters, registers as that version's directory. `codex plugin add` copies the plugin
into `CODEX_HOME/plugins/cache`; a session does not refresh the copy, and running `plugin add`
again does. Once the symlink points at a newer version, adding it is refused as "already added
from a different source" until `codex plugin marketplace remove muster`. `codex plugin
marketplace list --json` says where each marketplace is registered (`marketplace.txt`).

## What this decides for Muster

- `codex.toml` names the app-server in a `[server]` table, and the daemon refuses every report
  that comes through it, whatever pane it names, saying how to turn it off (section 1). Walking
  past it to the agent above, as for the sandbox helper, would file every session's reports under
  the first session's pane.
- `muster harness install codex` runs `codex features disable daemon_auto_start`, and prints
  `codex app-server daemon stop` for the person rather than running it, since it ends every
  session in the app-server (section 2).
- `codex.toml` reads each startup dialog by the options it offers, as blocked, ahead of the
  composer rule that read its caret as a draft (section 3).
- `muster harness install codex` removes a `muster` marketplace an earlier Muster install left
  before adding this one, and says to run it again after an update, which is what refreshes
  Codex's copy of the plugin (section 4).
