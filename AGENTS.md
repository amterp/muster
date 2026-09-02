# Muster

Native workspace for AI coding agents: real splits, real keybindings, agent status at a glance,
local and SSH agents side by side in one window, on daemon-owned sessions that outlive the app.

This file is the project's own account of itself: what Muster is for, how it is shaped, how it is built, and the
conventions a contributor works under. `README.md` is the front door for somebody deciding whether to install it,
and carries none of this. A `CLAUDE.md` symlink beside it points here; it is gitignored, so make your own
if your harness wants one.

**Early, and specific about which parts.** Built, and covered by the suite: splits and tabs, a rebindable keymap
that ships Ghostty's chords where Ghostty has one, an agent list carrying a state on every row with a chord to each
of the first nine, renaming, trading two agents' places by dragging a row, configuration that reloads when you save
it, a CLI that drives the window from inside a pane, a notification when an agent needs you that takes you to the
pane that asked, and a second daemon on an SSH machine in the same window - where Muster installs its own herdr
rather than trusting whatever is over there. Not built, and worth knowing before you install rather than after: the
shape of a split cannot be changed once it is made, mouse buttons and motion do not reach a pane, a pane on a
devenv cannot drive the window it is drawn in, and find is known to mishandle a long scrollback.

`docs/origin.md` is why this exists, `docs/architecture.md` is the shape, `docs/configuration.md` is every
setting, and `docs/cli/limits.md` is the same honest account for the CLI.

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
  shell, so an OS type cannot leak into it by accident. Inside that layer the most native answer wins: portability
  constrains the core, and is never a reason to make Muster feel less like the machine it is running on. Both chosen
  organs already run on Linux and Windows.
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

`docs/configuration.md` is every key Muster reads, what it means, and why it is spelled the way it is. What belongs
here instead is the rule that decides where the next setting goes: **a setting is Muster's when Muster acts on the
answer or hands it on**, including the ones it only translates onward - for libghostty, and now for herdr. It is the
daemon's when it is about the daemon's own interface, which Muster never shows you. And within the file, a table when
a subject has several answers, a root key when it has one.

## Driving Muster

Everything a chord does, `muster` does. `muster window` says what a window is showing and what
every agent in it is doing, `muster pane new` makes a pane, and `muster pane send` types into one
by name - all of it the same requests the keyboard sends, because there is one path through the
core and this is the other door into it.

    muster window
    muster pane new --down --run claude --name "🤖 A"
    muster pane send --pane p1w3r07bsd "read AGENTS.md and wait" --enter
    muster tab focus t1w3r07bsd
    muster window --json

**Tabs are named too, and for a narrower reason than panes.** `t1w3r07bsd`, minted by Muster and
unique across every machine a window shows, so `muster tab focus` and `muster tab rename` reach one
without saying which daemon holds it. What a tab does not get is a name in any pane's environment:
nothing has to tell a tab which tab it is, so there is no `$MUSTER_TAB`, and a script that means the
tab it is sitting in reads that out of `muster window` - where every pane says which tab holds it.

`muster docs` is the reference and it ships inside the binary, so it describes the version you are
running. `muster --help` has the grammar, `muster completions zsh` writes a completion script.

Every pane Muster makes can drive the window it is drawn in without being set up first: the
command is on its `PATH` from `~/.muster/bin`, `$MUSTER_PANE` says which pane it is, and
`$MUSTER_SOCKET` says which window to tell. So `muster pane new` inside a pane splits that pane,
and an agent told "split two panes below you and start an agent in each" can do it.

**Two paths reach `muster`, and it matters less than it looks which one you get.**
`~/.muster/bin/muster` is the app's, repointed at every launch to the CLI of the app that is
running, and Muster hands its daemon a `PATH` with that directory at the front. Homebrew's
`muster` is a second link, into `/Applications`, for terminals that are not panes. Whichever
one a pane ends up finding drives the window it is drawn in, because both read `$MUSTER_SOCKET`
and that is what names the window - so the two do not compete over *which* window, only over
which build of the CLI answers, and that is a question only while they are different versions.

