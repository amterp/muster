---
mip: 5
title: Harnesses - what Muster uses from an agent program, and an adapter for each
status: Draft
kind: Architecture
created: 2026-10-03
decided:
supersedes:
superseded-by:
related: 3, 4
---

# MIP-5: Harnesses - what Muster uses from an agent program, and an adapter for each

## Summary

Muster runs whatever coding agent a person already runs: Claude Code, Codex, Gemini and sixteen
more that its detection knows by name. This MIP calls such a program a harness, calls each thing
Muster can use from a harness a capability, and gives every harness an adapter that supplies the
capabilities it allows. Muster checks capabilities and never harness names, so a harness that
allows less gets less and nothing breaks.

An adapter is data and files, not daemon code: the harness's detection manifest, which the daemon
reads at runtime; `extras/<harness>/`, the hooks and plugin files that run inside the harness and
call Muster's own verbs; its recordings and observations; and a live tier, `./dev --<harness>`.
Rust written for one harness is allowed only where neither data nor a generic verb can say it, and
today that is one thing, Claude Code's inbox socket. Adding a harness means writing its adapter,
not editing the daemon, except to extend detection's engine when the harness draws something no
existing rule can read.

`muster docs harnesses` says which harness has which capability, in a table that is generated
from the manifests and `extras/` wherever Muster can derive a cell, and checked by the gate.

Codex is the second harness, after Claude Code, that Muster reads by more than its screen. Stage
one, built with this MIP: the doorbell, which wakes an idle agent by typing into its prompt
(MIP-4), can read Codex's prompt, using a new detection engine version, 7; Codex's hooks report
its state; its screens are recorded; and `./dev --codex` checks all of it against the installed
Codex. Stage two adds the rest of what Codex allows: its prompt while it works, fetching messages
from its hooks and the context it has used, and a route through `codex queue` that wakes an
idle Codex without typing into its pane.

## Decisions for amterp

These four decisions are the owner's. Each says whether it is one-way, meaning hard to reverse
once things are built on it. All four recommendations are already built; everything else below is
proposed as decided and was built the same way.

**1. Where a harness's adapter lives.**

- (a) In data and files Muster already reads: the detection manifest for what the daemon acts on,
  `extras/<harness>/` for what runs inside the harness, recordings, and a live tier. Rust only
  where neither a manifest rule nor a hook can say it.
- (b) A `Harness` trait in a crate of its own, with a module per harness implementing each
  capability.
- (c) (a), plus a `[capabilities]` table in each manifest declaring what the harness has.

Recommendation: (a). Nearly every capability is either a manifest rule the daemon evaluates or a
hook the harness runs, and neither is Rust; the one exception, Claude Code's inbox, is already a
module of its own. A trait would put each harness into the daemon's source, so a new harness or a
harness update would need a Muster release. A manifest, by contrast, can be overridden in
`~/.muster/agent-detection/` today, unless the update needs a new engine version. (c) declares
what nothing acts on: the daemon already finds out from the rules and the reports themselves
(section 4). Not one-way: (b) could be added beside (a) later, if a capability ever needs code per
harness, without moving the data.

**2. How Muster says what each harness supports.**

- (a) A table in `muster docs harnesses`, a capability per row and a harness per column. The
  cells Muster can derive from the manifests and `extras/` are generated, and a gate test fails
  when they are stale.
- (b) A verb, `muster harnesses`, computing the same live from the daemon's manifests.
- (c) Prose only.

Recommendation: (a). It ships in the binary like the rest of `muster docs`, so it describes the
version running, and the generated cells cannot drift from the files. (b) would also see a user's
override manifests, which is worth adding once a user has written one. Not one-way.

**3. How much of Codex support goes in stage one.**

- (a) Stage one: Codex's prompt for the doorbell, hooks reporting its state, its recordings and
  observation, and `./dev --codex`. Stage two: its prompt while it works (urgent posts), messages
  fetched by its hooks, the context it has used, and a route through `codex queue`.
- (b) All of it at once.

