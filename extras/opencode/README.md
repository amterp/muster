# OpenCode, telling Muster about itself

An OpenCode plugin that reports what a session is doing to the Muster daemon that owns its pane:
whether it is working, waiting on you or idle, its model, how full its context is and what it has
cost, and the session's id. The daemon keeps it on the pane's record, so it outlasts the app and
arrives from a devenv the same as from this machine.

The plugin runs only `"$MUSTER_DAEMON" report`, which every Muster pane can reach:
`$MUSTER_DAEMON` is the daemon's own executable. Outside a Muster pane it is not set, and the
plugin does nothing.

## Installing

Copy or link `plugin/muster.js` into OpenCode's plugin folder, for every project:

```sh
mkdir -p ~/.config/opencode/plugin
ln -s /path/to/muster/extras/opencode/plugin/muster.js ~/.config/opencode/plugin/muster.js
```

or into one project's `.opencode/plugin/`. OpenCode loads every file in that folder as it starts.
The file has no dependencies.

It was measured against OpenCode 1.18.34. OpenCode's free models refuse 1.3.15, which is why
nothing older was checked (`docs/observations/opencode-1.18.34.md`).

## What it reports

Each report says the state its event means, and that state outranks what Muster reads off
OpenCode's screen, so an OpenCode update that changes its screen does not change what Muster
shows:

| Event | State |
|---|---|
| `session.status` busy | working |
| `permission.asked` | waiting on you |
| `permission.replied` | working |
| `session.idle` | idle |

OpenCode ends every turn with `session.idle`: one that finished, one ended with Esc, and one ended
by refusing a permission. So a turn interrupted reads idle at once. A new session forgets the last
one's facts, and reports its id.

A sub-agent runs in a session of its own, which starts and goes idle inside the main session's
turn. The plugin leaves those sessions' turns out, so the pane reads working until the main
session is done, but counts a sub-agent's permission prompt, which holds the whole session up, and
what it spends. Switching to an earlier session reports that session's id and starts its spend
afresh.

After each assistant message it reports the model, the context used as a share of the model's
window, which OpenCode's own provider list gives, and what the session has cost so far. A free
model costs nothing.

## Not here yet

- **Waiting declared.** Claude Code's and Codex's adapters tell the model, as a session starts, how
  to say it ended a turn to wait on its own work. Whether a plugin can add to OpenCode's context
  that way was not measured.
- **Sub-agents counted.** The sessions are seen, and left out, but not counted.
- **Messages fetched by hooks.** OpenCode is rung at an empty prompt instead.

An `opencode run` that an agent starts from its shell inherits `$MUSTER_DAEMON`, and with the
plugin installed it reports into the agent's pane. Muster refuses a report from an agent outside
the process group the pane's own agent runs in, which a shell tool that leaves the group puts it
in; one that stays is not told apart. Start it as `env -u MUSTER_DAEMON opencode run ...` and it
does nothing either way.

## What was checked

`crates/muster-daemon/tests/daemon/harness_hooks.rs` runs the plugin under `node` on the events
OpenCode 1.18.34 was recorded publishing - a turn, a refused permission, a turn ended with Esc,
and a sub-agent's session - and pins every report it makes, in order. OpenCode loads it as it is:
one turn through the real 1.18.34 made the same reports.
