# Origin

Founded 2026-08-11 from a research session and frozen the same day: it records what we found and decided at the
start, so future sessions inherit the vision instead of re-deriving it. Later understanding does not land here.
Doctrine that evolves lives in `architecture.md`, `testing.md`, and `glossary.md`; large decisions are recorded as
MIPs in `mip/`; open questions live on the kan board.

## The itch

We wanted herdr - agent state tracking and persistent, daemon-owned sessions - but the day-one experience grated. As a
guest TUI inside Ghostty, herdr cannot use the shortcuts that Ghostty, macOS, and the programs inside its panes already
own, and its splits are virtual panes composed into a single terminal grid. herdr's own keyboard docs concede the cost
("a chord has to survive three layers"), and the top complaint in its Hacker News debut thread is the same one.
Meanwhile the daily workflow spans two machines: Claude Code locally on a work MacBook, plus agents on a Linux devenv
over SSH. Agent-state visibility is the non-negotiable; it is the reason to leave plain Ghostty splits at all.

## What we found

**herdr** ([herdrdev/herdr](https://github.com/herdrdev/herdr), Apache-2.0, Rust, ~222k LOC at v0.8.0). Server/client
over a Unix socket. The server owns PTYs and terminal state - a vendored libghostty-vt plus ~8k lines of FFI - and
re-encodes composed screens to ANSI for a ratatui client to paint into the host terminal. Agent state detection is
server-side screen analysis: zero hooks, works identically local and remote. No native app plans exist in their issues
or discussions; the roadmap doubles down on being a better guest (forwarding notifications to the host terminal,
host-identity plugins). One session cannot mix local and remote panes
([discussion #2232](https://github.com/herdrdev/herdr/discussions/2232)). Their AGENTS.md declares a runtime/client
boundary guardrail: "Herdr is migrating toward a server-owned runtime protocol with the TUI as one client" - shared
runtime facts must live in server state behind the JSON API, presentation state stays client-side. A native client
therefore rides their sanctioned direction, not an accident of their API.

**cmux** ([manaflow-ai/cmux](https://github.com/manaflow-ai/cmux), GPL-3.0-or-later, Manaflow Inc., Swift/AppKit).
Native macOS shell over libghostty - via a deep ghostty fork, 595 commits ahead and 435 behind upstream when we
looked. Real splits with a Ghostty-matching default keymap (137 rebindable actions, no keyboard-resize action). Remote
support mirrors tmux control mode (`-CC`) into native splits; we found no evidence that agent-state tracking survives
the remote path - detection rides locally installed hooks. Despite both projects' compare pages suggesting "run herdr
inside cmux", cmux contains zero herdr integration. Local sessions die with the app, though a daemon is under
construction in-tree (cmux-tui, cmux-remote).

**libghostty** is two things: libghostty-vt (VT parsing and terminal state - the part being stabilized, already inside
herdr) and the full embedding API (app, surface, GPU rendering, input) that Ghostty's own macOS app runs on,
documented as unstable and subject to change between releases.

The incumbents are converging from opposite ends - herdr has the back half (daemon, states, persistence), cmux the
front half (native shell) - and neither has both. Nobody occupies the square we want: native feel, uniform agent
states, local and remote in one window, on a foundation we control.

Both projects also write down their principles. herdr's contributor guide: state separated from runtime, render is
pure, platform code isolated, and "multiplicative performance paths" - budget work by frequency times cardinality
(per byte, per render, across panes and clients) and profile at 1 and 15 panes. cmux's stated zen: composable
primitives, no imposed workflow; its founding complaint - context-free notifications across too many panes - named
the attention-routing problem. Muster's desiderata adopt the performance discipline, attention routing, one shared
action path behind every entrypoint (cmux's shared-behavior policy), and the no-workflow stance.

## The insight

herdr's socket API is already a view-model API. From its published schema (`herdr-api.schema.json`, shipped in the
crate):

- `events.subscribe`, `pane.agent_status_changed`, `pane.agent_detected` - push events; states are
  working / blocked / idle / done
- `layout.export` / `layout.updated` / `layout.apply`, `pane.layout`, `layout.set_split_ratio` - the split tree as
  data, with change notifications
- `pane.focus_direction`, `pane.edges`, `pane.neighbor` - navigation
- `herdr terminal session control <target>` - a writable per-pane stream: newline-delimited JSON in
  (`terminal.input`, `terminal.resize`), base64-encoded ANSI frames out

A native client can therefore render herdr truthfully without forking it. And a client attached to two daemons - one
local, one on the devenv over SSH - solves the mixed local/remote gap at the view layer; the daemon never needs to
know.

First-light trick: a libghostty surface spawns a command and parses its output, so a pane's command can be a small
bridge wrapping `terminal session control` - JSON frames unwrapped to raw bytes out, keys wrapped into
`terminal.input` in. Mode-changing sequences ride the same byte stream, so the local surface tracks the inner
program's terminal modes and should encode input correctly; stage 1 exists to prove that.

A closer review against herdr's source, the same day, corrected this picture in three places: the pane stream is a
server-rendered frame diff that *consumes* the inner program's mode changes, so input encoding cannot live in the
surface and must happen where the modes live (see architecture.md); the mirror bootstrap primitive is
`session.snapshot`, not `layout.export` (which exports a portable restore tree without live state); and herdr
tracks a fifth agent state, `unknown`, with idle-vs-done gated on backend-recognized focus, which a client must
feed. The bridge trick stands for output only.

## The decision

Build it ourselves. No fork of ghostty: embed libghostty and budget for API churn - cmux's 595/435 fork drift is the
cautionary tale. No fork of herdr: daemon-side gaps become upstream PRs to an Apache-2.0 project that releases weekly.

The fallback property that makes this safe to attempt: the herdr TUI keeps working against the same daemons, so a
rough Muster never blocks a workday. Worst case is Ghostty + herdr, having lost nothing.

Keeping the herdr exit real costs little, because the view-model's vocabulary is already smaller than herdr's API:
the core speaks Muster's own terms (pane tree, pane byte streams, agent states, intents), everything herdr-shaped
lives in one adapter, and the contract corpus is the executable definition of what any backend must provide. Three
exit lanes, cheapest first: fork herdr (Apache-2.0 makes this routine - the daemon stays, its stewardship changes),
replace it wholesale (the corpus is the spec), or adapt a different backend (tmux control mode could drive panes and
layout, though it has no agent states).

## The devenv container

The remote half of the workflow needs a machine to talk to, and depending on any particular one would make development
and CI unreproducible. So the repo carries its own: a Linux container (`devenv/`) running sshd, the
Linux herdr build, and scripted fake agents, reachable at `ssh -p 2222 dev@localhost`. The same container is the
integration-test fixture for the remote path, locally and in CI - the dev sandbox and the test environment are one
artifact.
