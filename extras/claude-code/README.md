# Claude Code, telling Muster about itself

Claude Code configuration that reports what a session is doing to the Muster daemon that owns its
pane. The hooks report whether it is working, waiting on you or idle, and how many sub-agents it
runs. The statusline reports how full its context window is, which model it runs, and what it has
cost. The daemon keeps all of it on the pane's record, so it outlasts the app and arrives from a
devenv the same as from this machine.

Everything here calls `"$MUSTER_DAEMON" report`, which every Muster pane can reach:
`$MUSTER_DAEMON` is the daemon's own executable, and `$MUSTER_DAEMON_SOCKET` says where it
listens. Outside a Muster pane neither is set, and nothing here does anything.

## The hooks: working, waiting on you, idle, and sub-agents

Install them as a Claude Code plugin, from this checkout:

```sh
claude plugin marketplace add /path/to/muster/extras
claude plugin install muster@muster
```

Or merge the `hooks` in `hooks/hooks.json` into `~/.claude/settings.json`, or a project's
`.claude/settings.json`, beside any hooks already there; the plugin's file is in the settings
format.

Each hook reports the state its event means, and that state outranks what Muster reads off
Claude Code's screen, so a Claude Code update that changes its screen does not change what Muster
shows:

| Event | State |
|---|---|
| `UserPromptSubmit`, `PostToolUse` | working |
| `PermissionRequest`, and `Notification` for a permission prompt or a question | waiting on you |
| `Stop`, `StopFailure` | idle |

A working state with nothing moving on the screen for ten seconds is let go, since pressing Esc
mid-turn fires no hook, and Muster reads the screen again. `SubagentStart` and `SubagentStop`
count sub-agents, and `SessionStart` forgets the last session's facts when a new one starts or
`/clear` runs.

A plugin cannot set a statusline, so that is a step of its own either way.

## The statusline: context, model and cost

Claude Code runs its statusline command after every message, with the session's state as JSON on
stdin. `statusline.sh` reads the context used, the model and the cost from that JSON, reports
them in the background, and then draws your statusline. Put your own command after it:

```json
{
  "statusLine": {
    "type": "command",
    "command": "/path/to/muster/extras/claude-code/statusline.sh ~/.claude/statusline.sh"
  }
}
```

Your command gets the same JSON on stdin it always did. With nothing after it, `statusline.sh`
draws the model and how full the context is. It needs `jq`, which macOS ships in `/usr/bin` and a
Linux devenv may not.

## What was checked

Claude Code 2.1.283, in a pane of a daemon, with both pieces installed: the model and cost arrived
with the first statusline, the context used with the first message, and the sub-agent count rose
to one when a sub-agent started and fell back to none when it stopped. With the hooks loaded as a
plugin, a turn read working and then idle from the hooks alone. `./dev --claude-code` checks that
last part against whatever Claude Code is installed.
