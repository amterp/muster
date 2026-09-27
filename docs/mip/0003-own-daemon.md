---
mip: 3
title: A daemon of Muster's own, and herdr removed
status: Draft
kind: Architecture
created: 2026-09-26
decided:
supersedes:
superseded-by:
related: 1, 2
---

# MIP-3: A daemon of Muster's own, and herdr removed

## Summary

Muster replaces herdr with a daemon built from this repository, `muster-daemon`, and removes
herdr entirely in one cut. The daemon provides what Muster's desiderata need and nothing else:
panes whose processes outlive the app, the shape of every tab, agent state for every pane, and a
byte stream per pane. It runs on macOS and Linux. Windows is not a target.

Five decisions carry the rest:

- **Passthrough.** A pane's output reaches its surface as the program's own bytes. The daemon
  keeps a headless libghostty-vt terminal per pane beside the stream, for agent detection, reads,
  and catching up a surface that attaches late. Because the surface parses the real stream,
  Ghostty's own scrollback, search, selection and mouse handling work in every pane.
- **The daemon is the only writer to a pane's PTY.** Input reaches it as structured events, and it
  encodes them against the pane's real terminal modes with the libghostty encoder Ghostty uses.
  The surface receives the same events, so Ghostty's side effects of typing and scrolling happen,
  but whatever the surface writes back is discarded. The daemon is the only thing that answers a
  program's terminal queries.
- **Agent detection is a port of herdr's detection**, with herdr's detection manifests carried as
  data under Apache-2.0.
- **The protocol is Muster's**: protobuf over a Unix socket, persistent connections, a sequence
  number on every event, and Muster's names for tabs and panes on the wire.
- **The daemon is the process a message service will later live in**: agents in different panes,
  on different machines, passing messages to each other (kan `a_2Rtd0Ed0l`). None of the message
  service is built here. The crate layout and the protocol leave room for it.

herdr is about 126k lines of production Rust, and roughly 80% of it serves things Muster does not
use. A daemon covering what Muster does use is estimated at 9-13k lines. Typing, output and search
must feel the same as in plain Ghostty.

## Context / Motivation

### What herdr gives Muster

herdr owns every pane's PTY in a daemon that outlives the app. It restores the tab tree and each
pane's directory after a daemon restart. It detects agent state by reading each pane's screen,
with no hooks, identically on local and remote machines. It streams each pane to a client, and
its socket protocol works unchanged over a forwarded ssh socket. `docs/origin.md` chose it on those
grounds, and named three ways out if it stopped fitting: fork it, replace it with the corpus as
the spec, or adapt another backend.

### What it costs

Each cost below was measured, and is recorded in `docs/observations/herdr-0.8.0.md` and in the kan
card cited beside it:

- **herdr re-renders every pane's screen instead of forwarding its bytes.** Its daemon draws each
  attached client's pane into a cell grid, diffs it, and sends escape sequences for the changed
  cells. That costs about 1.5 ms of CPU per echoed keystroke per attached client, and sends 82
  bytes to the surface per echoed byte (`a_2KHGXkKSc`). All rendering and all API requests share
  one thread.
- **Typing waits for the next render slot.** Input-to-glyph is bimodal: a median of 1.4 ms and a
  p95 of 22.6 ms. herdr renders at most once per 16 ms across every client, so a keystroke that
  lands just after a render waits for the next one (`a_27GiE7gJJ`).
- **The surface never sees the program's terminal modes.** The re-rendered stream carries no mode
  changes, so Muster encodes keys blind against a guessed profile, sends arrows and paste through a
  second API that encodes them properly, and sends no mouse buttons at all (`a_27CTRluw7`,
  `a_27CTgqqdv`).
- **The surface holds no scrollback.** Find reaches 1000 rows through herdr's API, and nothing
  behind the screen of a full-screen program (`a_29Ayr1P8F`). Ghostty's own search, which covers
  all of a surface's scrollback, has nothing to search (`docs/observations/libghostty-9f9b8d1d.md`
  section 10).
- **A remote pane costs an ssh exec.** herdr serves pane streams only through a CLI, so each
  remote pane starts `herdr terminal session control` over ssh, about 400 ms per pane per launch
  (`a_2I776rXVS`), and a hidden remote pane cannot be detached cheaply (`a_2WDF2781G`).
- **herdr drops any client frame over 2 MiB**, freezing a pane past about 100k cells
  (`a_2KHGYMpnK`).

Eighteen of the twenty-nine cards in the board's `uncommitted` column are herdr findings or asks
of herdr's maintainers.

### What Muster does to make herdr safe to build on

Muster carries code whose only job is to cope with herdr's behavior:

- it rebuilds each tab's pane tree from the flat rectangles herdr publishes;
- it holds one subscription per pane, because no session-wide event carries agent state;
- it rejects old events herdr replays to every new subscriber, and remembers closed panes so a
  replayed creation cannot bring one back;
- it infers a tab closing, which herdr does without announcing it;
- it applies herdr's answer to a request as soon as it arrives, then recognizes and drops the
  broadcast of the same change that arrives about 100 ms later;
- it mints its own pane names, because herdr names a pane only in its answer, after the process
  has already started without knowing its name;
- it builds left and up splits from a split and a swap, and types a new pane's command after
  polling for a shell prompt;
