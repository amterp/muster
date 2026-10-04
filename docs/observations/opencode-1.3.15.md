# OpenCode 1.3.15

What OpenCode does at its prompt that the doorbell would act on, as far as it was measured: the
first harness after Claude Code and Codex considered for a prompt reader (MIP-5, section 9).

Measured 2026-10-04 on macOS 26.4.1 / arm64, with opencode 1.3.15 from its own installer, in a
100x30 pseudo-terminal driven by scratch scripts built on `tools/detection-capture.py`, with
OpenCode's data and config folders pointed at scratch so that no stored credential could be
used. The transcript is `corpus/opencode-1.3.15/first-run.txt`.

## 1. Its prompt is a box drawn with a left bar

OpenCode draws its prompt as lines starting with `┃`, the text two columns after it, the model
and agent on the box's last line and `╹▀▀▀` under it, with a faint suggestion, "Ask anything...",
in an empty prompt. No existing detection region reads that box alone.

## 2. A paste and a Return in one write is not sent

A line pasted and a Return in the same write stayed in the prompt, unsent; the same Return a
moment later, in a write of its own, sent it. The doorbell rings an idle agent with one write, so
a ring would sit unsent until Return is pressed again for it, five seconds later, as for a
Claude Code that is still starting.

## 3. The model could not be chosen from outside

Neither `-m opencode/minimax-m2.5-free` nor `model` in a scratch `opencode.json` changed the
model the TUI drew and used: with the machine's own data it was the stored OpenAI login, and with
scratch data a Google model with no key. So no turn of a model that answers was recorded.

## What this decides for Muster

Nothing is built for OpenCode yet. A prompt reader needs a working screen recorded beside the idle
ones, to be sure a rule reading the box does not read an agent at work as idle, and a model that
answers is what draws one; until then OpenCode keeps herdr's rules and is not rung. Its plugins'
events, which would supply reported state, were not measured.
