# Codex, telling Muster about itself

Codex hooks that report what a session is doing to the Muster daemon that owns its pane: whether
it is working, waiting on you or idle, how many sub-agents it runs, its model and how full its
context is. The daemon keeps it on the
pane's record, so it outlasts the app and arrives from a devenv the same as from this machine.

Every hook calls `"$MUSTER_DAEMON" report`, which every Muster pane can reach: `$MUSTER_DAEMON` is
the daemon's own executable, and `$MUSTER_DAEMON_SOCKET` says where it listens. Outside a Muster
pane neither is set, and nothing here does anything. Codex runs hooks outside its sandbox, with
the session's environment, so they reach the daemon whatever sandbox the session has.

## Installing

`muster harness install codex` installs it as a Codex plugin, from the adapters the running
Muster carries. By hand, from this checkout:

```sh
codex plugin marketplace add /path/to/muster/extras
codex plugin add muster-codex@muster
```

Codex runs a hook only once it is trusted: review them in `/hooks` in a session. Or copy
`hooks/hooks.json` to a trusted project's `.codex/hooks.json`, which is the same format.

`extras/` holds Claude Code's plugin too. Codex reads `.agents/plugins/marketplace.json` before
`.claude-plugin/marketplace.json`, so it is offered only `muster-codex`, and Claude Code only
`muster`.

## What the hooks report

Each hook reports the state its event means, and that state outranks what Muster reads off
Codex's screen, so a Codex update that changes its screen does not change what Muster shows:

| Event | State |
|---|---|
| `UserPromptSubmit`, `PostToolUse` | working |
| `PermissionRequest` | waiting on you |
| `Stop`, `Interrupt` | idle |

Esc ends a turn with `Interrupt` and no `Stop`, and so does declining a command at its approval
prompt; a message queued while Codex works and sent with Esc fires `Interrupt` and then
`UserPromptSubmit`. How long Muster keeps believing a report, and when it reads the screen
instead, is the same for every harness: `extras/claude-code/README.md` has it.

`SubagentStart` and `SubagentStop` count sub-agents. `SessionStart` forgets the last session's
facts when a new session starts or `/clear` runs. Codex fires it with the session's first prompt,
not when it opens.

`SessionStart` also reports the session's id, which lets Muster wake an idle Codex with `codex
queue` rather than typing into its pane, so a draft in the composer is left alone (`muster docs
msg`). It needs `jq`; without it, or before a session's first prompt, Codex is rung by typing as
any agent is. `codex` must be on the `PATH` a login shell sets, `.zprofile` rather than `.zshrc`,
since Muster runs it through one.

In a Muster pane, `SessionStart` also adds one line to the session's context: when Codex ends a
turn to wait on work it started, it first runs `"$MUSTER_DAEMON" report --waiting "<what>"`.
Muster then holds off calling the pane done until a later turn ends without the agent declaring
it again, or until you prompt it.

## Context and model

Codex has no statusline command, but every hook is handed its transcript and its model. After each
tool call and when a turn ends, a hook reads the last token count from the transcript's final 64 KB
and reports how full the context is, counted as Codex counts its own "N% context left": the first
12,000 tokens are not counted as used. It reports in the background, so a slow daemon never holds
Codex up, and it needs `jq`, which macOS ships in `/usr/bin` and a Linux devenv may not.

## Messages through hooks

`messaging-hooks.json` is for a session that takes part in `muster msg` (`muster docs msg`), and
is not in the plugin, so that only the sessions you choose take part. Merge its events into a
trusted project's `.codex/hooks.json`, beside the plugin's or `hooks/hooks.json`:

- `PostToolUse` hands the model anything that arrived, after each tool call, as context.
- `UserPromptSubmit` does the same when a turn starts, so the turn a ring starts begins with the
  messages it was rung for already in the model's context.
- `SessionStart` runs `muster msg join --pull`, which tells the daemon these hooks fetch the
  session's messages, so nothing is typed into its pane while it works.

Codex has nothing like Claude Code's background `Stop` hook: a `Stop` hook that waits holds the
session at "Running hook", taking typing only as queued messages, for as long as it waits. So
between turns Codex is still rung by the doorbell, and the ring's turn is where these hooks hand
over what it was rung for. A hook's output reaches the model as `additionalContext`; exiting 2, as
Claude Code's hooks do, would replace the tool call's result instead.

## The sandbox

Codex's `workspace-write` and `read-only` sandboxes refuse a command connecting to a Unix socket,
the daemon's included. The hooks are not affected, but what the model runs is: its own `report
--waiting`, and `muster msg read` and `muster msg post`, all fail with "Operation not permitted".
With the messaging hooks a sandboxed Codex is still handed what it is sent, since the hooks run
outside the sandbox, but it cannot answer. To let the model's own commands through, allow the
sandbox the network in `~/.codex/config.toml`:

```toml
[sandbox_workspace_write]
network_access = true
```

That lets every command the model runs reach the network, not only the daemon.

## Not here yet

A `codex exec` that Codex starts from its shell inherits `$MUSTER_PANE`, and with the plugin
installed its own hooks report into that pane. Muster refuses a report from an agent outside the
process group the pane's own agent runs in, but whether Codex's shell leaves that group has not
been measured; start it as `env -u MUSTER_DAEMON codex exec ...` and its hooks do nothing.

## What was checked

Codex 0.154.0: the hooks in a session's `.codex/hooks.json`, a turn, an approval, Esc mid-turn
and a message queued at work, with what each hook fired; what a hook's output does; and how Codex
counts its context (`docs/observations/codex-0.154.0.md`). `./dev --codex` checks, against
whatever Codex is installed, that a pane with these hooks reads working and then idle from them
and reports its context, that an urgent post reaches Codex at work, and that the messaging hooks
hand a sandboxed Codex what it was sent.
