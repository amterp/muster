# Glossary

One name per concept; docs and code use these terms. Alphabetical.

- **adapter** - the module translating the Muster vocabulary to one concrete backend; nothing backend-shaped escapes
  it.
- **agent state** - working / blocked / idle / done / unknown, per pane. Daemon-detected, except seen-ness, which
  clients feed.
- **backend** - the daemon system that owns sessions; herdr today.
- **backend session** - one live connection to one daemon.
- **bridge** - the subprocess a surface runs to deliver a pane channel; output only.
- **composition** - the Muster-owned arrangement: which daemons are attached, and which (daemon, workspace, tab)
  shows in which window region.
- **control plane** - everything except output: events, state, intents, input. Flows through the core.
- **core** - the headless, OS-free view-model: mirror, dispatcher, keymap, attention, config.
- **daemon** - one running backend server instance owning PTYs and sessions, local or remote.
- **data plane** - output only: pane channels, adapter to surface, bypassing the core.
- **devenv container** - the repo's Linux container (sshd, Linux herdr, scripted fake agents); dev sandbox and
  remote-path test fixture in one.
- **frame** - one screen-diff message on a pane channel.
- **intent** - a requested mutation sent to a daemon (split, focus, resize, input, scroll, spawn). Muster never
  mutates; it requests.
- **mirror** - the core's disposable cache of daemon structure, bootstrapped from snapshot plus events; never
  authoritative.
- **pane** - one terminal inside a tab's tree; owned by a daemon.
- **pane channel** - the output stream feeding one surface. With herdr: server-rendered frame diffs, not raw program
  output.
- **pane tree** - the split layout inside one tab; daemon truth.
- **region** - the part of a Muster window displaying one tab's pane tree.
- **seam** - an injected boundary the core is tested and swapped at. Two exist: backend and renderer.
- **seen-ness** - backend-recognized focus on a pane; distinguishes idle from done. Muster reports it as panes gain
  focus in its windows.
- **shell** - the per-OS native layer: windows, chrome, key capture, surfaces. Owns nothing.
- **surface** - one libghostty terminal view rendering one pane channel; disposable.
- **tab** - the unit that owns one pane tree, inside a workspace; daemon truth.
- **vocabulary** - the backend contract's nouns and verbs, owned by Muster; the contract corpus is its executable
  form.
- **workspace** - a daemon's top-level container of tabs.
