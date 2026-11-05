# Claude Code 2.1.318

Recorded 2026-11-05 on macOS 26.4.1 / arm64, using Haiku 5.5 with high effort, one
background Haiku Agent, and an isolated Muster daemon. The records are pane reads and hook
inputs, rather than raw PTY bytes: `corpus/claude-code-2.1.318/background-input.jsonl`,
`background-hooks.jsonl`, and the daemon's selected `background-delivery.jsonl` events.
Paths are replaced with `$PROBE_ROOT` and `$USER_HOME`. The recorded screens run through the
recorded-detection and prompt corpus tests.

## A parent waiting for its background agent accepts input

The parent launched an Agent, declared its wait with `report --waiting`, and ended its turn.
Its Stop hook ran at 15:24:31.848 UTC. Claude drew `✻ Waiting for 1 background agent to finish`
above its empty prompt box while the child ran a Python command that sleeps for 60 seconds.
The child had started that command at 15:24:30.195 UTC.

An ordinary post at 15:24:33 was held. The pane's effective state was waiting, but the
`background_agents_working` screen rule found no prompt and prevented typing. Its
`msg.ring.waits` event records that rule and the screen's shape.

A line typed directly at 15:24:35.493 UTC started a parent UserPromptSubmit while the original
child remained active. The parent read the held message and replied `TYPED_WHILE_CHILD_RUNNING`.
Its next Stop ran at 15:24:41.780 UTC, and the same Waiting line returned. A directly typed
`/compact` then started, and the screen recorded it completed by 15:24:57 while the original
child was still listed as running. Compaction's internal helper also fired SubagentStop;
that event does not mean the original child finished.

Muster therefore reads this line as idle with a readable prompt, and names the screen's wait
as `its background agents`. The usual busy title, turn spinner, permission dialog, draft and
input quiet-period checks still apply. A screen-derived wait has the existing ten-minute
limit; an explicit declared wait follows its own rules.

## A child hook can change the parent's effective state

The first probe used a child whose Bash tool backgrounded `sleep 60`. After the parent's Stop,
the child's PostToolUse ran at 15:22:58.188 UTC with a nonempty `agent_id`. The pane read working
at 15:22:59 even though the parent had ended its turn and the Waiting line was visible. The
ordinary wake then waited until the child's short turn ended. The selected first-probe screen
is included in `background-input.jsonl`.

The screen-rule correction removes the recorded prompt hold. The separate effect of child
PostToolUse state reports needs its own regression before changing hook authority; these
records do not establish how every child tool sequence affects it.