- it strips title spinners herdr misses;
- it detaches hidden panes' bridges to save herdr render time;
- it resizes every pane back to herdr's own layout on quit, because herdr keeps a PTY at the size
  its last client set.

The new daemon makes each of these unnecessary; section 14 lists what is deleted.

### The goal

Stated by amterp when this was scoped, 2026-09-26:

- macOS and Linux. No Windows support.
- Only what Muster needs to meet its vision, not parity with herdr.
- No technical debt carried across: decide each part from the ground up.
- Responsiveness and feel equivalent to native Ghostty, including search and the other things a
  Ghostty user is used to.
- A clean cut: no release runs herdr and the new daemon side by side.

## Decision

### 1. One `muster-daemon` per machine, per install

`muster-daemon` is a Rust binary built from this workspace. One runs per machine per user for each
install, on a socket named for that install, so a development build never adopts the release
daemon and tests never adopt either. The socket is `~/.muster/daemon/<install>.sock`, and the install
name is fixed when the protocol crate is built: `MUSTER_INSTALL` for a build that ships (a release
sets `release`), otherwise `dev-` and a hash of the checkout's path, so two working trees never share
a daemon. Tests pass a socket of their own. A lock file beside the socket makes the first daemon to
start the only one; a second exits and its starter dials the first.

On the Mac it is the executable inside `MusterSessions.app`, started through Launch Services
exactly as herdr is today and for the same reason: the process that owns the PTYs must be its own
responsible process, so macOS charges every pane's permission prompts to one named bundle that
outlives the app (`docs/observations/macos-26.4.1.md` section 8). The bundle id stays
`dev.amterp.muster.sessions`, so users keep the permissions they have granted. The daemon opts out
of App Nap and timer coalescing, and runs its per-pane threads at user-interactive QoS.

On a remote machine the app installs and starts it over ssh, in a session of its own (`setsid`),
so it survives the ssh connection and logind's `KillUserProcesses` where a distribution enables
it.

### 2. What the daemon holds

**Muster's units, by Muster's names.** A daemon holds tabs and panes. There is no equivalent of
herdr's workspace. A tab is known by its Muster tab name (`t1w3r07bsd`) and a pane by its Muster
pane name (`p1w3r07bsd`), both minted by Muster and passed in the request that creates them. The
name registry (`crates/muster-core/src/names.rs`) keeps minting names and loses its binding to
backend ids, because there are none.

A tab holds a pane tree over this machine's panes: each split has an axis and a ratio, each leaf is
a pane, and the tab records which pane is zoomed. The daemon sends the tree as a tree.

**A tab grouped across two machines is recorded on both daemons by membership only.** Each
daemon's part of the tab carries the Muster tab name it belongs to, so two daemons that each hold
a part of `t1w3r07bsd` are two regions of one tab and no file of Muster's has to record the
grouping. This closes the weaker durability tier MIP-2 accepted, where a grouped tab lived only in
`~/.muster/state/names.toml`. Region order and weights stay in the window's arrangement, where
`docs/architecture.md` already puts them ("The arrangement over regions is Muster's, and only
Muster's"). The tab's label is stored on every part with a generation counter; the core adopts the
highest and rewrites a lagging part when its daemon reconnects. When a grouped tab is closed while
one machine is unreachable, that machine's part returns as a tab of its own when it reconnects,
because its processes are still running.

**Per pane**: its name, cwd, the command it was started with, its title, its agent and agent
state, and whether its process is alive.

**What a restart needs is persisted, and nothing else.** The daemon writes one versioned file with
an atomic rename, shortly after each change and on shutdown: tabs, trees, ratios, zoom, labels,
pane names, each pane's current directory (from OSC 7, falling back to the foreground process's
cwd, written debounced), and the last palette and cell size an app gave it. Titles, agent state and
process liveness are observations and are never written. A daemon that starts and finds the file
rebuilds every tab and starts a shell in each pane's recorded directory, keeping every name. It
does not re-run commands: an agent restarted fresh in fifteen panes is not what anyone asked for.
Processes and scrollback do not survive a daemon restart, which is the guarantee herdr gives today
(`docs/architecture.md`, durability).

### 3. Starting a pane

A create request carries the pane's grid in cells and pixels, computed by the core from the
window's layout, so a program draws its first screen at the size it will be shown. A pane created
where no window shows it gets the size of the pane it was split from.

The daemon starts the user's shell as an interactive login shell. A pane given a command runs it
through that shell and then replaces it with an interactive shell (`$SHELL -l -i -c '<command>;
exec $SHELL -l -i'`), so the command starts immediately with no typed input for a program to
discard, and the pane drops to a shell when the command exits. This removes the prompt polling
Muster does today.

A pane's environment is the daemon's own (launchd's minimal environment on the Mac, which the login
shell builds on), plus:

- `TERM=xterm-ghostty`, with the terminfo entry carried by the daemon and reached through
  `TERMINFO_DIRS`, on both machines, so no host needs Ghostty installed;
- `COLORTERM=truecolor`;
- `MUSTER_PANE` and `MUSTER_SOCKET`;
- Ghostty's shell integration for bash, zsh and fish, injected the way Ghostty injects it, so
  prompts carry OSC 133 marks and `jump_to_prompt` and prompt-aware selection work.