Recommendation: (a), as built. Each stage-two piece needs a measurement first - whether the urgent
ring's two-step write is safe while Codex shows an approval prompt, whether a hook's output
reaches its model mid-turn, how to read context without parsing a growing transcript on every
hook - and stage one stands without them. Not one-way.

**4. One live tier per harness, or one for all.**

- (a) A flag each: `--claude-code`, `--codex`.
- (b) One flag, `--harness <name>`.

Recommendation: (a). Each tier reaches a different vendor with its own login and its own cost,
and a person running one has decided to spend on that vendor. Not one-way.

## Context / Motivation

amterp, 2026-10-03: "add Openai codex support, with extendability properly planned for additional
harnesses, muster must not be only for specific harnesses (tho may support some features only for
some harnesses depending on what they allow)". The kan card this came from (`a_2b6rCBx88`) folds
in two others: "evaluate and ensure we are not tying ourselves too much to claude", and the
doorbell ringing other harnesses, Codex first.

Muster's desiderata already say "Harness-agnostic. Strive to support many harnesses", and its
detection already is harness-agnostic: `muster-detect` carries manifests for nineteen harnesses,
ported from herdr (the session backend Muster used before MIP-3), each a list of screen rules.
Everything beyond the screen was used only by Claude Code, because that is what its authors ran.

### How tied to Claude Code Muster is

Read across the daemon, the CLI, the messaging crate and `extras/`, before this MIP:

