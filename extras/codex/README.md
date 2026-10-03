# Codex, telling Muster about itself

Codex hooks that report what a session is doing to the Muster daemon that owns its pane: whether
it is working, waiting on you or idle, and how many sub-agents it runs. The daemon keeps it on the
pane's record, so it outlasts the app and arrives from a devenv the same as from this machine.

Every hook calls `"$MUSTER_DAEMON" report`, which every Muster pane can reach: `$MUSTER_DAEMON` is
the daemon's own executable, and `$MUSTER_DAEMON_SOCKET` says where it listens. Outside a Muster
pane neither is set, and nothing here does anything. Codex runs hooks outside its sandbox, with
the session's environment, so they reach the daemon whatever sandbox the session has.

## Installing

As a Codex plugin, from this checkout:

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

In a Muster pane, `SessionStart` also adds one line to the session's context: when Codex ends a
turn to wait on work it started, it first runs `"$MUSTER_DAEMON" report --waiting "<what>"`.
Muster then holds off calling the pane done until a later turn ends without the agent declaring
it again, or until you prompt it.

## The sandbox

Codex's `workspace-write` and `read-only` sandboxes refuse a command connecting to a Unix socket,
the daemon's included. The hooks are not affected, but what the model runs is: its own `report
--waiting`, and `muster msg read` when the doorbell rings it, both fail with "Operation not
permitted". To let them through, allow the sandbox the network in `~/.codex/config.toml`:

```toml
[sandbox_workspace_write]
network_access = true
```

That lets every command the model runs reach the network, not only the daemon. Without it a
sandboxed Codex in a pane is still rung, but cannot read what it was rung for on its own.

## Not here yet

Codex has no statusline command, so nothing reports how full its context is; it draws "N% context
left" on its own screen. No hooks fetch `muster msg` messages, so a Codex session is reached by
the doorbell alone. A `codex exec` that Codex starts from its shell inherits `$MUSTER_PANE`, and
with the plugin installed its own hooks report into that pane; start it as `env -u MUSTER_DAEMON
codex exec ...` and they do nothing.

## What was checked

Codex 0.154.0: the hooks in a session's `.codex/hooks.json`, a turn, an approval, Esc mid-turn
and a message queued at work, with what each hook fired
(`docs/observations/codex-0.154.0.md`). `./dev --codex` checks that a pane with these hooks reads
working and then idle from them, against whatever Codex is installed.