Front of the `PATH` is what Muster asks for rather than what you necessarily get. A login shell
rebuilds `PATH` from your profile after the daemon has handed one over, and on the machine this
was measured on that leaves `~/.muster/bin` in 49th place and `/opt/homebrew/bin` in 20th. If you
install no Homebrew copy, add `~/.muster/bin` to your own `PATH` and terminals outside Muster
reach the running app's CLI the same way.

Muster imposes no workflow. These are primitives, and `extras/skill/SKILL.md` is a Claude Code
skill that points an agent at them and nothing more.

## Building

`./dev` is the only supported way to build, test, and lint. With no flags it takes the full gate, and
`.github/workflows/gate.yml` runs that one command on every push and pull request - so a contributor's green and a
merge gate's green cannot drift apart. A second workflow, `corpus-linux.yml`, runs `./dev --corpus-linux` beside it
on a Linux runner. Flags narrow it and cluster: `./dev -t` tests, `./dev -tl` tests and lints,
`./dev -h` lists them all.

**A narrowed flag still takes what it cannot run without**, so `./dev -t` on a checkout nothing has been built in
fetches libghostty and generates the seam's types before running anything. Both are near-free once they are there -
a stamp read and four path checks - and the alternative was worse than slow: the seam's Swift types are generated
during a build and committed nowhere, so a suite that skipped it ran the shell against whatever was generated last
and went green while the schema said something else.

`./dev --bundle` assembles `.build/muster.app` around the built binary - a thing you can double-click, keep in the
Dock, or hand to somebody, with the pinned herdr, both dylibs, the icon and the licenses inside it. Out of the gate
because nothing in the gate needs one, and it is also the only way to meet the descriptor ceiling launchd imposes on
a GUI-launched process. The signature is ad-hoc, which is what macOS needs to run a Mach-O at all and asserts nothing
about who built it.

**`MUSTER_SIGN_IDENTITY` is what makes a bundle somebody else can run**, and `./dev --notarize` is the rest of it.
Set the variable to a Developer ID and the same `--bundle` signs with it instead, under the hardened runtime and with
`packaging/muster-release.entitlements` rather than the debug entitlement SwiftPM signs a local build with - and the
run says which identity it used. `--notarize` then sends the result to Apple, waits, staples the ticket into the
bundle and asks `spctl` what a stranger's machine will conclude - which is the only answer worth having, since a
signature that verifies locally and a notarized one look identical until Gatekeeper is asked.

Credentials come from a `notarytool` keychain profile, so notarizing your own build needs no key file on disk:
`xcrun notarytool store-credentials muster-notary` once, and `MUSTER_NOTARY_PROFILE` names a profile stored under
another name. CI has no keychain worth storing one in, so `.github/workflows/release.yml` hands over the App Store
Connect key itself from repository secrets, and its header comment names the whole set. It runs both flags on a `v*`
tag, and refuses a tag that disagrees with `Cargo.toml`'s version.

A release is Apple Silicon only, and `--notarize` refuses to run anywhere else. Building universal would mean
cross-building libghostty under Zig for a second architecture, and the Homebrew cask in `packaging/homebrew/` says
`depends_on arch: :arm64` so an Intel Mac is turned away by brew rather than by a crash.