The daemon's `GHOSTTY_TERMINAL_OPT_TERMINFO_NAME` matches `TERM`, so XTGETTCAP answers agree with
it.

### 4. Output: passthrough

Each pane has a reader thread. Every chunk it reads from the PTY is sent unchanged to the bridge
attached to the pane, then written into the pane's headless libghostty-vt terminal. The bridge
writes the chunk to its stdout, which is the surface's PTY, so the surface's own libghostty parses
exactly the bytes the program wrote.

**Nothing on this path waits for another pane or for a request.** No render loop, no shared thread,
no throttle. The reader writes to the bridge's connection directly. Requests, agent detection and
attaching take the pane's lock briefly, and never while formatting a long history.

**Flow control is by credit.** A bridge acknowledges the bytes it has written to the surface. The
daemon keeps at most a fixed window of unacknowledged bytes per pane (256 KB to start, tuned by
measurement), and when the window is full it stops sending and marks the pane as behind. Credit
detects a slow reader on the far side of ssh, where the daemon's own queue depth does not, because
sshd and TCP buffer megabytes before the daemon's writes block. A pane that falls behind is caught
up with the screen only, not its history: bytes that scrolled off during a flood are in the daemon,
where `muster pane read` reaches them, and not in the surface's scrollback. The PTY reader never
blocks on a bridge.

**One bridge per pane.** A bridge holds a pane at a time, and its grid is the pane's size. A second
attach must ask for takeover or is refused; the displaced bridge is told why. A pane keeps its last
size when its bridge detaches.

**Hidden panes stay attached.** A hidden pane costs the daemon a socket write per chunk and its
surface a parse, the same as a background tab in Ghostty. The bridge's parking logic
(`crates/muster-bridge/src/attachment.rs`) goes.

**Streams ride the daemon's socket.** A bridge connects to the daemon's socket, locally or through
the forwarded ssh socket, and asks for a pane's stream. A remote pane no longer costs an ssh exec,
and a remote bridge restarts as cheaply as a local one. A libghostty surface only accepts bytes
from the command it spawns (`docs/observations/libghostty-9f9b8d1d.md` section 2), so the bridge
process stays.

**The surface is not a byte-exact copy of the headless terminal.** A resize reaches the surface
before the daemon, so output in flight is laid out at different sizes in each. Native Ghostty has
the same race with one copy; here the copies can differ until the next replay, which rewrites the
surface from the daemon's copy. The daemon's copy is what agent detection and `muster pane read`
see. The surface's copy is what Ghostty's search and selection see. `scrollback_bytes` sizes both.

**Kitty graphics pass through and are not replayed.** The daemon stores images with a small limit,
so a program that probes support gets a truthful answer, and refuses the file, temporary-file and
shared-memory transmission media, which name paths on the daemon's machine that a remote surface
cannot read. Programs then fall back to sending images inline. An image is gone from the surface
after a replay.

### 5. Attaching: the replay

When a bridge attaches, the daemon has to bring a fresh surface to the pane's current state before
live bytes follow. It does this with a replay: a VT stream Muster composes from the headless
terminal, using libghostty-vt's formatter for each part. The formatter's single all-in-one output
is not usable as a replay. A probe against the pinned library (2026-09-26) found that it emits
modes before content, so content replays in insert mode or with wrapping off; that its tabstop
output leaves the cursor at column 17; that it omits trailing blank rows while restoring the cursor
absolutely, so typing after a replay of a cleared screen lands on a history row; that it writes
all 256 palette entries as overrides, so a replayed pane stops following a theme switch; and that
its C API formats only the active screen, so a pane attached while a program is on the alternate
screen shows an empty main screen when the program exits.

The replay is, in order:

1. a reset;
2. the main screen and its history, formatted with modes, tabstops and palette off and soft wraps
   unwrapped, followed by enough newlines that the active area lines up with the original;
3. if the alternate screen is active, the switch to it and its content;
4. terminal modes, scrolling region, tabstops (followed by a carriage return), charsets, the
   current style and hyperlink;
5. only the palette entries and OSC 10/11/12 colors a program changed from their defaults;
6. the cursor position, last.

The daemon registers the bridge at the recorded stream offset while holding the pane's lock, so
no byte between the replay and the live stream is lost or sent twice. When formatting a long
history would hold the lock too long, the daemon copies the terminal under the lock and formats
the copy outside it.

Step 2 needs the main screen while the alternate one is active, which the pinned C API cannot
format. The first task of the replay spike is to pick the route: a field on the formatter's C
options selecting the screen (the Zig formatter already supports it, and an upstream comment asks
for it), sent upstream and carried as a patch on the pin until it lands; or a detour through the
snapshot API. The spike also measures what a replay of deep history costs.

### 6. Input

The core resolves every keystroke against the keymap as it does today (`docs/architecture.md`,
input precedence). An event the keymap does not bind goes two places:

- **To the daemon**, on the core's input connection (section 9), as the structured event libghostty
  carries: key, modifiers, consumed modifiers, text, unshifted codepoint, composing state. The
  daemon encodes it with libghostty-vt's key encoder configured from the pane's modes
  (`ghostty_key_encoder_setopt_from_terminal`) and queues the bytes for the PTY.
