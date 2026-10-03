# Glossary

One name per concept; docs and code use these terms. Alphabetical.

- **adapter** - the module translating the Muster vocabulary to one concrete backend; nothing backend-shaped escapes
  it.
- **agent state** - working / blocked / waiting / idle / done / unknown, per pane. Daemon-detected, except `done`: a
  finish the daemon holds as `finished_unseen`, which a window paints until it is seen; and `waiting`: an idle agent
  whose facts say what it ended its turn to wait on.
- **backend** - the daemon system that owns sessions: `muster-daemon`, Muster's own since it replaced herdr (MIP-3).
- **backend session** - one live connection to one daemon.
- **bridge** - the subprocess a surface runs to deliver a pane channel; output only.
- **capability** - one thing Muster can use from a harness: its state read off its screen, its own report of it,
  a prompt the doorbell can read, hooks that fetch its messages (MIP-5, section 2). Muster checks a capability,
  never a harness's name.
- **command endpoint** - the unix socket a window answers requests on, at
  `~/.muster/state/command-<pid>.sock`. The same schema the shell/core seam carries, arriving from another process -
  which is what the CLI is. A pane reads the path of its own window's from `MUSTER_SOCKET`.
- **composition** - the Muster-owned arrangement: which daemons are attached, which tabs the window holds and in
  what order, which of them is on screen, and how each divides between the machines holding panes in it. Not an
  input method's composition, which is a different thing with the same name and lives under `input::` wherever it
  appears in the code.
- **control plane** - everything except output: events, state, intents, input. Flows through the core.
- **core** - the headless, OS-free view-model: mirror, dispatcher, keymap, attention, config.
- **daemon** - one running backend server instance owning PTYs and sessions, local or remote.
- **data plane** - output only: pane channels, adapter to surface, bypassing the core.
- **devenv container** - the repo's Linux container, running sshd and no daemon until a remote test or an attach
  installs this build's; dev sandbox and remote-path test fixture in one.
- **doorbell** - a wake typed into the pane an agent runs in, one line and a Return, only into a prompt it has
  just read as empty, and only for a harness whose manifest can read its prompt (MIP-4, section 6). The prompt is
  an idle agent's, or for an urgent post a working one's too.
- **focus history** - the panes a window's keyboard has been on, oldest first, with a cursor at the current one:
  `focus_back`, `focus_forward`, `muster focus --back|--forward` and the mouse's back and forward buttons walk it.
  One per window, at most 50 panes, and not kept across a relaunch. A pane that closed or that another window holds
  is stepped over.
- **frame** - one message on a socket: a four-byte length, then that many bytes (`muster-frame`).
- **group** - a set of participants and the one log of messages they share (MIP-4). Addressing a message decides
  whom it wakes, never who may read it. Kept on the daemon it was made on, its home; another machine with a member
  holds a replica, named `review@machine` there (MIP-4, section 11).
- **guard** - the daemon refusing a post while its author has unread messages from others in that group. There is
  no override: read, then post.
- **harness** - the program an agent runs in: Claude Code, Codex, Gemini. Named by its detection manifest's id
  (`claude`, `codex`).
- **harness adapter** - everything that supplies a harness's capabilities: its detection manifest, its hooks and
  plugin files in `extras/<harness>/`, its recordings and observations, and its live tier (MIP-5). Data and files,
  not daemon code; not the backend's **adapter** above.
- **hold** - which window a tab belongs to. Every tab is held by exactly one window, open or closed, and a window
  lists only the tabs it holds; the record is `~/.muster/state/holding/tabs.toml`, shared by every window.
- **intent** - a requested mutation sent to a daemon (split, close, resize, zoom, input, spawn). Muster never
  mutates; it requests.
- **mirror** - the core's disposable cache of daemon structure, bootstrapped from snapshot plus events; never
  authoritative.
- **pane** - one terminal inside a tab's tree; owned by a daemon.
- **pane channel** - the output stream feeding one surface: the program's own bytes, passed through by the daemon.
- **pane name** - what Muster calls a pane: `p1w3r07bsd`, minted by Muster rather than borrowed from the backend,
  unique across every attached machine, and never reused. What every message and every CLI argument means by a pane.
  A pane reads its own from `MUSTER_PANE`.
- **pane tree** - the split layout inside one tab; daemon truth.
- **participant** - an agent, or the human as `@human`, known by name to one daemon's messaging, with a place in
  the log of every group it has joined. Another machine's participant is `name@machine` there.
- **policy** - a group's four rules, enforced by the group's home daemon: whom an unaddressed post wakes (`ring`), whom each
  author may address (`allow`), who may change the group (`membership`), and whether it is `paused`, holding every
  wake but the human's (MIP-4, section 8).
- **region** - the part of a Muster tab that one machine holds, as it sits on screen: that machine's pane tree,
  and how wide it is. One for every tab until somebody groups two.
- **roster** - every tab the window holds with its panes under it, ordered and labelled by the core, each row saying
  whether the window is showing it. Beside them, the machines - because a machine holding no panes contributes no
  tabs and would otherwise vanish. What the view is to the screen, this is to the session.
- **seam** - an injected boundary the core is tested and swapped at. Two exist: backend and renderer.
- **seen-ness** - whether anybody has looked at a pane since its agent finished; distinguishes idle from done. A pane
  is seen when it is on screen in a window holding the OS's focus. No daemon can see a window, so the window decides
  it and reports it to the daemon, which clears the finish for every window.
- **session name** - what a harness calls an agent's session: what Claude Code's and Codex's `/rename` set and
  `/resume` lists. Kept in step with what the pane is called (its label, set by `pane rename` or `--name`), both
  ways, as far as the harness allows (MIP-5, section 10).
- **shell** - the per-OS native layer: windows, chrome, key capture, surfaces. Owns nothing.
- **surface** - one libghostty terminal view rendering one pane channel; disposable.
- **tab** - a named set of panes a window shows together. Muster's own unit, and the one thing here that is not a
  daemon's: a tab holding panes on one machine is one backend tab and comes back after a daemon restart like
  everything else, and a tab somebody has grouped to hold panes on two is one backend tab on each, both carrying the
  same tab name. `docs/architecture.md`, durability, says what survives what.
- **tab name** - what Muster calls a tab: `t1w3r07bsd`, from the same registry as a pane name and on the same terms,
  and what every message and every CLI argument means by a tab. Nothing tells a tab which tab it is, so no pane's
  environment carries one - a script reads it out of `muster window`.
- **vocabulary** - the backend contract's nouns and verbs, owned by Muster; the contract corpus is its executable
  form.
- **wake** - the one-line notice that messages are waiting, sent to a participant once per group until it reads;
  never the message itself. For an agent in a pane it is rung by the doorbell; for a Claude Code session outside
  one, it is a line on its inbox socket; a session given the messaging hooks fetches its own.
- **window** - the unit that holds an ordered list of Muster tabs and shows one of them, with an arrangement of its
  own under `~/.muster/state/windows/`. Two windows are two arrangements rather than two views of one, and a window is
  named after its arrangement (`window-2`), so it is the same window after a quit.
- **workspace** - a daemon's top-level container of tabs. Nothing a person using Muster has to know about: the
  adapter works out which one a tab belongs in, and no message and no CLI argument names one.
