# OpenCode 1.18.34

What OpenCode does that Muster acts on: which model it answers on with no login, what its screen
shows at its prompt and at work, what it does with a pasted line and its Return, and what its
plugins are told. MIP-5 cites it for OpenCode's adapter.

Measured 2026-10-04 on macOS 26.4.1 / arm64, with opencode 1.18.34 installed by npm into a scratch
folder, running `opencode/big-pickle`, in a 100x30 pseudo-terminal driven by
`tools/detection-capture.py --harness opencode` and scratch scripts built on it. Every run unset
the environment's credentials and pointed OpenCode's data, config, state and cache folders at
scratch, so no stored login or key could be used. The transcripts are
`corpus/opencode-1.18.34/*.txt`; the screens are in
`corpus/conformance/agent-detection-recorded.json` and `agent-prompt.json`.

## 1. Its free models answer only a current build

OpenCode's own provider, `opencode`, lists free models and answers them with no key. With no
credential in the environment, `-m opencode/big-pickle` chose that model in the TUI and in
`opencode run`, and 1.18.34 answered. The installed 1.3.15 lists the same provider, but the service
refuses it with a 403, "OpenCode's free tier can only be used from within OpenCode", so 1.3.15
cannot be recorded at work without a paid login (`first-run.txt`). OpenCode's recordings are
therefore of 1.18.34.

## 2. Its prompt is a box with a bar down its left side

Every line of the prompt box starts with `┃`; the line above the box's foot, `╹▀▀▀`, names the
agent and model. The requests above it are drawn with the same bar and no foot, and the permission
prompt takes the box's place with no foot either. An empty box holds a suggestion, "Ask
anything…", drawn grey by color rather than faint, so a reader that blanks faint cells reads it as
typed text. After a turn the box is empty and has no suggestion.

The command palette (ctrl+p) is drawn over the box, which still shows beside it, and the session
list takes the screen; both are titled with `esc` at the right and a `Search` line below, and a
line typed then lands in the search. A quarter of a second after a request is sent the progress
bar is already drawn.

At work the box stays, and OpenCode draws a progress bar of `■` and `⬝` with "esc interrupt"
below it. Herdr's rule for that bar reads every recorded working screen as working, and its
permission rule reads the permission prompt as blocked.

## 3. A paste and its Return in one write is sent

The doorbell's write, a bracketed paste and its Return together, started a turn at once. 1.3.15
kept the same write unsent until a second Return.

## 4. A line typed while it works waits for the turn's end

A line typed and sent with Return while OpenCode works is drawn above the box marked `QUEUED`, and
answered as a turn of its own once the running one ends. Esc twice interrupts a turn.

## 5. Its plugins hear every change of state

A plugin is a JavaScript module whose function returns hooks, loaded from `plugin/` in OpenCode's
config folder or a project's `.opencode/`. Its `event` hook is called for every event OpenCode
publishes (`plugin-events.txt`):

- `session.status` with `busy` as a turn starts and `idle` as it ends, then `session.idle`. A turn
  ended by Esc and a turn ended by refusing a permission end the same way, the first after a
  `session.error` of `MessageAbortedError`.
- `permission.asked` as the permission prompt opens, and `permission.replied` when it is answered.
- `message.updated` for the assistant's message carries its token counts.
- `session.updated` carries the session's title, which OpenCode sets itself from the first request.
- Every event about a session carries its id, `sessionID`.

- `permission.replied` names the prompt it answers by `requestID`, the `id` `permission.asked`
  gave it.
- A sub-agent started with the task tool runs in a session of its own: `session.created` carries
  its parent's id in `info.parentID`, and its own `session.status` and `session.idle` arrive
  inside the main session's turn.
- The plugin's `client.config.providers()` lists every model with its context window,
  `limit.context`; an assistant message's `tokens.total` is its input, output, reasoning and
  cached tokens together, and `cost` is in dollars, 0 for a free model.

The plugin's context includes a shell, `$`, to run commands with. The `permission.ask` hook was
not called. Whether a plugin can put text before the model mid-turn, as Codex's hooks do, was not
measured.

## What this decides for Muster

`opencode.toml` reads the prompt box with detection engine 9: the `bar_prompt` region, the bar
cut from each line as the prompt's margin, and the suggestion named as a placeholder that reads
empty. So the doorbell rings OpenCode at an empty box. It does not ring OpenCode at work, since a
line typed then waits for the turn to end anyway. The plugin events are enough for a plugin to
report working, blocked and idle, an interrupt, the context used and the session's id, and
`extras/opencode` does, leaving a sub-agent's session out.