- **To the surface**, through `ghostty_surface_key`, so the surface does what Ghostty does on a
  keystroke: scrolls its viewport to the bottom, clears its selection, hides the mouse, resets the
  cursor blink. The bytes the surface encodes are discarded by the bridge, as today. There is still
  one writer to the PTY.

The surface's generated configuration sets `keybind = clear`, so no Ghostty binding fires beneath
Muster's keymap. Muster's keymap offers Ghostty's binding actions in three groups: actions local to
the surface (scrolling, `jump_to_prompt`, `select_all`, search), actions the daemon performs
because they write to the program or change the pane's state (`clear_screen` clears the headless
terminal's history and writes a form feed; `reset`; `text:`, `csi:`, `esc:`), and actions not
offered.

**Mouse and wheel events go to both, always.** The surface scrolls its own viewport or selects,
and its reports are discarded. The daemon decides from the pane's modes what the program gets: a
mouse report, arrow keys for alternate scroll when a program on the alternate screen asked for
none (`less`, `man`, git's pager), or nothing. That is the decision Ghostty's `Surface.zig` makes,
made once, by the side that writes.

**Each pane has one writer thread and one queue**, carrying keystrokes, pastes, `muster pane send`
text and query answers in order. The encoder reads a copy of the pane's input modes that the
reader updates after each chunk, so encoding never waits on a parse or a replay. When the queue is
full, query answers are dropped rather than blocking the reader, which is what prevents the
deadlock where a program floods output containing queries while not reading its input.

**`muster pane send` uses this same path**, so text an agent sends and text a person types are
encoded by the same code against the same modes.

**Paste** goes to the daemon, which fences it for bracketed paste when the pane asked for it. When
the pane has not, and the text contains a newline, the daemon reports the paste as unsafe instead
of writing it, and the shell asks for confirmation as Ghostty does.

**IME gets better.** The surface now knows the cursor, so composition can be drawn inline with
`ghostty_surface_preedit` and the candidate window placed with `ghostty_surface_ime_point`.

### 7. Terminal queries and effects

**The daemon answers every terminal query**: primary and secondary device attributes, cursor
position, XTVERSION, window and cell size in cells and pixels, kitty keyboard, XTGETTCAP, and the
palette queries OSC 4, 10, 11 and 12. It answers whether or not a window is open, so an agent
started while the app is closed sees the same terminal as one started in a window. Palette answers
come from the appearance Muster already derives for libghostty: the core sends the daemon its
palette at connect and again when the appearance changes, and the daemon then tells programs that
asked for color-scheme updates (mode 2031). Pixel sizes come from the bridge with each resize.
Before any app has connected, after a reboot, the daemon answers with the palette and cell size it
persisted.

**Effects are the daemon's and arrive as events**: title, directory (OSC 7), bell, desktop
notifications (OSC 9 and 777), progress (OSC 9;4), and clipboard writes (OSC 52). Attention routing
already consumes daemon events, and now receives these whether or not the pane is on screen. The
shell ignores the surface's own effect callbacks, and refuses its clipboard reads. A clipboard write
is applied according to a Muster setting whose default matches Ghostty's `clipboard-write`
default, allow. libghostty-vt has no clipboard-read effect, so a program asking to read the
clipboard gets no answer.

### 8. Agent detection

Detection answers two questions per pane, which agent is running and what state it is in, with no
hooks. It is a port of herdr's detection at v0.8.0, covering four parts: the manifest evaluator
(`src/detect/manifest.rs`), the state machine that decides when a state is published (`src/pane.rs`
and `src/pane/agent_detection.rs`), how the detection text and the OSC title and progress are
extracted, and process identification (`src/detect/mod.rs` and the macOS and Linux probes). herdr
spends about 4-5k lines on these. Its hook arbitration, plugin authority and remote manifest
catalog are not ported.

**Which agent.** The daemon reads the pane's foreground process group (`proc_pidinfo` and
`KERN_PROCARGS2` on macOS, `/proc` on Linux), prefers the group leader, and unwraps interpreters:
for `node`, `bun`, `python` and shells it walks argv to the script. The executable names an agent
answers to are part of its manifest, so in Muster's port the list of agents is data, where in
herdr it is a compiled enum. A `MUSTER_AGENT` variable in the process's environment overrides the
match.

**What state.** Each agent's manifest is a list of TOML rules, each with a target state, a
priority, a region of the screen and a matcher (substrings, regexes, per-line regexes, nested
all/any/not). Regions are named slices of the bottom of the screen and of the OSC title and
progress. The highest-priority matching rule wins; a known agent with no matching rule is idle; an
unknown process is unknown. The daemon reports four states, working, blocked, idle and unknown,
and Muster derives `done` from seen-ness, as it already does.

**When.** A pane is checked every 500 ms with no agent identified and every 300 ms with one. A
newly identified agent gets three seconds of grace. Working to idle is debounced: an idle that
comes from no rule matching is confirmed by checks 100 ms apart and published only after three in
a row or 700 ms, while a matching idle or blocked rule publishes at once. A pane whose screen has
not changed since its last check is not read again. The headless terminal sets DEC mode 2027, as
herdr's patched libghostty does, so the detection text for a grapheme cluster matches what herdr's
manifests were written against.

**Manifests travel with the app.** The daemon has built-in manifests, and the app sends its own at
connect, versioned by engine version, so a detection fix reaches a daemon the app adopted without
restarting it. A person's overrides in `~/.muster/agent-detection/` win over both.

