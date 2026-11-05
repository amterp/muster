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
Muster carries, and turns off Codex's shared app-server (below). By hand, from this checkout:

```sh
codex plugin marketplace add /path/to/muster/extras
codex plugin add muster-codex@muster
codex features disable daemon_auto_start
```

Codex runs a hook only once it is trusted: a session asks about new hooks before it starts,
"Hooks need review", or review them in `/hooks`. Or copy `hooks/hooks.json` to a trusted
project's `.codex/hooks.json`, which is the same format.

Codex installs a copy of the plugin, and a session never refreshes it, so run the install again
after updating Muster. Codex records the marketplace by its resolved path, which on an SSH machine
is the version `~/.muster/extras` pointed at, so the install first removes a `muster` marketplace
an earlier Muster install left; one added from anywhere else is left for you to remove.

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

`SessionStart` also reports the session's id, which lets Muster resume it after a restart. It
needs nothing installed, since `muster-daemon report` reads the id out of the hook's input itself.
Wakes are typed into Codex's visible composer. A draft holds the wake until it is sent or cleared;
an urgent post at work still joins the running turn (`muster docs msg`).

Muster does not use `codex queue`: `/new` keeps the old conversation loaded and leaves its id
reported until the first new prompt, so a queued wake can start a turn in that old conversation.
Codex can also retain a queued notification after its message is read and replay it on resume.
An older Muster may already have stored such notifications. A turn they start in an earlier
conversation can still consume the pane's messages through these hooks, and end a sender's wait,
without the visible conversation seeing them. Start a fresh Codex process to isolate a new
assignment. `docs/observations/codex-0.166.0.md` records the transitions and limits.

In a Muster pane, `SessionStart` also adds one line to the session's context: when Codex ends a
turn to wait on work it started, it first runs `"$MUSTER_DAEMON" report --waiting "<what>"`, and
for a job it started in the background adds `--until-exits <pid>`. Codex has nothing that starts a
turn when such a job ends, so Muster ends the wait when the process exits and wakes Codex with a
message from `@muster`; a wait declared without it ends after 10 minutes.
The line also says that waiting for a message or a person is just ending the turn, since an agent
that reads `waiting` is not idle and a `muster pane wait --until idle` on it would not return; a
message wakes it either way. Muster then holds off calling the pane done until a later turn ends without the agent declaring
it again, or until you prompt it.

## Context and model

Codex has no statusline command, but every hook is handed its transcript and its model. After each
tool call and when a turn ends, a hook reads the last token count from the transcript's final 64 KB
and reports how full the context is, counted as Codex counts its own "N% context left": the first
12,000 tokens are not counted as used. It reports in the background, so a slow daemon never holds
Codex up. When a turn ends, the hook also reports the reasoning effort the model ran at, from the
transcript's last `turn_context`: Codex hands its hooks the model but not the effort, and writes the
effort only as a turn starts, so the hook searches the whole transcript, once a turn rather than
after every tool call. It needs `jq`, which macOS ships in `/usr/bin` and a Linux devenv may not,
since it works the percent out of the transcript rather than reading one field; so do the messaging
hooks below, which hand the model JSON they build.

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

## Codex's shared app-server

With its defaults, Codex 0.160 runs the commands and hooks of every session on the machine in one
`codex app-server` process. The first session starts it, every later one joins it, and it outlives
the first session, so everything it runs carries the first session's `$MUSTER_PANE`. A hook's
report, or the model's `report --waiting`, would name the wrong pane, so Muster refuses every
report that comes through it, and says why. It refuses `muster msg` through it too, so the
messaging hooks below deliver nothing to a session in it.

`codex features disable daemon_auto_start`, which the install runs, makes each session run its own
commands and hooks again; `codex features enable daemon_auto_start` turns it back on. What needs
the app-server, such as `codex agents`, does not run without it. An app-server already running
still takes new sessions: `codex app-server daemon stop` ends it, and every session running in it,
which is why the install leaves it to you. `codex --no-daemon` keeps a single session out of it.
`docs/observations/codex-0.160.0.md` has what was measured.

## The sandbox

Codex's `workspace-write` and `read-only` sandboxes refuse a command connecting to a Unix socket,
the daemon's included. The hooks are not affected, but what the model runs is: its own `report
--waiting`, `muster msg read` and `muster msg post`, and `muster window` and the `pane` verbs, all
fail with "Operation not permitted", which `muster` reports as the sandbox refusing the socket.
With the messaging hooks a sandboxed Codex is still handed what it is sent, since the hooks run
outside the sandbox, but it cannot answer. To let the model's own commands through, allow the
sandbox the network in `~/.codex/config.toml`:

```toml
[sandbox_workspace_write]
network_access = true
```

That lets every command the model runs reach the network, not only the daemon.

## A `codex exec` started from Codex's shell

A `codex exec` that Codex starts from its shell inherits `$MUSTER_PANE`, and with the plugin
installed its own hooks report into that pane. Muster refuses a report from an agent outside the
process group the pane's own agent runs in, and Codex's shell leaves that group: measured on
2026-10-05 with Codex 0.159.0 on macOS, where the shell is a group of its own, and 0.160.0 on
Linux, where each command runs under Codex's sandbox helper in a group of its own. So a nested
`codex exec`'s reports are refused. The helper is the codex binary under another name, and the
manifest names it, so the model's own `report --waiting` through it still counts as the pane's.
Start a nested one as `env -u MUSTER_DAEMON codex exec ...` and its hooks do nothing at all.

## What was checked

Codex 0.154.0: the hooks in a session's `.codex/hooks.json`, a turn, an approval, Esc mid-turn and a
message queued at work, with what each hook fired; what a hook's output does; and how Codex counts
its context (`docs/observations/codex-0.154.0.md`). Codex 0.160.0: where its app-server runs a
session's commands and hooks, its dialogs before a session starts, and what it records for a
marketplace added through a symlink (`docs/observations/codex-0.160.0.md`). `./dev --codex` checks,
against whatever Codex is installed, that a pane with these hooks reads working and then idle from
them and reports its context, that a draft holds delivery, that a post after `/new` reaches the new
conversation without replay on resume, that an urgent post reaches Codex at work, and that the
messaging hooks hand a sandboxed Codex what it was sent.
