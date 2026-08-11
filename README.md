# Muster

Native workspace for AI coding agents: real splits, real keybindings, agent status at a glance, local and SSH agents
side by side in one window, on daemon-owned sessions that outlive the app.

Pre-alpha; nothing works yet. Start with `docs/origin.md` for why this exists and `docs/architecture.md` for the
shape.

## Desiderata

- **Agent states are the point.** Every pane shows working / blocked / done / idle at a glance, for local and remote
  agents alike. Lose this and the tool has no reason to exist.
- **Attention routing.** Glanceable states are the floor, not the ceiling: when an agent needs you, the notification
  says why, and one keystroke lands you on the pane that asked.
- **Native feel.** Real GPU-rendered splits and the platform's own keybindings. No prefix key, no chord-scavenging
  around a host terminal, no virtual splits composed into one grid.
- **Fast is a feature.** Input-to-glyph latency indistinguishable from plain Ghostty. Work is budgeted by frequency
  times cardinality - per byte, per event, per render, across panes and daemons - and profiled at 1 and 15 panes.
- **Local and remote in one window.** Harnesses on this machine and on an SSH devenv, rendered and driven identically.
- **Sessions outlive everything.** Backend daemons own the PTYs. Quit the app, drop the VPN, close the lid: agents
  keep working and every pane comes back.
- **View = f(daemon state).** The app owns no truth. Layout, agent state, and scrollback live in the backend; the app
  renders them and forwards intent.
- **Swappable organs, pragmatically.** The session backend ([herdr](https://github.com/herdrdev/herdr) today) and the
  renderer ([libghostty](https://github.com/ghostty-org/ghostty) today) sit behind narrow seams we own: the core
  speaks Muster's own vocabulary, each dependency lives in one adapter, and the contract corpus is the executable
  definition of what a replacement - wholesale, or a fork - must provide. We embrace a dependency where it
  simplifies; we never let one own our contract.
- **Green suite means it works.** Muster is built largely by AI agents, so the suite carries the confidence an
  author's memory cannot. What makes that achievable here: a thin shell over a thick headless core, so no logic hides
  in the untestable layer; the world faked only at the two seams, with the fakes audited against real herdr; oracles
  recorded from reality - terminal grids via libghostty-vt, intent on the wire - never pixels or internals;
  deterministic and offline.
- **Cross-platform stays open.** macOS ships first. The shell layer is thin and per-OS; nothing outside it may assume
  an OS. Both chosen organs already run on Linux and Windows.
- **AI-native surface.** Configuration is files. Every action runs through one shared path exposed to GUI, CLI, and
  API alike - parity by construction, not by discipline - so agents can drive Muster the way they drive herdr.

## Non-goals

Muster imposes no workflow: it provides panes, states, sessions, and a scriptable surface, and has no opinion about
how you run your agents.

- Not a terminal emulator - libghostty is.
- Not a multiplexer or session daemon - herdr is.
- Not an agent framework - Muster runs whatever agents you already run.

## Shape

    native shell (macOS first)        thin, per-OS, mostly dumb
      ├─ renderer seam → libghostty   real splits, GPU, VT fidelity
      └─ backend seam  → herdr API    JSON socket + ANSI pane streams
                           ├─ daemon on this machine   local agents
                           └─ daemon on devenv (SSH)   remote agents

## Repo conventions

- `docs/`: `origin.md` is the founding story, frozen as history; `architecture.md`, `testing.md`, and `glossary.md`
  are living doctrine; `docs/mip/` holds MIPs, the rare large decisions. Routine rationale lives in commit
  messages; open questions live in the kan board's `uncommitted` column.
- Work is tracked on the in-repo [kan](https://github.com/amterp/kan) board (`.kan/`, `kan list`). Agents keep it
  current: move cards as work starts and finishes, and add cards for work discovered along the way. If finishing work
  empties the `next` column, refill it from the backlog by priority and natural sequencing, so there is always a
  scoped next thing to pick up.