### 9. The protocol

**Protobuf, length-prefixed, over a Unix socket.** The schema lives in `proto/` beside
`muster.proto`, built by the same generator. Pane bytes travel as `bytes` fields with no base64.

**Three kinds of connection to one socket.** Each begins with a handshake naming the protocol
version each side speaks.

- The **control connection**, one per app per daemon, carries requests, answers and events.
- The **input connection**, one per app per daemon, carries input events. They are never
  acknowledged and never queue behind a large answer on the control connection. Over ssh it is a
  separate channel with its own window.
- A **stream connection**, one per bridge, carries one pane's replay, bytes and credit.

The core's writes to any of them never block its main thread.

**Answers say what happened.** Every mutation answers with one of: done; already so; refused
because the thing named does not exist; refused for another stated reason.

**Events are ordered and numbered, and answers follow their events.** Every event carries a
sequence number. Subscribing returns a snapshot and the sequence number it is current to, then
streams every later event in order, with nothing replayed from before. The events a mutation
produced are delivered before its answer, and the answer names the last of them, so a client
applies events only and uses the answer to learn when its request has taken effect. Agent state for
every pane travels on this one stream. A client that sees a gap resubscribes.

**Requests in the first version**: snapshot and subscribe; create a pane beside another on any of
four sides, or in a new tab, with a ratio, grid, cwd, environment, command and name; close a pane or
a tab; resize, zoom, swap and move panes; set a split ratio; rename a pane or tab; read a pane's
text in pages addressed by absolute row, with no row cap; set the palette, the shell and the
scrollback depth; send manifests; stop. There is no focus request: daemon focus existed for herdr's
own clients, and Muster never routes by it. Configuration arrives over the protocol, so no daemon
reads a Muster config file and `~/.muster/state/herdr.toml` has no successor. Requests are
namespaced by service (`pane.*`, `tab.*`, `session.*`), so a later message service takes a namespace
of its own.

**App and daemon can differ in version.** The app adopts a running daemon whose protocol version it
supports, so an app upgrade does not restart any agent. The version is a major and a minor. A client
talks only to a daemon of its own major, and the daemon refuses the handshake otherwise; the minor
grows when a request or event is added, so a client can tell whether an adopted daemon knows a request
before sending it. A daemon handed a request it does not know answers refused, never misreads it.
Within a major, a field keeps its number and type: `proto/muster_daemon.v<major>.baseline.proto` is the
schema as that major was first published, and a test fails when the schema no longer reads it. Until a
release ships the daemon, an incompatible change replaces the baseline; after that it means a new
major.

### 10. Replacing a running daemon: handoff

A newer app keeps using the daemon already running, so a daemon fix reaches a machine only when
that daemon is replaced. Ending every agent to do that is a cost people will decline, and they will
run old daemons for weeks. So replacing a daemon hands its panes to the new one:

1. the old daemon starts the new one and passes each pane's PTY master over `SCM_RIGHTS`, with the
   pane's persisted fields and a replay of its headless terminal;
2. the new daemon rebuilds each headless terminal by parsing the replay, takes over the socket, and
   tells the old one to exit.

A replay rather than libghostty-vt's snapshot format carries the terminal, because a VT stream
means the same thing to both libghostty versions and the snapshot format has no compatibility
guarantee. The daemon is structured from its first commit so that a pane can be rebuilt from a PTY
master and a replay. Handoff ships before the first daemon update after the cut-over.

A pane's process is not the new daemon's child, so the new daemon learns that it exited from the
PTY closing rather than from `waitpid`.

### 11. Crates

- `muster-daemon`: the binary. Session model, PTYs, streams, persistence, handoff, the socket
  server.
- `muster-daemon-proto`: the generated protocol types, shared by the daemon and its clients.
- `muster-daemon-client`: the client side, used by the seam and the bridge. It replaces
  `muster-herdr`.
- `muster-detect`: agent detection as a pure library. Screen text, title and process facts in, a
  state out. No PTY, no socket.
- `muster-vt`: gains `Send` for its terminal, the formatter (VT and plain text), mode and kitty
  keyboard reads, scrollback limits, the write-back callback for query answers, the effect
  callbacks, the encoders configured from a terminal, and a plain-text screen read that does not
  cost three FFI calls per cell. The daemon links libghostty-vt statically; nothing in the app
  process does, because a static libghostty-vt collides with GhosttyKit there
  (`docs/observations/libghostty-9f9b8d1d.md` section 8).

**The backend seam survives, with one implementation.** `muster-core` keeps its backend-neutral
traits (`BackendChannel`, `PaneChannel` and the event vocabulary), reshaped to the new protocol's
events, and `muster-daemon-client` implements them. The core depends on none of the daemon crates,
and its own tests run without a daemon, as they do today. A message service would be one more
crate, depending on nothing about panes, hosted by `muster-daemon` and never depended on by
`muster-core`.

### 12. Distribution

