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
  simplifies; we never let one own our contract - and the surface a person or an agent drives is part of that
  contract. Where Muster acts on the answer, the way to ask is Muster's, never the dependency's.
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
  API alike - parity by construction, not by discipline - so an agent can drive Muster as readily as a person can,
  through Muster's own surface and in Muster's own vocabulary.
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

## Configuring

Everything Muster owns lives in `~/.muster`, and `MUSTER_HOME` moves the lot. Two files in
it, both optional, both TOML. `~/.muster/config.toml` is yours to write, and it is the only
file Muster reads: `[[daemon]]` blocks name the machines a window attaches to, `[keymap]`
rebinds any of Muster's own actions, `[font]`, `[colors]` and `[cursor]` decide what the
window looks like, and the rest decides what a keystroke becomes on its way to a pane.

One directory rather than a file in each of the XDG trees, because Muster's surface is meant
to be discovered rather than taught - an agent that can list one directory needs no
documentation to find the whole of it. `XDG_CONFIG_HOME` and its family still decide where
herdr listens and what herdr reads; they no longer move anything of Muster's.

The rule, so that the next setting has an obvious home: **a setting is Muster's when Muster
acts on the answer**, including the ones it only translates onward for libghostty. It is the
daemon's when the daemon owns the thing being configured and Muster never sees it - what a
pane runs, how deep its scrollback is. And within the file, a table when a subject has
several answers, a root key when it has one.

```toml
option_as_alt = "left"         # never (the default) | always | left | right
resize_step = 2                # cells per resize chord; omit for the daemon's own step
scroll_multiplier = 1.5        # scales what the trackpad or wheel reported
pane_padding = 2               # points between a pane's text and its edges; 0 fits the most rows

[keymap]
split_right = "cmd+d"          # the default; Ghostty's, wherever Ghostty has one
split_left = "cmd+opt+d"       # ships unbound, as it does in Ghostty
zoom = "cmd+shift+return"
close_pane = ""                # unbound - the action stays, the shortcut goes

[text]
"shift+enter" = "\n"           # this chord sends these bytes, whatever the encoder would say

[font]
family = "Fira Code"           # omit for whatever monospace this machine would have picked
size = 13

[colors]
background = "#282c34"
foreground = "#ffffff"
cursor = "#f5e0dc"
cursor_text = "#1e1e2e"        # the character under the cursor
selection_background = "#414868"
selection_foreground = "#c0caf5"
divider = "#4a4a4a"            # the line between two regions; omit for the platform's
palette = [                    # the ANSI sixteen, all of them or none
  "#000000", "#cc0000", "#4e9a06", "#c4a000",
  "#3465a4", "#75507b", "#06989a", "#d3d7cf",
  "#555753", "#ef2929", "#8ae234", "#fce94f",
  "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
]

[cursor]
style = "block"                # block | bar | underline | hollow
blink = true                   # omit to let the program in the pane decide
```

`[keymap]` is partial, so a file that names one action rebinds one action. Chords are
modifiers and a key, in any order and any case, spelled the way you would say them: `cmd`,
`opt`, `ctrl`, `shift`, and `left`, `return`, `f5`, `[`. The actions are `new_tab`,
`next_tab`, `previous_tab`, `focus_tab_1` to `focus_tab_9`, `split_*` for each direction,
`close_pane`, `next_pane`, `previous_pane`, `focus_*` and `resize_*` for each direction,
`zoom`, `increase_font_size`, `decrease_font_size`, `reset_font_size`, `toggle_sidebar`,
`reload_config`, and `show_shortcuts`. On macOS these become menu items, which is
where the platform dispatches a key equivalent from - so a rebound action moves in the menu
too, and System Settings can move it again.

Two of them ship with no chord at all. Ghostty has `split_left` and `split_up` as actions and
binds neither, so Muster does the same rather than inventing a shortcut for them - they are in
the menu, one click away and one `[keymap]` line from a chord. Which of the four sides costs
herdr one request and which costs two is not something you can tell from here, and that is the
point: herdr splits rightward and downward only, so the other two are a split and a rearrange,
and Muster takes the arrangement from the daemon's own answer rather than letting you watch the
pane move.

The two ways of moving are different axes rather than two flavours of the same one.
`next_pane` and the four directions reach every pane the window is **showing**; `next_tab` and
`focus_tab_N` reach the tabs behind those, which nothing else can get to once the agent list
is put away. Both cross machines: a window's tabs are one numbered list, so `cmd+3` is the
third caption down the list whichever daemon holds it.

`option_as_alt` is the one that decides whether `opt+t` reaches an agent. macOS treats option
as a composing key, so by default it produces `†` and a program waiting for `alt+t` never
hears it. Naming a side keeps accented characters on the other hand. `[text]` is the escape
hatch beneath all of that: a chord bound there sends exactly those bytes and no encoder is
consulted. It is keyed by chord where `[keymap]` is keyed by action, because an action has
one chord and text has no name to key on.

