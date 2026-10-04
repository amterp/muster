# Harnesses: what Muster gets from each

Muster runs whatever coding agent you already run. It calls the program an agent runs in a
harness - Claude Code, Codex, Gemini - and gets from each what that harness allows. Every pane
running one shows its state; the rest depends on the harness, and on whether you installed the
small adapter Muster ships for it (`extras/` in Muster's source).

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
  and when Codex does not start the turn within five seconds, it is rung as usual. An agent at work or at a dialog is not woken this way:
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

The first name a session reports is the one it started with, not a rename: a pane with a name
keeps it and gives it to the session, and a pane without one takes the session's. Neither
direction sets off the other.

Claude Code can also be reached through its own inbox socket, when it has one
(`muster docs msg`). A harness Muster cannot ring, and that has no hooks fetching, is not woken by
a post: the post says its prompt cannot be read, and exits 6 when nobody else heard it.

## Installing an adapter

**Claude Code**: a plugin for its hooks, a statusline, and optional messaging hooks.

    claude plugin marketplace add /path/to/muster/extras
    claude plugin install muster@muster

`extras/claude-code/README.md` has the statusline and the messaging hooks.

**Codex**: a plugin for its hooks, and optional messaging hooks.

    codex plugin marketplace add /path/to/muster/extras
    codex plugin add muster-codex@muster

Codex runs a hook only once you trust it, in `/hooks`. Its sandbox refuses a command connecting to
Muster's daemon, so a sandboxed Codex cannot run `muster msg read` or `post` itself unless its
sandbox may use the network; the messaging hooks hand it what it is sent regardless.
`extras/codex/README.md` has both, and the setting's cost.

**OpenCode**: a plugin for its state, context and cost.

    mkdir -p ~/.config/opencode/plugin
    ln -s /path/to/muster/extras/opencode/plugin/muster.js ~/.config/opencode/plugin/

`extras/opencode/README.md` has what it reports, and what it does not yet.

Every other harness has no adapter: Muster reads its state off its screen, and does not ring it.
