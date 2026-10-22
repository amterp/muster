# Harnesses: what Muster gets from each

Muster runs whatever coding agent you already run. It calls the program an agent runs in a
harness - Claude Code, Codex, Gemini - and gets from each what that harness allows. Every pane
running one shows its state, read off its screen. Most of the rest needs the harness's adapter:
hooks or a plugin that Muster ships for Claude Code, Codex and OpenCode, and installs with the
harness's own plugin command:

    muster harness install claude-code
    muster harness install codex --dry-run    # print the commands instead

With an adapter, an agent reports its own state, its context, model and cost, and its session,
which is what `compact_at` acts on and what brings a session back after a restart. `muster
window` says whether each agent's adapter reports, in `adapter`, and notes `(adapter not
reporting)` on the row of an agent whose harness has one that is not reporting (`muster docs
window`).

| Capability | Claude Code | Codex | OpenCode | Every other harness |
|---|---|---|---|---|
| Its state read off the screen | yes | yes | yes | yes |
| Its own report of its state | yes | yes | yes | no |
| Context used, model and cost | yes | yes | yes | no |
| Sub-agents counted | yes | yes | no | no |
| Rung at an empty prompt | yes | yes | yes | no |
| Rung while it works, for an urgent post | yes | yes | no | no |
| Messages fetched by its hooks | yes | yes | no | no |
| Woken by its own command, typing nothing | no | yes | no | no |
| Its session named after the pane | yes | yes | no | no |
| The pane named after its session | yes | no | no | no |
| Compacted when asked | yes | no | no | no |
| Resumed after a restart | yes | yes | no | no |

Every other harness: agy, amp, cline, copilot, cursor, devin, droid, gemini, grok, hermes, kilo,
kimi, kiro, maki, pi, qodercli.

- **Its state read off the screen**: working, blocked or idle, from rules Muster keeps for each
  harness's screens.
- **Its own report of its state**: the harness's hooks tell Muster, which outranks the screen, so
  a harness update that changes its screen does not change what Muster shows. Needs the
  adapter's hooks installed.
- **Context used, model and cost**: shown on the pane's record. Claude Code reports all three
  through the statusline in its adapter. Codex has no statusline command, so its hooks report its
  context, read off its transcript, and its model, but not its cost. OpenCode's plugin reports
  all three after each assistant message.
- **Sub-agents counted**: how many sub-agents the session runs, from its hooks. Codex's are wired
  and have not yet been seen firing.
- **Rung at an empty prompt**: `muster msg post` types a one-line wake into the agent's pane once
  it is idle at an empty prompt (`muster docs msg`). Muster can read the prompt from the screen
  alone, so this needs no adapter.
- **Rung while it works, for an urgent post**: `muster msg post --urgent` types the wake into the
  prompt of an agent at work, which takes it into the turn it is running: Claude Code once the
  tool call it is in returns, Codex as a message held for after its next tool call. OpenCode holds
  a line typed at work until the turn ends, so it is not rung at work. Its Return is
  pressed only once a second look sees the wake alone in the prompt, never at a dialog.
- **Messages fetched by its hooks**: the session is handed what arrived after each tool call, with
  nothing typed into its pane. Needs the adapter's messaging hooks. Claude Code's are also woken
  when a turn ends, by a hook waiting in the background. Codex has no such hook, so between turns
  it is rung, and its hooks hand it what it was rung for as the ring's turn starts - which reaches
  a sandboxed Codex that cannot run `muster` itself.
- **Woken by its own command, typing nothing**: a post to an idle agent runs the harness's own
  command to start a turn in its session, rather than typing a wake into its pane, so it reaches
  an agent holding a draft and never mixes with what a person is typing. Codex's is `codex queue`,
  by the session id its adapter's hooks report as each session starts, and it must be on the
  `PATH` a login shell sets (`.zprofile`, not `.zshrc`). Until then, whenever the command fails,
  and when Codex does not start the turn within 20 seconds, it is rung as usual. An agent at work or at a dialog is not woken this way:
  Codex holds a queued message until its turn ends, and at an approval prompt never sends it.
- **Its session named after the pane**: naming a pane - the chord, the menu, `muster pane rename`,
  or `pane new --name` once its agent starts - types `/rename <name>` into the agent's prompt once
  it is idle at an empty prompt, as a ring is typed, so the session goes by the pane's name in
  `/resume` and wherever else the harness shows it. Needs no adapter. `name_sessions = false` in
  the config file turns it off (`docs/configuration.md`).
- **The pane named after its session**: renaming the session in the harness renames the pane.
  Claude Code hands its statusline the session's name, so this needs the adapter's statusline,
  and its `refreshInterval` for the pane to follow within seconds rather than at the next message.
  Codex names every session itself after its first request, where a rename goes too, so Muster
  cannot tell a name you gave from one Codex chose, and takes neither.
- **Compacted when asked**: `muster pane compact`, or `compact_at` in the config file, types the
  harness's compact command, `/compact <focus>` for Claude Code, at the agent's prompt once it is
  idle and the prompt is empty, as a ring is typed. Needs no adapter, though `compact_at` acts on
  the context its adapter reports. Codex and OpenCode have a `/compact` of their own that Muster
  has not yet been recorded typing, so it does not type it.
- **Resumed after a restart**: a pane whose agent reported its session comes back after a daemon
  restart or a reboot running that session again, with the flags the agent was started with,
  less what would start a session of its own (`docs/configuration.md`, `resume_agents`). Claude
  Code's manifest spells it, `claude --resume <id>`, and its adapter's hooks report the session.
  Codex's spells `codex resume <id>` and carries none of its flags, since where `codex resume`
  takes them has not been recorded, so a Codex comes back without its `--sandbox` or `-c`
  settings and its record's `uncarried` says so. OpenCode spells none yet, so its panes come back
  as shells. A manifest says it in `[session]`: `resume`, the command with `{args}` where the
  flags go and `{session}` for the id, `resume_drops`, the flags never carried, and
  `resume_values`, the flags that take a value, which is the only way a word after a flag is told
  from a first prompt (detection engine 12); from engine 15 a `resume` may leave `{args}` out and
  carry nothing. When a word could be a prompt, only `resume_keeps` goes along, `--model` and
  `--effort` for Claude Code (detection engine 15). A wrapper that starts its agent names its own resume in the agent's
  environment, `MUSTER_RESUME='mywrapper --resume {session}'`, which is run instead, so whatever
  the wrapper sets up is set up again (`docs/configuration.md`, `MUSTER_RESUME`).
- **Its own helpers not taken for a nested agent**: a manifest's `helpers` lists the names a
  harness runs its own binary under for work that is not a session (detection engine 13). Codex's
  lists its Linux sandbox, which every command the model runs goes through, so the model's own
  `report --waiting` from its shell counts as the pane's rather than a nested Codex's
  (`muster docs limits`).
- **Its server refused**: a manifest's `[server]` names the subcommand under which a harness's
  binary serves sessions other than its own, with what a person does about it (detection engine
  14). Codex's names `app-server`, which by default runs every Codex session's commands and hooks
  with the environment of the session that started it, so Muster refuses every report that comes
  through it and says how to turn it off (`muster docs limits`).
- **What hides its prompt, named**: a rule for a screen to wait out, or one the agent is blocked
  at, may say in `shows` what that screen is, and a message held up behind it says so (detection
  engine 15). Claude Code's names its menus, its transcript viewer and the box its feedback tool
  leaves over the prompt once it has drafted a bug report, which takes digits and holds every
  wake until a person answers it.
- **What it waits on, read off its screen**: a rule about idle may say in `waiting` what an agent
  its other rules read idle is waiting on, and the pane reads `waiting` rather than idle, as if
  the agent had said so with `report --waiting` (detection engine 16). Such a rule never decides
  the state itself. Claude Code's reads the end of the line it prints as a turn ends, `· 2 shells
  still running`, since a background shell that ends starts its next turn; a monitor alone does
  not count. A wait read off the screen lasts 10 minutes at most, since that line stays for as
  long as a shell lives, and the agent then reads idle, its turn finished. A wait the agent said
  itself has no limit. Needs no adapter.

The first name a session reports is the one it started with, not a rename: a pane with a name
keeps it and gives it to the session, and a pane without one takes the session's. Neither
direction sets off the other.

Claude Code can also be reached through its own inbox socket, when it has one
(`muster docs msg`). A harness Muster cannot ring, and that has no hooks fetching, is not woken by
a post: the post says its prompt cannot be read, and exits 6 when nobody else heard it.

## Installing an adapter

`muster harness install <harness>` installs the adapter on the machine it runs on: from a pane on
an SSH machine, on that machine. It never edits a harness's configuration files: it runs the
harness's own commands against the adapters this Muster carries, and prints what no command of
the harness's can do. `--dry-run` prints the commands without running them, and the hand steps
as shell comments, so its answer pastes into a shell. Sessions already running take an adapter
up when they restart.

The adapters are in `Muster.app/Contents/Resources/extras/`. An SSH machine Muster installed a
daemon on has them too, linked at `~/.muster/extras`, which keeps its path across an update. From
a source checkout, they are its `extras/`.

**Claude Code**: a plugin for its hooks, then a statusline and optional messaging hooks by hand,
since Claude Code takes a statusline only from its settings.

    claude plugin marketplace add <extras>
    claude plugin install muster@muster

`extras/claude-code/README.md` has the statusline and the messaging hooks.

**Codex**: a plugin for its hooks, optional messaging hooks, and its shared app-server turned off.

    codex plugin marketplace add <extras>
    codex plugin add muster-codex@muster
    codex features disable daemon_auto_start

Codex runs a hook only once you trust it, in `/hooks`. Left on, `daemon_auto_start` runs every Codex
session's commands and hooks in one app-server that carries the first session's pane, and Muster
refuses every report that comes through it (`muster docs limits`); `codex features enable
daemon_auto_start` turns it back on, at the cost of those reports. An app-server already running
still takes new sessions until `codex app-server daemon stop`, which install leaves to you because
it ends the sessions running there. Codex installs a copy of the plugin, recorded against the
resolved path of `<extras>`, so run the install again after updating Muster: it first removes a
`muster` marketplace an earlier Muster install left, and leaves one added from anywhere else, such
as a source checkout. Its sandbox refuses a command connecting to Muster's daemon, so a sandboxed
Codex cannot run `muster msg read` or `post` itself unless its sandbox may use the network; the
messaging hooks hand it what it is sent regardless. `extras/codex/README.md` has both, and the
setting's cost.

**OpenCode**: a plugin for its state, context and cost. OpenCode's `plugin` command installs npm
modules rather than a file, so `muster harness install opencode` runs nothing and prints these:

    mkdir -p ~/.config/opencode/plugin
    ln -sf <extras>/opencode/plugin/muster.js ~/.config/opencode/plugin/

`extras/opencode/README.md` has what it reports, and what it does not yet.

Every other harness has no adapter: Muster reads its state off its screen, and does not ring it.