The other three root keys are small answers a terminal is expected to let you change, each
one line because each is one value. `resize_step` is how many cells a resize chord moves a
divider; omit it and the daemon decides, which is what a chord meant before the key existed.
`scroll_multiplier` scales whatever your trackpad or wheel reported, so `1` is the device's
own answer and `0.5` is half of it - a multiplier rather than a line count, because how big
one notch is belongs to the device. `pane_padding` is the space between a pane's text and its
edges, one number for both axes; `0` is what fits the most rows into a window of fifteen
agents.

`[font]`, `[colors]` and `[cursor]` are the window's appearance, and every one of them is
optional. **Leave a value out and you get the renderer's own default, not one Muster
invented** - the vocabulary names what you may change and nothing else, because Muster has no
opinion about which monospace font your machine has and a default palette written into Muster
would be a transcription of somebody else's. `palette` is the sixteen ANSI colours, all of
them or none: a partial one leaves the rest as the renderer's and produces a scheme nobody
designed. `divider` sits with the pane colours even though Muster rather than the renderer
paints it, because you pick colours all at once and which piece of code holds the brush is
not something you should have to know.

Muster reads no file belonging to another application. It used to: fonts and colours came
from a Ghostty config if you had one, which is why the whole of `[colors]` is new rather than
a rename. If you configured Muster's appearance through Ghostty, that stops working and this
is where it moves to. `docs/architecture.md` says what the loan cost and why it went.

One thing Muster still does not decide: scrollback depth is the daemon's, because herdr owns
the buffer that a scroll intent moves.

**Saving the file is enough.** Muster watches it and reads it again, and `cmd+shift+,` or
Reload Configuration asks for the same thing when you would rather say so yourself - the
watcher dispatches that action rather than being a second way in. Colours, fonts, the cursor,
the keymap, `[text]` and `option_as_alt` all take effect where they are, including in panes
that were already open; `pane_padding` reaches panes opened afterwards, because that is as far
as the renderer takes it.

The exception is `[[daemon]]`. Which machines a window is attached to is a question about live
sessions rather than about settings, and answering it on a save would move panes somebody is
working in - so a change there is read, noticed, and reported as still wanting a relaunch. A
file that will not parse changes nothing at all and says so, which means an editor that saves
halfway through a thought cannot leave you running half a config.

`~/.muster/state/` is Muster's to write, and holds two files nobody should edit. `window.toml`
is rewritten whenever the window settles: which tabs it was showing, in what order, at what
widths, and under `[window]` whether the agent list was open and how far the text was sized
from what the config file asked for. Delete it and the next launch
opens fresh. Nothing about a session is in it - what a tab holds is the daemon's answer, asked
again on every launch.

Text size is the one appearance setting that is also an action. `cmd+=`, `cmd+-` and `cmd+0`
size every pane in the window at once - not one pane, because a grid you read at a glance is
harder to read with ragged cell sizes - and the size you land on is remembered under `[window]`
and comes back on the next launch. `cmd+0` is the way back to whatever `[font] size` says.

`libghostty.conf` is `[font]`, `[colors]` and `[cursor]` restated in the renderer's own
format, because libghostty has no way to be handed a value except as a file. Rewritten every
launch, so editing it changes nothing - but reading it answers "what did Muster actually tell
the renderer", which is the first question when a colour does not take.

## Building

`./dev` is the only supported way to build, test, and lint. With no flags it takes the full gate, and
`.github/workflows/gate.yml` runs that one command on every push and pull request - so a contributor's green and a
merge gate's green cannot drift apart. Flags narrow it and cluster: `./dev -t` tests, `./dev -tl` tests and lints,
`./dev -h` lists them all.

`./dev --bundle` assembles `.build/muster.app` around the built binary - a thing you can double-click, keep in the
Dock, or hand to somebody, with the pinned herdr and both dylibs inside it. Out of the gate because nothing in the
gate needs one, and it is also the only way to meet the descriptor ceiling launchd imposes on a GUI-launched process.

`./dev --contract` is the exception that stays out of the gate. It launches the real app against a real herdr and
reads its run log to see what connected, so it needs a daemon on PATH and a logged-in GUI session - neither of which
the default suite is allowed to require.

`./dev --ssh` is the remote tier, and sits out of the gate for the same reason: it starts the devenv container and
proves a forwarded socket is a socket - a real ssh master to a real machine running a real daemon - which needs
docker rather than a GUI session.

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

`deps/rad.pin` names the Rad the gate is written against, and is the one pin `./dev` cannot act on - it is already
running under Rad by the time it could look. CI installs exactly that build; your own `rad` is free to be any
version, and one too old to parse `./dev` says which line it could not read.

herdr is not something you install. Muster ships one: `deps/herdr.pin` names a release and a checksum per platform,
`./dev` downloads that binary into `deps/herdr/<version>/` once and verifies it, and every place that needs a daemon
gets that one - the tests spawn it, a build stages it beside the app, and `./dev --bundle` puts it inside
`muster.app`, where the app starts it on a socket of its own. Deliberately **not** your PATH, and deliberately not
the socket your own herdr uses: a Muster talking to a daemon its corpus was never recorded against is a window whose
every behaviour is unverified. So the herdr you run for your own work stays whatever version you want, a suite that
passed did so against the daemon the corpus was recorded with, and the app never meets either.
`MUSTER_HERDR=/path/to/herdr` overrides it for anyone
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