**The app carries every daemon it can install.** `./dev` cross-compiles `muster-daemon` for
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` with the Zig toolchain the build
already requires, linking libghostty-vt statically and using mimalloc rather than musl's allocator.
The bundle carries both under `Contents/Resources`, outside anything `codesign` treats as code. A
remote install copies the matching binary from the bundle to `~/.muster/daemon/<version>/` on the
far machine and starts it. A remote Mac gets the app's own arm64 daemon. Nothing is downloaded, so
there is no pin and no checksum file, and a remote machine with no internet access can still be
installed to.

### 13. Testing

**Tests run the real daemon, built from the same commit.** `docs/testing.md`'s rule, "Do not fake
the backend. Run a real one.", holds unchanged, and gets cheaper: no download, no pin, and no drift
check against a recorded daemon (the schema check that remains is between versions, section 9).
`crates/muster-harness` is the neutral harness that spawns `muster-daemon` under a scratch
directory, handed the binary's path by the caller - within the daemon's own package that is
`CARGO_BIN_EXE_muster-daemon`, which cargo builds before the tests run. `until` and the
answer-withholding relay moved there from `herdr-harness`. A spawned daemon must answer its first
request within 25 ms, as herdr does today; any slower, and daemon-backed tests would be too slow to
stay in the default gate. The first build answers in about 4 ms.

**Oracles stay external.** Recording Muster's own daemon and judging the client against the
recording would be an oracle the code rewrites for itself. What stays external:

- terminal grids from libghostty-vt, the engine both the daemon and the surface run;
- the kernel's PTY behavior;
- real agents' screens, recorded as detection fixtures from each harness at a named version;
- herdr's own detection tests, ported with the detection code;
- `corpus/herdr-0.8.0`, frozen as the migration reference. The probe's scenarios run against the
  new daemon for every behavior Muster keeps, and every difference is recorded in the corpus with
  its reason, never removed by editing the recording to match.

**The replay has its own oracle.** Feed recorded bytes to terminal A, replay A into terminal B, and
compare grids, cursors, modes, and what happens when both receive the same bytes afterwards. The
cases include, by name: the alternate screen and then its exit; trailing blank rows below history;
insert mode, autowrap off and origin mode; custom tabstops; an untouched palette and then a theme
switch; a program that set OSC 11; a wide character at the last column of a soft-wrapped row. The
formatter probe written for this MIP is the first fixture.

**The headless terminal is fuzzed** with recorded pane output. It parses every pane's untrusted
output in one process, so a crash in it ends every agent on the machine; the optimize mode it is
built with is chosen for that.

**Responsiveness is measured against the floor.** The latency tier launches the daemon the way the
app does, through Launch Services, keeps its bare-PTY row, and gains a daemon row in place of
herdr's. The targets, at one pane and at fifteen:

| measure | target |
|---|---|
| input-to-glyph, median | within 0.5 ms of the bare PTY |
| input-to-glyph, p95 | within 1 ms of the bare PTY, with no second mode |
| bytes on the stream per echoed byte | 1, plus framing |
| keystroke echo on one remote pane while another remote pane floods | within 1 ms of the same echo with no flood, plus network time |
| attach to a painted replay, full screen and 10,000 rows of history | measured by the replay spike, then set |

The tier also gains a flood case: a pane running `cat` on a large file, attached to a bridge that
reads slowly.

### 14. What Muster deletes

- `crates/muster-herdr`, `deps/herdr.pin`, `tools/herdr-probe` once the migration diff is done,
  herdr's license and NOTICE lines, and every herdr step in `./dev` and CI.
- In the core: the blind mode profile (`input/mode_profile.rs`), arrows and paste routed through a
  second API, the server-encoded input queue, the name registry's backend binding, rect-to-tree
  reconstruction, replay rejection, the answer-before-broadcast settling, inferred tab closes, the
  per-pane agent subscriptions, the workaround for herdr's 2 MiB frame cap, the quit-time size
  hand-back, `find` and `viewport` on `BackendChannel`, `find.rs`, the scroll intent, and the
  holding record's wait for a new tab's name.
- In the seam: the find-and-scroll loop and the prompt polling before a pane's command.
- In the bridge: frame decoding, the grid report, parking.
- In the shell: `HerdrLocation.swift`, the `--herdr-*` bridge flags, the scroll path, and the
  selection pinning and click re-driving in `SurfaceView.swift`, which exist only because the
  surface never scrolled.

### 15. Doctrine that changes

- AGENTS.md's non-goal "Not a multiplexer or session daemon - herdr is." becomes: Muster runs a
  session daemon of its own, which exists only to serve Muster and has no user interface of its
  own. It is still not a terminal emulator; libghostty is.
- "Swappable organs" keeps libghostty behind its seams. The daemon is Muster's, so its protocol is
  part of Muster's contract rather than an adapter over somebody else's.
- "Green suite" reads "a real daemon built from the same commit" where it reads "a real,
  version-pinned herdr".
- "One action path" says that find in a window is a view action, performed by Ghostty's search in
  the surface, while a pane's text read by the CLI or an agent comes from the daemon. The two have
  different reach and matching rules.
- `docs/architecture.md`: control plane and data plane, ownership of truth, input precedence (the
  wheel is no longer always an intent), the renderer seam, degradation, durability.
- `docs/glossary.md`: adapter, backend, daemon, devenv container, frame, pane channel, tab.

### Delivery

No release runs both backends, and nothing in the tree switches between them.

1. **Foundations.** All new code, with herdr untouched: `muster-vt`'s additions and the replay
   spike, `muster-detect`, the protocol, the daemon, and the Linux cross-build. The daemon is tested
   on its own. The stage ends with a throwaway vertical slice, not merged into the app's code
   path: one pane in a real surface on the new daemon, with daemon-side input, attach and detach,
   measured in the latency tier. It tests the replay in a real surface, input side effects and
   latency before stage 2 commits to them.
2. **Cut-over.** One integration moves the seam, the bridge, the core's mirror and input path, and
   the shell onto the daemon, swaps the test harness, and deletes herdr in the same change.
3. **Distribution, handoff and doctrine.** The helper app, bundled Linux daemons and remote install,
   `muster daemons` and `./dev --doctor`, handoff, and the documents above.

Main is not released between the start of stage 2 and the end of stage 3. A fix needed in that
window branches from the last release tag.

## Rationale

**The costs are structural, not bugs.** Re-rendering is why typing waits, why modes are invisible,
why the surface has no scrollback and why frames have a size cap. Upstream fixes can shave each
symptom, but herdr re-renders because its own TUI needs a composed screen, and herdr has no reason
to change that for Muster.

**What Muster needs is small, and Muster already built much of it.** Estimated against herdr's
source at v0.8.0, a daemon that does sections 1 to 9 is about 9-13k lines of production Rust,
7-10% of herdr's. Much of the hard part is already in this repo: libghostty-vt's bindings, its key
encoder, the ssh transport, remote install, and the helper app. The detection rules are reusable
as data.

**Passthrough is the only design that makes a pane feel like Ghostty.** Ghostty's search,
scrollback, selection and mouse handling all read the surface's own terminal state. Any design
where the surface does not parse the program's bytes has to rebuild those features on the other
side of a socket, one at a time, and each rebuilt one is a place Muster feels different.

**One writer gives one owner for everything that talks back to the program.** With passthrough the
surface and the daemon both parse every query a program sends. If both answer, a program gets two
answers; if they take turns, a query in flight when a surface detaches gets none. Making the daemon
the only writer removes the race by construction. It also keeps key encoding out of the shell, the
one layer the suite cannot reach, and makes `muster pane send` and typing one path. Feeding the
surface the same events keeps every side effect Ghostty attaches to typing and scrolling, without
giving it a second route to the program.

**Muster's names on the wire remove a race.** A pane's environment must carry its name when the
process starts, and herdr's id arrived only in its answer, after the process had started. A daemon
that takes the name in the creating request needs no translation, and the registry's backend
binding goes.

**Bundling the Linux daemons makes an app and the daemon it installs one tested unit.** The argument
against bundling (`crates/muster-herdr/src/remote.rs`, module header) was about 72 MB of app: four
18 MB herdr assets, most of them for machines nobody attaches to. Two static Linux builds of a much
smaller daemon cost less; the cross-build in stage 1 measures how much. Bundling also removes the
download and the pin. A pin would be circular for a daemon built from this repo: its checksums do
not exist until CI builds the commit that would have to contain them.

**Handoff is designed in because its absence produces stale daemons.** The part of the daemon that
changes most, detection, no longer needs a restart, because manifests travel with the app. Every
other daemon fix does, and with handoff that restart costs nothing.

## Alternatives Considered

**Fork herdr and carry Muster's patches.** Recorded as an open option in `a_26BJbJeZL`. It keeps
herdr's detection and restore for free. Rejected because every cost above comes from herdr's
architecture rather than from missing flags, so a fork keeps the re-render, the one render thread
and the CLI-only stream, and Muster would maintain 126k lines to use about a fifth of them.

**Keep herdr and push the asks upstream.** The original plan (`docs/origin.md`, "The decision"). The
asks are recorded and several are small. Rejected for the same reason: the largest costs come from
re-rendering, which herdr's own TUI needs.

**Own daemon, server-rendered frames like herdr's.** Keeps bandwidth bounded under output floods and
lets a client watch at a different size. Rejected because the surface would still hold no
scrollback, so search, scrolling and selection would stay rebuilt features, and Muster would write a
renderer and diff encoder that passthrough gets from libghostty. Passthrough recovers the flood case
with credit and a screen-only catch-up.

**Surface-native input encoding.** The shell hands unbound keys to `ghostty_surface_key` and lets the
surface's bytes through; the surface encodes against its own modes, which it now sees. Deletes the
most input code, and is what Ghostty itself does. Rejected because the surface and the daemon would
then both answer terminal queries: the daemon must answer while no surface is attached, the
surface's answers and keystrokes arrive on the same stream and cannot be separated, and a query in
flight when a surface detaches is answered by nobody. Section 6 keeps what this option would have
bought, Ghostty's side effects on typing, by feeding the surface every event and discarding its
bytes.

**Keep the herdr adapter for a transition release.** Would let live herdr agents stay visible across
the upgrade. Rejected by the clean-cut decision: it keeps the pin, the harness, the corpus tooling
and the bundle for another release, and makes every change in that window pay for two backends.

**Import live agents through herdr's handoff.** herdr can pass its PTYs to another executable
(`server.live_handoff`, `src/server/handoff.rs`), so a one-time importer could carry running agents
across. Rejected: herdr's handoff manifest is internal and tied to 0.8.0, none of the handoff has
been measured, and macOS permission (TCC) attribution across it is exactly the kind of claim
`docs/observations/macos-26.4.1.md` says to measure first.

**JSON lines, as herdr speaks.** Easier to read in a socket dump. Rejected because pane bytes would
need base64, the types would be written by hand on both sides, and Muster already generates
protobuf in both languages. The run log is where a person reads the traffic.

**Detection written from scratch.** Simpler rules for the few harnesses Muster's users run. Rejected
because the rules are the part that changes weekly: herdr's changelog has about sixty detection
fixes. A manifest only means what the code evaluating it makes it mean, so with herdr's evaluator
and state machine reproduced, herdr's manifest fixes arrive as data, for every harness at once,
which "harness-agnostic" asks for. Fixes to herdr's state machine or process identification still
arrive as code.

**One connection per machine, multiplexing every pane's stream.** Fewer connections. Rejected
because a surface can only take bytes from the command it spawns, so a bridge per pane stays
regardless, and a connection per bridge keeps one pane's backlog from delaying another's.

**Download remote daemons per release, as herdr's are today.** Rejected in favor of bundling, for
the reasons under Rationale.

**Store a grouped tab's region order and weights on every daemon.** Would let a tab reassemble its
arrangement from daemons alone. Rejected because two stores of one arrangement need a conflict rule,
a divider drag while one machine is unreachable would write only one of them, and the window already
owns the arrangement. Membership is the one fact that must survive without Muster's files.

## Consequences & Trade-offs

**The herdr TUI fallback is gone.** `docs/origin.md` counted on it: a rough Muster never blocked a
workday, because herdr's own client worked against the same daemons. Nothing replaces it in the
first version. Under passthrough a terminal client for one pane is little more than the bridge run
in a terminal; Future Directions lists it as `muster pane attach`.

**Muster owns keeping agents detected.** herdr's manifest fixes remain available as data while the
ported code stays compatible. A harness herdr does not cover is Muster's to add.

**Upgrading to the first release with the daemon leaves herdr daemons running.** Their agents keep
working and are reachable with herdr's own client, and the new app does not show them. The release
notes say so and say how to stop them. `muster daemons` and `./dev --doctor` recognize them for that
release.

**The build gains two Linux targets.** `rust-toolchain.toml` adds two musl targets, libghostty-vt is
built for each, and the `--ssh` tier installs a daemon built from the commit under test. The
`corpus-linux` workflow's job, diffing herdr on Linux against herdr on macOS, ends with herdr. The
same diff between the daemon on Linux and on macOS is worth keeping, and needs the daemon built in
that job.

**The corpus changes authority.** The recorded herdr files become the migration reference and then
history.

**Stage 2 is large and indivisible.** The mirror, input path, bridge and shell move together, and the
suite has to move with them. The vertical slice at the end of stage 1 exists to find its surprises
first.

## Future Directions

- **A message service in the same daemon** (`a_2Rtd0Ed0l`): participants, messages, and groups whose
  policy is data. With this daemon, presence is agent state, delivering a message to a pane is pane
  input, and the transport is the forwarded socket already open to every machine.
- **`muster pane attach`** in any terminal, which restores the fallback `docs/origin.md` relied on.
- **Scrollback that survives a daemon restart**, using libghostty-vt's snapshot format once it is
  stable.
- **Watching a pane from two windows**, which passthrough makes possible once a pane can have one
  size owner and several readers.
- **Agent state reported by the agent**, through a hook, beside screen detection.
- **Resuming agents after a daemon restart**, from the session reference an agent reports, instead of
  a bare shell.

## Open Questions

- **Which route to the main screen while the alternate one is active**: an upstream formatter field,
  carried as a patch until it lands, or the snapshot detour. The replay spike decides it first.
- **What a replay of deep history costs.** Fifteen panes attaching at launch, each with megabytes of
  history, may need history capped or streamed after the screen.
- **Size of a detached pane.** It keeps its last size (`a_29ryxUDCY`). Whether a pane should grow to
  a default when nothing is attached is undecided.
- **`TERM_PROGRAM`.** Programs key features on it. Whether a pane should claim `ghostty`, whose
  features it has, or name Muster, is undecided.
- **Which of Ghostty's binding actions fall into which of section 6's three groups.** The groups are
  decided; the full list is worked out in stage 2.

## References

- `docs/origin.md`, "The decision": the three exits, of which this is the second.
- MIP-1, the portable core. MIP-2, Muster's own units.
- `docs/observations/herdr-0.8.0.md`, `docs/observations/libghostty-9f9b8d1d.md`,
  `docs/observations/macos-26.4.1.md`.
- kan `a_2Rtd0Ed0l` (messaging, and first raised this question), `a_26BJbJeZL` (the herdr team, and
  the fork option).
- herdr v0.8.0 source, Apache-2.0: `src/detect/`, `src/pane.rs`, `src/pane/agent_detection.rs`,
  `src/persist/`, `src/server/headless.rs`, `src/server/handoff.rs`, `src/protocol/render_ansi.rs`.
- Ghostty at the pinned commit: `src/Surface.zig` (keystroke side effects, wheel handling),
  `src/terminal/formatter.zig`.
- Prior art, not verified here: shpool (passthrough with a terminal model for restore), zmx (headless
  libghostty-vt for the same purpose).

---

## History
- 2026-09-26 Draft, from four research passes over Muster, herdr v0.8.0 and the pinned libghostty,
  a prose review, and a design review that probed the pinned formatter.
