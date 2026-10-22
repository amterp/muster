# Codex 0.163.0

What changed since 0.160.0 that Muster acts on: how long a session takes to start a turn for a
message `codex queue` stored.

Measured 2026-10-21 on macOS 26.4.1 / arm64, with codex-cli 0.163.0 from the Homebrew cask, in a
Muster pane, with a scratch `CODEX_HOME` and a local stub as the model provider, so every turn
ended at once and none reached a model. The transcript is `corpus/codex-0.163.0/queue-latency.txt`.

## 1. A session with no shared app-server takes a queued message up to 10 s later

With `[features] daemon_auto_start = false`, each `codex` runs its session in its own process, and
an idle one started a turn for a queued message 1.2-9.7 s after `codex queue` exited, spread
across that range, in 15 trials out of 15. That is a session polling its queue about every ten
seconds. Joined to a shared app-server, the default, the same session started the turn before
`codex queue` had exited, every time.

A session at work holds a queued message until its turn ends and then starts a turn of its own
with it, as 0.154.0 did (`docs/observations/codex-0.154.0.md`, section 9).

## What this decides for Muster

- A wake handed to Codex by `codex queue` is given 20 seconds, two of those polls, to start a turn
  before the doorbell types it instead. With five seconds, the doorbell typed 22 of 45 such wakes
  on this machine between 2026-10-18 and 2026-10-22, every Codex there running without the
  app-server. The typed copy started a turn, Codex then took its stored copy and held it to the
  end of that turn, and the agent was woken a second time, right after answering, for a message it
  had read.
