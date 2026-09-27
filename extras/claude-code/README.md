# Claude Code, telling Muster about itself

Two pieces of Claude Code configuration that report what a session is doing to the Muster daemon
that owns its pane: how full its context window is, which model it runs, what it has cost, and
how many sub-agents it has running. The daemon keeps these on the pane's record, beside the
agent state it detects from the screen, so they outlast the app and arrive from a devenv the same
as from this machine.

Both call `"$MUSTER_DAEMON" report`, which every Muster pane can reach: `$MUSTER_DAEMON` is the
daemon's own executable, and `$MUSTER_DAEMON_SOCKET` says where it listens. Outside a Muster pane
neither is set, and both pieces do nothing but what they did before.

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

## The hooks: sub-agents

A sub-agent's start and stop each fire a hook, so the daemon counts: `SubagentStart` adds one and
`SubagentStop` takes one away. `hooks.json` holds both, and a `SessionStart` hook that forgets
the last session's facts when a new one starts or `/clear` runs. Merge its `hooks` into
`~/.claude/settings.json`, or a project's `.claude/settings.json`, beside any hooks already there.

## What was checked

Claude Code 2.1.283, in a pane of a daemon, with both pieces installed: the model and cost arrived
with the first statusline, the context used with the first message, and the sub-agent count rose
to one when a sub-agent started and fell back to none when it stopped.
