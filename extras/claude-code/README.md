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

`muster harness install claude-code` installs them as a Claude Code plugin, from the adapters
the running Muster carries, and prints the statusline step below. By hand, from this checkout:

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
| `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure` | working |
| `PermissionRequest`, and `Notification` for a permission prompt or a question | waiting on you |
| `Stop`, `StopFailure` | idle |

A working state with nothing moving on the screen for ten seconds is let go, since pressing Esc
mid-turn fires no hook, and Muster reads the screen again. It is also set aside while Muster's
screen rules read a permission prompt that was up before it came, or has been for two seconds, since one sub-agent's tool calls go
on reporting working while another's prompt waits on you. Waiting on you or idle is let go once
Muster's screen rules, having read it the same way, read something else for two seconds: Esc or
No at a permission prompt fires no hook either. If the rules never read it that way, it is let go
once the screen has moved for three seconds running: an approved tool fires no hook until it
finishes, and a background task can keep the screen busy after `Stop`. `SubagentStart` and `SubagentStop`
count sub-agents, and `SessionStart` forgets the last session's facts when a new one starts or
`/clear` runs. `SessionStart` also reports the session's id, however the session started, which is
what lets the pane come back running this session after a daemon restart or a reboot
(`docs/configuration.md`, `resume_agents`). The report reads the id from the hook's own input, so
the hook needs no JSON tool.

In a Muster pane, `SessionStart` also adds one line to the session's context: when Claude Code ends
a turn to wait on work it started, it first runs `"$MUSTER_DAEMON" report --waiting "<what>"`.
The line also says that waiting for a message or a person is just ending the turn, since an agent
that reads `waiting` is not idle and a `muster pane wait --until idle` on it would not return; a
message wakes it either way. Muster then holds off calling the pane done until a later turn ends without the agent declaring
it again, or until you prompt it: `UserPromptSubmit` reports an empty wait. An agent that forgets
reads as done, as before. One that waits on something that never wakes it reads as waiting until
it is next prompted. A turn ends at each `Stop`, so a `Stop` hook of your own that makes the agent
keep going (one that runs the tests before it lets it stop, say) ends the wait at the second
`Stop`, and the pane reads done while the agent waits.

A `claude -p` that Claude Code starts from its Bash tool inherits `$MUSTER_PANE`, and with the
plugin installed at user level its hooks report into the outer session's pane. Muster refuses
those reports: the Bash tool runs it in a process group of its own, and Muster takes a report only
from processes in the group the pane's own agent runs in (`muster docs limits`). One it cannot
tell apart, such as a nested session started some other way in the same group, is still taken;
`env -u MUSTER_DAEMON claude -p ...` keeps its hooks quiet either way.

A plugin cannot set a statusline, so that is a step of its own either way.

## The statusline: context, model, cost and the session's name

Claude Code runs its statusline command after every message, with the session's state as JSON on
stdin. `statusline.sh` reads the context used, the model, the cost and the session's name from
that JSON, reports them in the background, and then draws your statusline. Put your own command
after it:

```json
{
  "statusLine": {
    "type": "command",
    "refreshInterval": 2,
    "command": "/path/to/muster/extras/claude-code/statusline.sh ~/.claude/statusline.sh"
  }
}
```

The session's name is what renames the pane when you `/rename` the session. A rename does not run
the statusline, so `refreshInterval` is what brings the new name to the pane within seconds;
without it, the pane takes it at the session's next message. Renaming the pane renames the
session either way (`muster docs harnesses`).

Your command gets the same JSON on stdin it always did. The report needs nothing installed:
`muster-daemon report` reads Claude Code's JSON itself. With nothing after it, `statusline.sh`
draws the model and how full the context is, and that needs `jq`, which macOS ships in `/usr/bin`
and a Linux devenv may not.

## Messages through hooks

`messaging-hooks.json` is for a session that takes part in `muster msg` (`muster docs msg`): its
own hooks fetch its messages, so nothing has to be typed into its pane or held for approval at
its inbox. Pass it to the sessions that should use it, `claude --settings
/path/to/muster/extras/claude-code/messaging-hooks.json`, or merge it into a project's
`.claude/settings.json`. It is not in the plugin, because every session with the plugin would
then take part in messaging.

- `PostToolUse` runs `muster msg read --if-unread` after each tool call, and hands anything
  unread to the model on stderr with exit 2. Claude Code shows that to you as the hook's
  feedback.
- `Stop` runs `muster msg wait --due` in the background (`asyncRewake`) once a turn ends. When a
  message arrives for the session, the wait prints the wake and the hook exits 2, which starts a
  turn with that wake. Its `timeout` of a day is what keeps it waiting: without one, Claude Code
  ends the hook after its default (`docs/observations/claude-code-2.1.283.md`, section 4). `--due` answers only a wake the session is due, so a session
  that ends its turn without reading is woken once more and then not again until it reads.

While a session's hooks run - a wait of its own is connected, or it is working and ran a `muster
msg` command in the last five minutes - the daemon types nothing into its pane. A turn that ends
without its `Stop` hook, in an API error or at Esc, leaves the session idle with no wait, and the
daemon rings it for what it has unread.

## What was checked

Claude Code 2.1.283, in a pane of a daemon, with both pieces installed: the model and cost arrived
with the first statusline, the context used with the first message, and the sub-agent count rose
to one when a sub-agent started and fell back to none when it stopped. With the hooks loaded as a
plugin, a turn read working and then idle from the hooks alone. `./dev --claude-code` checks that
last part against whatever Claude Code is installed, and that a `Stop` hook marked `asyncRewake`
still wakes an idle session and a `PostToolUse` hook's stderr still reaches the model.