`./dev --contract` is the exception that stays out of the gate. It launches the real app against a real herdr and
reads its run log to see what connected, so it needs a daemon on PATH and a logged-in GUI session - neither of which
the default suite is allowed to require. It assembles a bundle on the way past and launches that too, with launchd's
own `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, because a bundle is a different layout from the one SwiftPM leaves behind -
the daemon moves into `Contents/Library/` - and 0.3.0 shipped a cask where no pane rendered while every check here
was green against the other layout.

`./dev --ssh` is the remote tier, and sits out of the gate for the same reason: it needs docker rather than a GUI
session. It recreates the devenv container, runs the remote tests against it while it holds no daemon at all - which
is the claim, that Muster puts its own herdr on a machine with nothing on it and a pane there works - and then drops
a pinned Linux daemon in for the corpus half below. It leaves that container running, deliberately: a devenv is a
thing you keep, and recreating it per run is what `up` is for. The tier says so on the way out, because the thing
most likely to run next is `--perf`, and a container running beside a benchmark is enough to move the numbers it
judges - `./devenv/devenv down` when you are finished with it.

`./dev --corpus-linux` is that corpus half on its own: record the probe's scenarios against the container's Linux
daemon and diff them against the macOS recording, which is what catches a herdr re-pin that moves one platform and
not the other. Docker and python3 are the whole toolchain, no Rust, Swift or Zig - so this one *is* in CI, as the
`corpus-linux` workflow, where the rest of the remote tier is not.

`./dev --perf` and `./dev --latency` are the other two out-of-gate tiers: the first measures the per-unit budgets
against a checked-in baseline and fails on regression, the second times input-to-glyph against a real daemon, at one
pane and at a full window of fifteen with fourteen of them printing. A functional green is never a performance
claim, so neither runs by default.

`--perf` also refuses to run at all on a machine whose fast cores are already committed. Everywhere else a busy
machine only makes a run slow; here it makes the run lie against a file in the repository, and a tier that fails for
reasons unrelated to the code is one people learn to skip. The line is a one-minute load average at the machine's
performance-core count, which on Apple silicon is fewer than its cores - past that many runnable threads, work
starts landing on the slower ones. `./dev --perf --anyway` measures regardless, for when you want the numbers and
not the verdict.

`./dev --doctor` says what this repo's own tooling has left running on this machine: herdr daemons paired with the
work each one holds, the devenv container, and whatever is currently eating the CPU. It answers the question the
load average at the top of every run raises and cannot itself answer, which is *what* is busy - and it is what makes
ending a stray daemon safe, since the process holding somebody's live agent looks exactly like the nineteen that
hold nothing.

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
`muster.app` - as a helper application of its own, which the app starts through Launch Services on a socket of its
own, so that macOS charges every pane's permission prompts to the daemon rather than to a Muster that will quit
(`docs/observations/macos-26.4.1.md`). Deliberately **not** your PATH, and deliberately not
the socket your own herdr uses: a Muster talking to a daemon its corpus was never recorded against is a window whose
every behaviour is unverified. So the herdr you run for your own work stays whatever version you want, a suite that
passed did so against the daemon the corpus was recorded with, and the app never meets either.
`MUSTER_HERDR=/path/to/herdr` overrides it for anyone
bisecting herdr itself, and the run says so when it does. A daemon whose wire schema differs from
`corpus/herdr-<version>/api-schema.json` fails the run with the command that shows what moved, rather than
surfacing later as a confusing test failure. The download is the one step that touches the network; everything
after it is offline.

The same pin is compiled into the app, which is how a machine you attach over SSH gets the daemon everything was
tested against rather than whatever was installed there. `./dev --ssh` fetches that machine's asset against the same
pin and hands it to the remote tests as a filled cache, so the tier proves the install over ssh and still reaches
nothing itself.

The seam's types are generated from `proto/muster.proto` on both sides and committed on neither, so a checkout
cannot hold a shell and a core that disagree. Neither generator is a thing you install: Rust compiles the schema
with a Rust library, and swift-protobuf vendors protoc's own source, so `./dev` builds it. `./dev --proto`
regenerates on demand; a normal build does it only when the schema's hash changes.

## Repo conventions

- `docs/`: `origin.md` is the founding story, frozen as history; `architecture.md`, `configuration.md`, `testing.md` and
  `glossary.md` are living doctrine; `docs/mip/` holds MIPs, the rare large decisions; `docs/observations/` records what a
  dependency was measured doing, one file per version, each claim citing raw transcripts in `corpus/`. Routine
  rationale lives in commit messages; open questions live in the kan board's `uncommitted` column. `docs/cli/` is
  the reference `muster docs` ships inside the CLI binary, so a file there is prose the gate checks is reachable.
- `extras/` holds things that are Muster-adjacent rather than Muster: today one Claude Code skill pointing an agent
  at `muster docs`.
- `packaging/` is everything that exists only so that Muster can leave this machine: the icon and its source, the
  entitlements a release is signed with, and the Homebrew cask - which lives here rather than only in the tap because
  it changes when the app does, and should be reviewed beside it.
- `licenses/` is what the bundle carries about software that is not ours. `THIRD-PARTY.md` is generated by
  `tools/licenses.py` from the dependency graph and the gate fails when it stops describing what the binaries
  contain; `NOTICE` covers what ships as a whole file rather than compiled in.
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

- Keep `docs/`, this file, and `README.md` up to date with changes. Don't inflate them needlessly, be judicious.
- Use Conventional Commits for commits.
