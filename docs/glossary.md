# Glossary

One name per concept; docs and code use these terms. Alphabetical.

- **adapter** - the module translating the Muster vocabulary to one concrete backend; nothing backend-shaped escapes
  it.
- **agent state** - working / blocked / idle / done / unknown, per pane. Daemon-detected, except `done`, which Muster
  derives from seen-ness.
- **backend** - the daemon system that owns sessions; herdr today.
- **backend session** - one live connection to one daemon.
- **bridge** - the subprocess a surface runs to deliver a pane channel; output only.
- **composition** - the Muster-owned arrangement: which daemons are attached, which tabs the window holds and in
  what order, which of them is on screen, and how each divides between the machines holding panes in it. Not an
  input method's composition, which is a different thing with the same name and lives under `input::` wherever it
  appears in the code.
- **control plane** - everything except output: events, state, intents, input. Flows through the core.
- **core** - the headless, OS-free view-model: mirror, dispatcher, keymap, attention, config.
- **daemon** - one running backend server instance owning PTYs and sessions, local or remote.
- **data plane** - output only: pane channels, adapter to surface, bypassing the core.
- **devenv container** - the repo's Linux container (sshd, Linux herdr, scripted fake agents); dev sandbox and
  remote-path test fixture in one.
- **command endpoint** - the unix socket a window answers requests on, at
  `~/.muster/state/command-<pid>.sock`. The same schema the shell/core seam carries, arriving from another process -
  which is what the CLI is. A pane reads the path of its own window's from `MUSTER_SOCKET`.
- **frame** - one screen-diff message on a pane channel.
- **intent** - a requested mutation sent to a daemon (split, focus, resize, input, scroll, spawn). Muster never
  mutates; it requests.
- **mirror** - the core's disposable cache of daemon structure, bootstrapped from snapshot plus events; never
  authoritative.
- **pane** - one terminal inside a tab's tree; owned by a daemon.
- **pane name** - what Muster calls a pane: `p1w3r07bsd`, minted by Muster rather than borrowed from the backend,
  unique across every attached machine, and never reused. What every message and every CLI argument means by a pane.
  A pane reads its own from `MUSTER_PANE`.
- **pane channel** - the output stream feeding one surface. With herdr: server-rendered frame diffs, not raw program
  output.
- **pane tree** - the split layout inside one tab; daemon truth.
- **region** - the part of a Muster tab that one machine holds, as it sits on screen: that machine's pane tree,
  and how wide it is. One for every tab until somebody groups two.
- **roster** - every tab the window holds with its panes under it, ordered and labelled by the core, each row saying
  whether the window is showing it. Beside them, the machines - because a machine holding no panes contributes no
  tabs and would otherwise vanish. What the view is to the screen, this is to the session.
- **seam** - an injected boundary the core is tested and swapped at. Two exist: backend and renderer.
- **seen-ness** - whether anybody has looked at a pane since its agent finished; distinguishes idle from done. A pane
  is seen when it is on screen in a window holding the OS's focus, so Muster computes this rather than reading it -
  no daemon can see a window.
- **shell** - the per-OS native layer: windows, chrome, key capture, surfaces. Owns nothing.
- **surface** - one libghostty terminal view rendering one pane channel; disposable.
- **tab** - a named set of panes a window shows together. Muster's own unit, and the one thing here that is not a
  daemon's: a tab holding panes on one machine is one backend tab and comes back after a daemon restart like
  everything else, and a tab somebody has grouped to hold panes on two is one backend tab on each, held together by
  Muster's name registry and nowhere else. `docs/architecture.md`, durability, says what that costs.
- **tab name** - what Muster calls a tab: `t1w3r07bsd`, from the same registry as a pane name and on the same terms,
  and what every message and every CLI argument means by a tab. Nothing tells a tab which tab it is, so no pane's
  environment carries one - a script reads it out of `muster window`.
- **vocabulary** - the backend contract's nouns and verbs, owned by Muster; the contract corpus is its executable
  form.
- **window** - the unit that holds an ordered list of Muster tabs and shows one of them, with an arrangement of its
  own under `~/.muster/state/windows/`. Two windows are two arrangements rather than two views of one.
- **workspace** - a daemon's top-level container of tabs. Nothing a person using Muster has to know about: the
  adapter works out which one a tab belongs in, and no message and no CLI argument names one.