| Piece | What it is | Names Claude Code? | Verdict |
|---|---|---|---|
| Detection (`muster-detect`) | Manifests per harness, process names, screen rules | Only as one of nineteen manifests | Generic. Keep. |
| `muster-daemon report` | A pane's state, facts and sub-agents from the agent itself | No: `--agent` takes any manifest's id | Generic. Keep; it is every harness's reporting verb. |
| Report precedence (`detector/reporting.rs`) | When a reported state outranks the screen, and when it lapses | No | Generic, though its timings were measured on Claude Code. Keep. |
| Doorbell (`messages/doorbell.rs`, `prompt.rs`) | Typing a wake into a pane | No: it rings any agent whose manifest reads its prompt | Generic in mechanism, Claude-only in fact: only `claude.toml` had a prompt rule. This MIP adds Codex's. |
| Hooks route (`msg join --pull`, `msg read --if-unread`, `msg wait --due`) | An agent's own hooks fetch its messages | No | Generic verbs; only Claude Code's hooks call them. |
| Inbox route (`messages/inbox.rs`, the CLI's `CLAUDE_CODE_MESSAGING_SOCKET`) | Writing a wake into Claude Code's own socket | Yes | Claude Code's by nature: no other harness has the socket. Keep, as Claude Code's adapter's one piece of Rust. |
| `extras/claude-code` | Plugin hooks, statusline, messaging hooks | Yes, by design | Claude Code's adapter. `extras/codex` beside it. |
| `./dev --claude-code`, `corpus/claude-code-*` | Live checks and recordings | Yes, by design | Claude Code's adapter. A tier and recordings per harness. |
| Docs ("only Claude Code is rung") | What a user is told | Yes | Rewritten in capabilities. |

Nothing in the daemon decides by harness name. The daemon knows an agent only as its manifest's
id, a string from detection, and every Claude-only behavior followed from Claude Code being the
only harness whose adapter supplied the capability. Even the inbox is chosen by whether the CLI
passed a socket, not by name (section 3). So the work is to name the capabilities, say
where an adapter supplies each, and supply them for a second harness, not to take Claude Code out
of the core.

### What Codex allows

Codex 0.154.0 was measured for this MIP (`docs/observations/codex-0.154.0.md`). It has hooks in
Claude Code's format, with nearly the same events: it adds `Interrupt`, which Claude Code lacks,
and has no `Notification`. Its composer takes typing while it works, and holds a message for after the
next tool call. It names sessions and resumes them by id or name, writes each session to a
transcript holding its token counts, and has plugins and marketplaces. It has no statusline
command, and its sandbox refuses a command connecting to the daemon's socket.

## Decision

### 1. Vocabulary

A **harness** is the program an agent runs in: Claude Code, Codex. Muster names it by its
detection manifest's id (`claude`, `codex`), the name that already flows through the daemon,
`muster window --json` and `muster-daemon report --agent`. A **capability** is one thing Muster
can use from a harness. A harness's **adapter** is everything that supplies its capabilities;
the glossary calls it a harness adapter, since its plain "adapter" is the backend's.

Paths below write `<harness>` for the harness's directory name, which is its manifest id or an
alias of it (`claude-code` for `claude`), and `<id>` for the manifest id itself.

### 2. The capabilities

| Capability | What Muster does with it | Without it | Supplied by |
|---|---|---|---|
| Identified | Knows the pane runs this harness | The pane is a shell; nothing below applies | Manifest: id, aliases, script paths; or a `MUSTER_AGENT` variable in the agent's environment, which a wrapper sets to say which harness it starts |
| Screen state | Working, blocked or idle, read off the screen | No state at all | Manifest rules |
| Reported state | The harness's own word on its state, outranking the screen | The screen alone decides, and a harness update that redraws its screen can break the reading | Hooks in `extras/<harness>`, calling `report --agent <id> --state` |
| Interrupt reported | A turn ended at Esc reads idle at once | A working report lapses ten quiet seconds later | A hook for the harness's interrupt event |
| Waiting declared | A turn ended to wait on work reads `waiting`, not idle | Such a pane reads idle, and done once seen | A hook telling the model to run `report --waiting` |
| Context, model, cost | Shows the pane's context used, model and spend | Not shown | A statusline or hook calling `report --context-used` and the rest |
| Sub-agents | How many sub-agents run | Not shown | Hooks calling `report --subagent-started` and `--subagent-stopped` |
| Readable prompt | The doorbell rings the agent, idle at an empty prompt | A post to it says its prompt cannot be read | A manifest rule carrying `prompt` |
| Prompt at work | An urgent post rings the agent while it works | An urgent post waits for idle | A working rule carrying `prompt` (detection engine 6 and later) |
| Messages fetched by hooks | The agent reads its messages between tool calls and is woken at a turn's end | It is rung instead | Hooks calling `msg read --if-unread` and `msg wait --due` |
| Inbox | A wake written to the harness's own socket, for a session the doorbell cannot ring | A session outside a pane is not woken | Rust: Claude Code's wire format |
| Session named after the pane | Naming a pane types the harness's rename at the agent's idle, empty prompt | The session keeps its own name | A manifest's `[session] rename` (detection engine 8) |
| Pane named after the session | A session renamed in its harness renames the pane | The pane keeps the name it has | A statusline or hook calling `report --agent <id> --session-name` |
| Session reference | Wakes the session through the harness's own command | - | Hooks calling `report --agent <id> --session-id` |
| Woken by its own command | A wake to an idle agent runs a command that starts a turn in its session, typing nothing into its pane | It is rung | A manifest's `[session] wake` (detection engine 10), with the session reference |
| Resume, compaction reported | Not yet used | - | To come (section 10) |

The table in `muster docs harnesses` (section 5) says which harness has which.

### 3. An adapter's parts, and where each lives

- **The manifest**, `crates/muster-detect/manifests/<id>.toml`: every capability the daemon acts
  on while a pane runs - identifying the harness, reading its state and its prompt. It can be
  overridden in `~/.muster/agent-detection/<id>.toml` without a rebuild. Each rule reads a named
  region of the screen, and the regions and gates a rule can use are detection's engine, which
  grows when a harness draws something no region reads: engine 7 adds `current_prompt` for
  Codex's composer. A manifest can also say what to type to rename the session, a `[session]`
  table that engine 8 adds, and the command that wakes a session, `wake` in that table, which
  engine 10 adds. So "data, not code" means data in a
  vocabulary the engine extends, with each extension behind an engine version.
- **`extras/<harness>/`**: what runs inside the harness - hooks, a statusline, the plugin and
  marketplace files that install them. Everything there calls only Muster's own verbs,
  `"$MUSTER_DAEMON" report` and `muster msg`, so the daemon learns nothing per harness.
- **Recordings, observations and a live tier**: section 8.
- **Rust for one harness** only where none of these can say it, in a module named for what it
  does, which runs only when a caller presents what it needs, never by comparing a harness's
  name. Claude Code's inbox is the one case: the CLI passes on `CLAUDE_CODE_MESSAGING_SOCKET` when it is set, and the
  daemon writes Claude Code's wire format to it (`messages/inbox.rs`).

### 4. Degrading by capability

Every consumer checks for the capability itself:

- Detection evaluates whatever rules the pane's manifest has. A harness with no recorded screens
  still has herdr's rules.
- The daemon accepts a report only when its `--agent` id matches the agent detection found in
  the pane, and lets it lapse by rules that know nothing of harnesses (`detector/reporting.rs`).
  A harness without hooks sends none, and the screen decides.
- The doorbell rings an agent only if its manifest reads a prompt (`Manifests::reads_prompt`),
  and rings one at work only if a working rule reads one (`reads_prompt_at_work`). Otherwise a
  post says the prompt cannot be read, and the agent is reached by whatever else it allows.
- The messaging service picks a route by what a participant has: a wait or recent hook activity,
  then the pane's doorbell, then an inbox (MIP-4, section 6).

### 5. The table in `muster docs harnesses`

`docs/cli/harnesses.md` holds a table with a capability per row and a column each for Claude
Code, Codex and every other harness detection knows. Where Muster can derive a cell - a readable
prompt and a prompt at work from the manifests; reported state, context, sub-agents and message
fetching from what `extras/<harness>/` calls - the cell is generated by
`crates/muster-detect/tests/detect/harnesses.rs`. That test fails when the page disagrees, and
rewrites the table when run with `MUSTER_WRITE_HARNESSES=1`. What Muster cannot derive, such as
the inbox, and the prose around the table, are kept by hand.

### 6. Claude Code's adapter, as it stands

`claude.toml` reads its states, its prompt idle and at work, and its dialogs, and says how to
rename its session; `extras/claude-code` is a plugin reporting its state and sub-agents, a
statusline reporting context, model, cost and the session's name, and messaging hooks; `messages/inbox.rs` writes to its inbox; `./dev --claude-code` and
`corpus/claude-code-*` check it against the installed version. Nothing moved: it already was an adapter
in this sense, and this MIP names it one.

### 7. Codex's adapter

Stage one, built:

- **Readable prompt.** `codex.toml` has an idle rule reading Codex's composer, in the new
  `current_prompt` region: the last line starting with Codex's caret, `›`, and its wrapped
  continuation lines, down to the blank line above Codex's footer. Both bounds are needed: Codex
  draws earlier requests with the same caret, and draws a footer of model and folder below the
  composer, which a region running to the screen's end would read as typed text. The suggestion
  Codex draws faint in an empty composer reads as untyped, as Claude Code's does. The rule ranks
  below the rule that reads a working screen, which shows the composer too, so such a screen
  still reads working; and it does not mark its idle as certain, so detection holds it a moment,
  as it holds any idle, before publishing it.
- **Reported state, interrupt, waiting declared, sub-agents.** `extras/codex` is a Codex plugin:
  `UserPromptSubmit` and `PostToolUse` report working, `PermissionRequest` blocked, `Stop` and
  `Interrupt` idle, and `SessionStart` clears the last session's facts and tells the model how to
  declare a wait. Esc ends a Codex turn with `Interrupt` and no `Stop`, so `Interrupt` is what keeps
  a turn ended at Esc from leaving a declared wait standing. `SubagentStart` and `SubagentStop`
  count sub-agents, though they were not seen firing (Open Questions).
- **Installing.** `extras/` carries Codex's marketplace file beside Claude Code's: Codex reads
  `.agents/plugins/marketplace.json` first, and without that file it offered Claude Code's plugin
  for install.
- **The sandbox.** Codex's sandbox refuses a command connecting to a Unix socket. Its hooks run
  outside it and report. The model's own `report --waiting` and `muster msg read` need the
  sandbox's network on (`sandbox_workspace_write.network_access`), which `extras/codex/README.md`
  says, with what it costs.

Stage two, built the same day, each piece measured first (`docs/observations/codex-0.154.0.md`,
sections 6 to 9):

- **Prompt at work.** Codex holds a line typed with Return while it works "to be submitted after
  next tool call", and its approval prompt ignores a pasted line and takes Return as "Yes", as
  Claude Code's permission dialog does, so the urgent ring's two-step write (MIP-4, section 6)
  guards it the same way. `codex.toml`'s working rule reads the composer, and `./dev --codex`
  posts urgently to a Codex at work and sees it answered. Measuring it found one thing more:
  Codex opens its file search on a last word holding an `@`, as a wake's group name can, and a
  Return then goes to the search; the doorbell types a space after such a line, for every harness.
- **Messages fetched by hooks.** A hook's output reaches Codex's model as `additionalContext`,
  where exiting 2 replaces a tool call's result, and a `Stop` hook that waits holds the session at
  "Running hook". So `extras/codex/messaging-hooks.json` hands what arrived to the model after each
  tool call and as each turn starts, joins with `--pull`, and waits for nothing: between turns
  Codex is rung, and the ring's turn starts with its messages. Since hooks run outside Codex's
  sandbox, a sandboxed Codex is handed what it is sent, though it cannot answer without the
  sandbox's network.
- **Context used.** Its hooks read the last token count from the transcript's final 64 KB after
  each tool call and at each turn's end, counted as Codex counts its own "N% context left", past a
  12,000-token baseline, and report it with the model, in the background. Codex runs hook commands
  in the user's shell, so they are written to mean the same in zsh as in sh, and the hook tests run
  every harness's hooks in both.
- **A route that types nothing**, built 2026-10-04. `codex queue --thread <id> --message <text>`
  starts a turn in a running session without typing into its pane, and leaves a draft in the
  composer alone. The plugin's `SessionStart` hook reports the session's id with `report --agent
  codex --session-id`, which the daemon keeps on the pane, forgets when the agent leaves it, and
  hands over with it, since Codex says it only as a session starts. `codex.toml` names the command
  as `[session] wake`, with `{session}` and `{message}` each standing for one whole argument, run
  through the user's login shell so that it finds the `codex` a pane finds.

  It is chosen over the doorbell exactly where the doorbell would ring the agent idle: idle or
  waiting, with no wait for an empty prompt or a pause in anyone's typing, since it types
  nothing. Measured first (`docs/observations/codex-0.154.0.md`, section 9): at an approval
  prompt Codex stores a queued message and never submits it, so a blocked Codex waits as it does
  for the doorbell; and at work Codex holds one until the turn ends, where an urgent ring typed
  into the composer joins the running turn, so an urgent post at work is still typed. The command
  runs only while the agent's prompt shows, since a picker or a dialog over it might keep the
  message unsent.

  A command that succeeds says only that Codex stored the message, as it does for a session
  whose Codex has exited - which a `codex exec` run from the pane leaves behind, having reported
  its own session's id through the same hook. So a wake handed over by command is watched like a
  typed ring: if its agent neither goes to work nor reads within five seconds, the wake is typed
  instead and the session's id forgotten. A message Codex kept may then arrive too, a wake too
  many rather than one lost. A command that fails is rung as usual; one that refused - exited
  non-zero, or could not start - is not run for that session again, while one that only took
  longer than five seconds, as a loaded machine can make it, has that wake typed and is tried
  again for the next; a second time, it is not run for that session again either.
  It runs through a login shell, which reads `.zprofile` but not `.zshrc`, so `codex` must be on
  the `PATH` a login shell sets.

### 8. Recordings and tiers

A harness's screens are recorded by `tools/detection-capture.py --harness <id>`, which runs it in
a pseudo-terminal of a given size through named phases. `render_capture` then renders them into
the detection corpus, naming each case for the harness and version. What it does beyond its
screen is measured in throwaway sessions driven by small scripts, condensed into
`corpus/<harness>-<version>/`, and written up in `docs/observations/<harness>-<version>.md`. Its
live tier, `./dev --<harness>`, out of the gate, runs it in panes of a daemon and checks what its
adapter claims. A tier leaves the harness's own configuration as it found it: Codex's tier trusts
its scratch folders only for the run, because answering Codex's trust question writes the folder
into Codex's config.

### 9. Adding a harness

1. Record its screens with `tools/detection-capture.py`, adding its phases there, and pin them in
   the detection corpus. Fix its manifest's rules where they read a screen wrong.
2. If something it draws sits where no existing region reads, extend detection's engine behind a
   new engine version, as engine 7 did for Codex's composer.
3. Give its manifest a prompt rule, if its prompt can be read, with cases in
   `corpus/conformance/agent-prompt.json`.
4. If it has hooks, an `extras/<harness>/` whose hooks call `report --agent <id>`, and a row for
   it in `harness_hooks.rs`, which pins what each event reports and that every turn end reports
   idle.
5. If a session can be renamed by typing at its prompt, a `[session] rename` in its manifest; if
   its hooks or statusline can say the session's name, and only for names a person gave, a
   `--session-name` in its report.
6. An observation file and its transcripts.
7. A live tier.
8. Regenerate the table in `muster docs harnesses`.

Only an engine extension, when one is needed, touches Rust outside the tests.

### 10. Capabilities still to come

Two capabilities are named here so that work on them follows this MIP's structure: resuming a
session after a daemon restart, and compaction reported. Each is a capability an adapter
supplies: a report field its hooks fill, or a manifest table the daemon reads - how to resume -
behind an engine version. The daemon code that acts on the answer will be generic, and a new
protocol field is added as a minor version, per `proto/muster_daemon.proto`'s rules.

A session reference, the third, is built in exactly those two forms for Codex (section 7): a
report field, `--session-id`, and a manifest's `[session] wake`. Claude Code's hooks are handed
its session's id too, and do not report it until something acts on it for Claude Code.

The fourth, a session name kept in step with the pane's, is built, both ways, in exactly those
two forms:

- **Pane to session**: a manifest's `[session] rename`, `"/rename {name}"` for Claude Code and
  Codex, which the doorbell's thread types at the agent's idle, empty prompt under an idle ring's
  rules, and never while it works, since a Return typed at work can answer a dialog. It is taken
  once the prompt is empty again; one that is not is given up and not retyped until the pane is
  renamed. `name_sessions = false` in Muster's config turns this direction off, handed to the
  daemon as a setting since the daemon is what types; the other direction types nothing and stays.
- **Session to pane**: `report --agent <id> --session-name <name>`, empty for no name, which
  Claude Code's statusline sends on every run. Claude Code does not run its statusline on a
  rename, so its `refreshInterval` decides how soon the pane follows. Codex has no statusline, and
  names every session itself after its first request in the one place a rename also goes
  (`docs/observations/codex-0.154.0.md`, section 5), so Codex's pane does not follow it: taking
  Codex's names would rename every pane after its first request.
- **No loop**: a name the pane took from the harness is never typed back, a name the harness says
  again is not news, and a reported name the pane already has ends any typing still to come
  (`crates/muster-daemon/src/session_name.rs`). The first name a session reports is the one it
  started with, so a pane with a name keeps it and gives it to the session, and an unnamed pane
  takes it.
- **Not carried through a handoff**: the daemon taking a pane over hears the session's name again
  from its next report, as a first report; a harness without a statusline has the pane's name
  typed once more, which `/rename` takes as it took it the first time.

### 11. OpenCode's adapter

OpenCode is the third harness, measured at 1.18.34 (`docs/observations/opencode-1.18.34.md`): the
installed 1.3.15 is refused by OpenCode's free models, so only a current build could be recorded
at work without a paid login.

Built:

- **Screen state.** Herdr's rules read every recorded screen right: working by its progress bar,
  blocked at its permission prompt, idle otherwise.
- **Readable prompt.** OpenCode draws its prompt as a box with `┃` down its left side, which no
  region read alone, so detection engine 9 adds one: `bar_prompt`, the run of `┃` lines above the
  box's foot, `╹▀▀▀`, without the line naming the agent and model. Two rule keys come with it:
  `prompt_margin`, cut from the start of every line after the first, since the bar is drawn on
  each; and `prompt_placeholder`, what an empty prompt reads as when the harness draws its
  suggestion in a color rather than faint, as OpenCode does. 1.18.34 sends a paste and its Return
  in one write, so a ring needs no second Return.
- **Reported state, interrupt, context, session reference.** `extras/opencode` is a plugin, a
  JavaScript module OpenCode loads into itself, whose `event` hook turns OpenCode's events into
  `"$MUSTER_DAEMON" report --agent opencode`: `session.status` busy reports working,
  `permission.asked` blocked and `permission.replied` working again, and `session.idle`, which
  ends every turn, Esc and a refused permission included, reports idle. A new session clears the
  last one's facts and reports its id. Each assistant message reports the model, the context used
  against the window OpenCode's own provider list gives, and the session's cost. A sub-agent runs
  in a session of its own whose turns start and end inside the main session's, so the plugin
  leaves sessions with a parent out. It is tested under `node` on the events 1.18.34 was recorded
  publishing, which is why the Linux suite's container carries node; the table in `muster docs
  harnesses` reads a plugin's argument list as it reads a hook's command.

Not built, and why:

- **Prompt at work.** A line typed while OpenCode works waits for the turn to end, so ringing at
  work gains nothing over ringing once idle.
- **Waiting declared, sub-agents counted, messages fetched.** Each needs the plugin to tell the
  model something, or to count the sessions it now leaves out; neither was measured.
- **Session names.** Whether OpenCode can rename a session by typing at its prompt was not
  measured. It names each session itself from its first request, as Codex does, so the pane would
  not take OpenCode's names for the same reason it does not take Codex's.

## Delivery

- **Stage one**, built 2026-10-03: this MIP; detection engine 7 and Codex's prompt rule, with
  recorded screens; `extras/codex`; the hook tests over both harnesses; Codex's observation and
  transcripts; `./dev --codex`; `muster docs harnesses`.
- **Stage two**, built 2026-10-03: Codex's prompt at work, its messaging hooks and its context
  used, each measured first (section 7); `codex queue` measured and left for later.
- **Session names**, built 2026-10-03 (section 10): detection engine 8 and `[session]` in both
  manifests, the daemon typing a pane's name and taking a session's, and Claude Code's statusline
  reporting it.
- **OpenCode**, built 2026-10-04 (section 11): detection engine 9 and OpenCode's prompt rule, with
  recorded screens of 1.18.34; its plugin events measured.
- **Codex woken by `codex queue`**, built 2026-10-04 (section 7): the session reference reported
  and handed over, detection engine 10 and `[session] wake`, the doorbell choosing the command
  for an idle agent.
- **OpenCode's plugin**, built 2026-10-04 (section 11): `extras/opencode`, reporting its state,
  context, cost and session id.
- **Later**: the rest of section 10.

## Rationale

What was missing was a name for each thing a harness can give, a place for each, and a second
harness proving the places hold. Data over code follows from where the capabilities already were:
a harness update that changes a screen should be a manifest edit a user can make today, not a
release, unless it needs a new engine version. Checking capabilities rather than names follows
from the request quoted in Context: some features exist only for harnesses that allow them, and a
harness that allows less must not break anything.

## Alternatives Considered

- **A `Harness` trait and a module per harness.** Rejected (decision 1). It moves into Rust what
  is data today, and needs a release for every harness or harness update. It would earn its place
  only if several capabilities needed per-harness code, and one does.
- **A declared `[capabilities]` table in each manifest.** Rejected (decision 1). The daemon finds
  each capability by using it, so a declaration would be a second statement of the same fact,
  free to disagree with the first. The generated table in `muster docs` says it once, derived.
- **Driving Codex headless through its app server.** `codex app-server` speaks JSON-RPC, with
  turns, steering and thread names as calls, which would be exact where the screen is a reading.
  Rejected for now: it replaces the Codex a person runs in a pane with one Muster runs, and Muster
  runs whatever agents a person already runs.
- **An MCP server for messages.** Codex and Claude Code both take MCP servers, and a `muster`
  server would give the model tools to post and read. Not rejected, not needed for stage one:
  the CLI already does it from the model's shell, though a sandboxed Codex needs the sandbox's
  network for that (section 7), and an MCP server does not wake anyone. Whether it would spare a
  sandboxed Codex that setting depends on where Codex runs MCP servers, which was not measured.
- **One `--harness` tier.** Rejected (decision 4).

## Consequences & Trade-offs

- Codex's manifest needs engine 7, and both Claude Code's and Codex's need engine 8 once they say
  how to rename a session. An older daemon, one a newer app adopted, refuses them and keeps its
  own rules, so it neither rings Codex nor renames sessions until a daemon of this version takes
  over.
- A harness that one agent runs as a command from its shell - `codex exec`, `claude -p` - inherits
  `$MUSTER_PANE`, and its hooks report into the pane it was started from. Both READMEs say how to
  stop that.
- A sandboxed Codex cannot run `muster` without the sandbox's network, which also lets every
  command it runs reach the network.
- Recordings age with each harness version, as Claude Code's do; a tier says when they no longer
  hold.
- Every harness with hooks is another live tier someone has to run, and another vendor's spend.

## Open Questions

- What Codex fires when it gives up on an unreachable model. Measured as far as three minutes
  (`docs/observations/codex-0.154.0.md`, section 10): it retries with its turn open and reads
  working, firing nothing after `UserPromptSubmit`, which has already cleared any declared wait -
  so none stands meanwhile. The end of the retries was not reached.
- Whether Codex's `SubagentStart` and `SubagentStop` fire for the agents it spawns as Claude
  Code's do; the hooks are wired and were not seen firing.
- Whether an OpenCode plugin can hand the model its messages mid-turn, as Codex's hooks do, and
  whether OpenCode renames a session from its prompt (section 11).
- Whether `codex queue` should also wake a Codex at work for a post that is not urgent, rather
  than wait for idle as the doorbell does: Codex holds it until the turn ends, unless the turn
  meets an approval prompt first, which was not measured.
- Whether a person will want `muster` to install an adapter (`muster setup codex`) rather than
  run the harness's own plugin commands.

## References

- MIP-3, section 8: agent detection, ported from herdr.
- MIP-4, section 6: wake adapters, the doorbell, and the prompt it reads.
- `docs/observations/claude-code-2.1.283.md`, `claude-code-2.1.288.md`, `codex-0.154.0.md`.
- `extras/claude-code/README.md`, `extras/codex/README.md`.

---

## History
- 2026-10-03 Draft, from kan `a_2b6rCBx88` (folding `a_2AJS0Xz7I` and `a_2YACckiKU`). Stage one
  built the same day.
- 2026-10-03 Session names kept in step with pane names, both ways (kan `a_2b6Wx8Sox`), as section
  10 described: a manifest table behind engine 8, and a report field.
- 2026-10-03 Codex stage two (kan `a_2bBl75a4X`): prompt at work, messaging hooks and context
  used built; `codex queue` measured and left for later.
- 2026-10-04 OpenCode as the third harness (section 11): its prompt read behind engine 9, its
  plugin events measured, its adapter's plugin left for later.
- 2026-10-04 Codex woken through `codex queue` (kan `a_2bEEFlerm`): the session reference built
  as section 10 described, and when the command beats the doorbell decided from measurements.
- 2026-10-04 `extras/opencode`, a plugin reporting OpenCode's state, context and session id
  (section 11).
