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
- **Sessions outlive the app, and their shape outlives the daemon.** Backend daemons own the PTYs, so quitting
  Muster, dropping the VPN or closing the lid costs nothing: agents keep working and every pane comes back. Below
  that line the guarantee weakens honestly rather than silently - a daemon restart returns the pane tree and each
  pane's directory but not the processes, and a reboot is the same case with the daemon to start first. What can be
  written down is written down; a live process cannot be, and Muster does not pretend otherwise
  (`docs/architecture.md`, durability).
- **View = f(daemon state).** The app owns no truth. Layout, agent state, and scrollback live in the backend; the app
  renders them and forwards intent.
- **Swappable organs, pragmatically.** The session backend ([herdr](https://github.com/herdrdev/herdr) today) and the
  renderer ([libghostty](https://github.com/ghostty-org/ghostty) today) sit behind narrow seams we own: the core
  speaks Muster's own vocabulary, each dependency lives in one adapter, and the contract corpus is the executable
  definition of what a replacement - wholesale, or a fork - must provide. We embrace a dependency where it
  simplifies; we never let one own our contract.
- **Green suite means it works.** Muster is built largely by AI agents, so the suite carries the confidence an
  author's memory cannot. What makes that achievable here: a thin shell over a thick headless core, so no logic hides
  in the untestable layer; a real, version-pinned herdr behind the backend seam rather than a stand-in, because a
  stand-in is Muster's own guess at a daemon and a wrong guess passes; oracles recorded from reality - terminal grids
  via libghostty-vt, intent on the wire - never pixels or internals; deterministic, and offline in the sense that
  nothing reaches the network.
- **Every run explains itself.** Muster is several processes, often on several machines, and a symptom in one usually
  has its cause in another: a window that ignores the keyboard is a bridge that never started, or one that started and
  could not dial back. Each run leaves a single machine-readable timeline spanning all of them, so a bug report is a
  file an agent can read rather than a session someone has to re-stage. Terminals carry secrets, so what you typed is
  never in it unless you ask for it.
- **Cross-platform stays open.** macOS ships first. The shell layer is thin and per-OS; nothing outside it may assume
  an OS, and the core is portable by construction rather than by intention - it is a different language from the
  shell, so an OS type cannot leak into it by accident. Both chosen organs already run on Linux and Windows.
- **AI-native surface.** Configuration is files. Every action runs through one shared path exposed to GUI, CLI, and
  API alike - parity by construction, not by discipline - so agents can drive Muster the way they drive herdr.
- **Harness-agnostic.** Strive to support many harnesses.

## Non-goals

Muster imposes no workflow: it provides panes, states, sessions, and a scriptable surface, and has no opinion about
how you run your agents.

- Not a terminal emulator - libghostty is.
- Not a multiplexer or session daemon - herdr is.
- Not an agent framework - Muster runs whatever agents you already run.

## Shape

    native shell (macOS first)        thin, per-OS, mostly dumb - Swift + AppKit today
      ├─ renderer seam → libghostty   real splits, GPU, VT fidelity
      └─ core seam     → one symbol   protobuf over a C ABI; events, never bytes
           portable core (Rust)       mirror, keymap, dispatch, attention, config
             └─ backend seam → herdr  JSON socket + ANSI pane streams
                  ├─ one bridge per pane      unwraps frames onto a surface's PTY
                  ├─ daemon on this machine   local agents
                  └─ daemon on devenv (SSH)   remote agents

## Building

`./dev` is the only supported way to build, test, and lint. With no flags it takes the full gate, which is also all
CI runs, so a contributor's green and a merge gate's green cannot drift apart. Flags narrow it and cluster: `./dev -t`
tests, `./dev -tl` tests and lints, `./dev -h` lists them all.

`./dev --contract` is the exception that stays out of the gate. It launches the real app against a real herdr and
reads its run log to see what connected, so it needs a daemon on PATH and a logged-in GUI session - neither of which
the default suite is allowed to require.

`./dev --perf` and `./dev --latency` are the other two out-of-gate tiers: the first measures the per-unit budgets
against a checked-in baseline and fails on regression, the second times input-to-glyph against a real daemon, at one
pane and at a full window of fifteen with fourteen of them printing. A functional green is never a performance
claim, so neither runs by default.

Two toolchains, one door: the gate builds, tests and lints the Rust core and the Swift shell together, and a suite
that discovers zero tests fails in either language rather than reporting green.

Requires a Swift 6.2 toolchain, [Rad](https://github.com/amterp/rad), and Zig 0.16 on your PATH; a missing `rad`
shows up as `env: rad: No such file or directory`. Rust installs itself - `rust-toolchain.toml` pins the version and
rustup fetches it on the first `cargo` call, the same way `deps/ghostty.pin` decides which libghostty gets built.
libclang, which the libghostty-vt bindings are generated with, comes from the Xcode command line tools.

herdr is a test dependency as well as a runtime one, because tests that need a daemon spawn a real one rather than
a stand-in. It is deliberately **not** taken from your PATH: `deps/herdr.pin` names a release and a checksum per
platform, and `./dev -t` downloads that binary into `deps/herdr/<version>/` once, verifies it, and hands the path
down to the tests. So the herdr you run for your own work stays whatever version you want, and a suite that passed
did so against the daemon the corpus was recorded with. `MUSTER_HERDR=/path/to/herdr` overrides it for anyone
bisecting herdr itself, and the run says so when it does. A daemon whose wire schema differs from
`corpus/herdr-<version>/api-schema.json` fails the run with the command that shows what moved, rather than
surfacing later as a confusing test failure. The download is the one step that touches the network; everything
after it is offline.

The seam's types are generated from `proto/muster.proto` on both sides and committed on neither, so a checkout
cannot hold a shell and a core that disagree. Neither generator is a thing you install: Rust compiles the schema
with a Rust library, and swift-protobuf vendors protoc's own source, so `./dev` builds it. `./dev --proto`
regenerates on demand; a normal build does it only when the schema's hash changes.

## Repo conventions

- `docs/`: `origin.md` is the founding story, frozen as history; `architecture.md`, `testing.md`, and `glossary.md`
  are living doctrine; `docs/mip/` holds MIPs, the rare large decisions; `docs/observations/` records what a
  dependency was measured doing, one file per version, each claim citing raw transcripts in `corpus/`. Routine
  rationale lives in commit messages; open questions live in the kan board's `uncommitted` column.
- `corpus/`: what the code is judged against, in no language. `conformance/` holds the cases that define the core's
  behavior, `snapshots/` the rendered oracles too broad to be cases, and the rest raw transcripts recorded from a
  real dependency. The gate fails if a file here is checked in and never run.
- `crates/` is the portable core (Rust), `Sources/` the macOS shell (Swift), and `proto/muster.proto` plus
  `include/muster.h` are the seam between them. Both languages are built, tested and linted by `./dev`.
- Work is tracked on the in-repo [kan](https://github.com/amterp/kan) board (`.kan/`, `kan list`). Agents keep it
  current: move cards as work starts and finishes, and add cards for work discovered along the way. If finishing work
  empties the `next` column, refill it from the backlog by priority and natural sequencing, so there is always a
  scoped next thing to pick up.

## Contributing

- Keep ./docs and this README.md up to date with changes. Don't inflate them needlessly, be judicious.
- Use Conventional Commits for commits.
