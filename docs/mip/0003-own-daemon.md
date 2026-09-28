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
and takes an agent's own report of its state through its API, identically on local and remote
machines. It streams each pane to a client, and
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

**Its log is its own, and a run's log follows it.** A daemon outlives the run that started it, so
it never writes into a run's log: that file would stop describing it when the run ended, and grow
or dangle after. It ignores `MUSTER_LOG_FILE`, which it would otherwise inherit from whichever run
started it. It writes a file of its own beside its socket, `~/.muster/daemon/<install>.log`, and
rotates it itself to `<install>.log.1` past 4 MiB, so at most about 8 MiB is on disk; nothing else
holds the file open, which is what makes renaming it enough. `MUSTER_LOG=0` turns it off and
`MUSTER_LOG_LEVEL` sets its level, as for every Muster process. It also keeps its last thousand
records, numbered from 1 for each run of the daemon, and a control connection that sends
`FollowLog` gets them, then every record after, as `LogLine` messages. That is how each run's
single timeline gets the daemon's side, for a daemon on a devenv as much as one on this machine,
over the connection the app already holds. What a person typed is never in it: the daemon logs no
input's content, and does not read `MUSTER_LOG_INPUT`.

Merging the file into a run's log was the alternative. It fails twice: an app that crashes never
merges, and a devenv daemon's file would need a second ssh channel to fetch. A stream alone fails
the other way, recording nothing while no app is attached, which is when a daemon's own failures
are hardest to explain afterwards. So the file is the record, and the stream carries it into the
timeline.

The app's part, at cut-over: start the daemon without `MUSTER_LOG_FILE`. On each connection,
follow the log: from nothing on a daemon run it has not seen (a new `instance`), and after the
last number it holds on a reconnect to the same one. Append each line to the run's log, adding the
daemon's configured name. For a daemon on another machine, stamp each record's `mono_ns` with its
own receipt time and keep the daemon's as another field, since the two machines' monotonic
clocks do not compare. Say in the run's log when `LogFollowed.oldest` shows records it wanted had
already left the daemon's memory; those are only in the daemon's file.

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
state, whether its agent finished something nobody has seen, whether its process is alive, and
what its agent has said about itself.

**An agent's own facts.** The agent in a pane can report how full its context window is, how many
sub-agents it has running, its model, what its session has cost, and up to sixteen other named
values, and the daemon keeps them on the pane's record and publishes each change like any other.
They are the agent's own words, bounded and never read off its screen: a statusline is whatever
its user configured, so reading one would be guessing. A harness reports from inside the pane with
`"$MUSTER_DAEMON" report`, which reaches the daemon that owns the pane with no window involved,
on a machine where the daemon is the only piece of Muster installed. The daemon counts sub-agents
from their starts and stops, since that is what a harness's hooks see, and forgets the facts when
detection sees the pane's agent change or leave. `extras/claude-code/` wires Claude Code to it.

**What a restart needs is persisted, and nothing else.** The daemon writes one versioned file with
an atomic rename, shortly after each change and on shutdown: tabs, trees, ratios, zoom, labels
with their generations, pane names and labels, each pane's current directory (from OSC 7, falling
back to the foreground process's cwd, checked after output), each pane's size in cells and
pixels, and every setting an app gave it, the palette among them. Titles, agent state, an agent's
own facts, commands and process liveness are observations and are never written; an agent's facts
belong to a process that a restart ends. A daemon that starts and finds the file rebuilds every tab
and starts a shell in each pane's recorded directory, keeping every name. It does not re-run
commands: an agent restarted fresh in fifteen panes is not what anyone asked for. Processes and
scrollback do not survive a daemon restart, which is the guarantee herdr gives today
(`docs/architecture.md`, durability).

The file is JSON beside the socket, `~/.muster/daemon/<install>.state.json`, so a person can read
it, although the protocol's enums are numbers in it: a number survives the protocol renaming a
value, where the name would not. A split's axis is a word, and is never renamed. The settings in it are the protocol's own `Settings` message, so a setting added
to the protocol is kept across a restart without anyone remembering to. The format outlives the
code that wrote it, so it changes only by rule: a field is added with a default, never renamed
or retyped - a renamed `Settings` field keeps its old name as an alias - and anything else raises
the format's version, which every later daemon goes on reading. A checked-in file of the first
version, with every setting set, must load as it was written, and such files are never edited: a
setting added later gets a new file holding it, and a test fails until one does. It is written a second after the
first change it covers, whatever changes after, with the session locked only to copy the state
out; the write itself goes to a temporary file that is synced and renamed over the last, so a
crash at any moment leaves the old file or the new one. A write that would change no byte, such as
one after a title changed, is skipped. A daemon that stops, by `stop` or by a signal, writes what
it held before it closed anything, so stopping a daemon is not asking it to forget its tabs; a
write that fell due while it stopped is skipped rather than writing the closed session. Nor is a
state written that the next start would refuse: that would be a bug, and it is logged as one while
the file keeps the last state that was.

A file this daemon cannot use is never replaced. One written by a newer daemon, whose format
version is higher, is refused with an error that says so, and this daemon starts with no tabs and
saves nothing over it. One that does not parse, or holds a state no daemon would have written, is
moved aside to `<file>.corrupt-<seconds>` with a warning, and the daemon starts empty. Neither stops
the daemon from starting. Restoring runs once the socket is served, one tab at a time, so a
directory on a hung mount cannot keep the daemon from answering; a client that connects meanwhile
sees the tabs arrive as events, and its snapshot says `restoring` until a `restored` event names
what did not come back. The app waits for that before it decides a tab of its grouping file is
gone or makes a pane under a saved name, since a name a client takes first is not restored.
Nothing is written until every saved tab is back, so a crash or a stop while restoring loses
nothing. A directory that no longer exists starts its pane's shell at home, as does one that has
not said whether it exists within two seconds, such as one on a hung mount. Every saved directory
is asked about at once, so any number of them on a hung mount cost those two seconds once. A shell
that will not start - one uninstalled since the last run, a directory it may not enter - is tried
again as the default shell, in the same directory and then at home. What still does not come back, the next
write leaves out, so the file as it was is first copied to `<file>.unrestored-<seconds>` and the
daemon's log names it; if the copy fails, the daemon saves nothing that run rather than lose the
only record, and `restored` says so. It says so too if restoring fails partway, which is a bug,
so that `restored` always arrives. A restored pane
has the environment the daemon gives every pane (section 3) and not what its create asked for:
that is chiefly `MUSTER_SOCKET`, which names a window's socket and so a window a restart may well
have outlived. `muster` run in a restored pane therefore reaches no window until the app hands
panes a way to find it again, which `docs/cli/limits.md` says at the cut-over.

### 3. Starting a pane

A create request carries the pane's grid in cells and pixels, computed by the core from the
window's layout, so a program draws its first screen at the size it will be shown. A pane created
where no window shows it gets the size of the pane it was split from.

The daemon starts the user's shell as an interactive login shell. A pane given a command runs it
through that shell and then replaces it with an interactive shell (`$SHELL -l -i -c '<command>;
exec $SHELL -l -i'`), so the command starts immediately with no typed input for a program to
discard, and the pane drops to a shell when the command exits. The command reaches that shell in
`MUSTER_PANE_COMMAND` and runs through `eval`, so a command with an open quote or a trailing backslash
fails as a shell error and still leaves the `exec` to run. In a POSIX shell that is `command eval`:
`eval` is a special builtin there, and dash - `/bin/sh` on Debian and Ubuntu - abandons the rest of
the script after a syntax error inside one. zsh and fish keep a plain `eval`, since they read
`command eval` as a program called `eval`. This removes the prompt polling Muster does today.

A pane's environment is the daemon's own (launchd's minimal environment on the Mac, which the login
shell builds on), less any `GHOSTTY_*` and `VTE_VERSION` it inherited from the terminal it was started
in, plus:

- `TERM=xterm-ghostty`, with the terminfo entry carried by the daemon, on both machines, so no
  host needs Ghostty installed. `TERMINFO_DIRS` names it, first, with an empty entry after it so
  the system database is still searched. `TERMINFO` does not, as it does in Ghostty.app, because
  tic writes into `TERMINFO` when it can, and the daemon's data can be a signed bundle. It is
  unset, replacing one inherited or requested, unless the `sudo` feature is on; then it is the
  pane's `~/.terminfo`, which the daemon gives the entry where it has none, and which Ghostty's
  wrapper carries through sudo's reset environment, so `sudo vim` finds the terminal. With
  `TERMINFO` unset, ncurses reads `~/.terminfo` before `TERMINFO_DIRS`, so an entry there wins
  over the daemon's: on a devenv, the one an `ssh-terminfo` install from an older Muster or
  Ghostty left. The entry changes rarely, and Ghostty users see the same. With `sudo` off, sudo
  drops `TERMINFO_DIRS`, so a root program finds `xterm-ghostty` only where the system database
  or root's own `~/.terminfo` has it; macOS's has not, as in Ghostty with the feature off;
- `COLORTERM=truecolor`;
- `TERM_PROGRAM=ghostty`, because a pane is a Ghostty terminal and programs key features on the
  name; Muster's identity is already in `MUSTER_PANE` and `MUSTER_SOCKET`. `TERM_PROGRAM_VERSION`
  is the pinned Ghostty's declared version and commit (`1.3.2-dev+9f9b8d1d`): Ghostty's own build
  gives one commit different versions depending on the branch it was built from, and those two
  parts are what stay true;
- `MUSTER_PANE` and `MUSTER_SOCKET`;
- `MUSTER_DAEMON` and `MUSTER_DAEMON_SOCKET`, the daemon's own executable and its socket, which
  is how a program in the pane reaches the daemon that owns it (section 2, an agent's own facts).
  The executable is named through a link beside the socket, `<socket stem>.muster-daemon`, which
  each daemon points at itself as it starts to serve, a daemon taking over by handoff included
  (section 10): a pane outlives the daemon that started it, and the directory an older daemon ran
  from can go once none runs from it. Where the link cannot be made, a pane is told the
  executable's own path;
- Ghostty's shell integration for bash, zsh and fish, injected the way Ghostty injects it, so
  prompts carry OSC 133 marks and `jump_to_prompt` and prompt-aware selection work. Its
  `title` feature is on, as in Ghostty. Three more are settings (`[shell]` in
  `docs/configuration.md`): `ssh-terminfo` and `ssh-env`, on where Ghostty leaves them off, whose
  `ssh` wrapper installs the entry on the host it reaches and forwards the terminal's name; and
  `sudo`, off, which makes `TERMINFO` survive sudo but needs a sudoers rule allowing SETENV. That wrapper runs
  `$GHOSTTY_BIN_DIR/ghostty +ssh`, and a pane has no Ghostty, so `GHOSTTY_BIN_DIR` names a
  directory of the daemon's data holding Muster's own `ghostty`, which handles `+ssh` with
  `muster-daemon ssh`, a port of Ghostty's: install the entry in the host's `~/.terminfo` over a
  connection of its own unless the host is remembered as having it, then connect as
  xterm-ghostty, or as xterm-256color where the install failed. An ssh that opens no terminal
  on a host (`-N`, `-f`, `-W`, `-O`, `-G`, `-V`, `-Q`, `-s`) runs as given. `path`, which would put that
  directory on the `PATH`, is off, since it holds no Ghostty. Its `cursor` feature, which sets a bar at
  every prompt, follows the app's `[cursor]`: blinking or steady as `blink` says while no style
  is named, as Ghostty decides from `cursor-style-blink`, and off when a style is named, because
  Muster turns the integration on without being asked and a named shape is what the person
  asked for. A daemon no app has sent a cursor to uses Ghostty's default, a blinking bar, unless
  a previous run's is saved (section 2), which it uses from the start. A command pane's own shell
  runs the command
  without it, and its `exec` hands it to the interactive shell after: zsh's and fish's
  integrations each undo their injection as they load, so the first shell would use it up.

None of the requested environment overrides these. The terminfo entry and the integration scripts
are the daemon's data directory, `muster-daemon-data`, found beside the executable or where
`--data` says; a daemon without a complete one refuses to start. It is a directory rather than bytes
compiled in because Ghostty's bash and zsh scripts are GPLv3, derived from kitty's, and the daemon is
Apache-2.0: shipped as separate files, as Ghostty ships them, they are aggregated with it rather
than combined into it.

The daemon's `GHOSTTY_TERMINAL_OPT_TERMINFO_NAME` matches `TERM`, so XTGETTCAP answers agree with
it.

Closing a pane sends SIGHUP to its shell's process group and to whatever group holds its
terminal's foreground, as a terminal closing does, and SIGKILL to either still running three
seconds later: a program that traps or ignores the hang-up would otherwise outlive its pane for
as long as the machine runs. A daemon that stops waits for those before it exits, since nothing
would kill them after. A group id the kernel reused within those three seconds would be killed
too, which is accepted: ids are handed out in order, and a group lives while any of its
processes do.

A pane taken over in a handoff (section 10) needs one more check, because its shell is not this
daemon's child: whoever adopted it reaps it, and from then on its pid is free for any process.
So its groups are signaled only if the shell still leads the session its terminal belongs to, or
no process has the shell's pid at all - a group left behind by a dead shell keeps its id while any
of its processes live, and the kernel reuses no id still in use. A process holding the shell's pid
without leading the pane's terminal is somebody else's, and gets nothing.

### 4. Output: passthrough

Each pane has a reader thread. Every chunk it reads from the PTY is sent unchanged to the bridge
attached to the pane, then written into the pane's headless libghostty-vt terminal. The bridge
writes the chunk to its stdout, which is the surface's PTY, so the surface's own libghostty parses
exactly the bytes the program wrote.

**Nothing on this path waits for another pane or for a request.** No render loop and no shared
thread. The reader hands each chunk to a queue drained by the stream's own writer thread, which
credit bounds, so a bridge that stops reading costs its own queue and holds the reader for one
grace period at most (below). Requests
and agent detection take the pane's lock briefly, and `pane read` a batch of rows at a time.
Attaching holds it while it formats the replay (section 5), which stalls only that pane's output.
Nothing holding the session lock ever waits on a pane's lock: what a request does to panes'
terminals - hanging one up, applying new settings - runs once the session lock is let go and before
the request is answered, so a long replay never stalls another connection.

**Flow control is by credit.** A bridge acknowledges the bytes it has written to the surface. The
daemon lets a window of unacknowledged output per pane reach the bridge, and when the window is
full the pane's reader waits for credit, for at most a grace period (100 ms) on each read, holding
no lock. Credit that arrives in time lets it go on: the program is slowed to the bridge's pace and
nothing is lost, which is what Ghostty's own reader does when it waits for its parser and what
ssh does when its channel window is full. Without the wait, any burst larger than the window
reached a surface only in part, however fast the bridge was, because four of the reader's reads
fill the window before the first credit can come back: in the vertical slice a bridge crediting
every message at once received 80% of a 3 MB burst, fell behind 5,621 times, and was caught up as
often. With it, the same bridge receives every byte and never falls behind. Credit, counted this
way, also detects a slow reader on the far side of ssh, where the daemon's own queue depth does
not, because sshd and TCP buffer megabytes before the daemon's writes block. It counts output
only, not a replay's bytes: a replay is bounded by the pane's scrollback and comes once per
attach, and counting it would put every large attach straight behind.

The grace is per read, not per episode. One grace per episode - a total the waits add up to,
restored once the bridge caught up - was built and rejected: a bridge slower than its program, a
local surface parsing a large file, waits on every read with its window never draining, so it used
the grace up partway through and lost the middle of the output, where Ghostty would have slowed
the program and shown all of it. Per read, a bridge that keeps crediting holds its program to its
own pace for as long as the output lasts, which is the bar Ghostty and ssh set.

The cost is that a far bridge holds its program to about one window per round trip, so **a bridge
chooses its window when it attaches**. The default is 256 KiB, which suits a bridge on the
daemon's machine; the daemon keeps a request between 64 KiB and 4 MiB, since every byte of a
window can be queued for the bridge at once. A bridge whose daemon is reached over ssh asks for
2 MiB, the window ssh's own channels use, so Muster's flow control is not the tighter of the two.
Measured on 2026-09-27 through an ssh-forwarded socket to a container whose outgoing traffic
`netem` delayed, flooding ten million lines (79 MB) into a surface that read as fast as it came,
with a second pane's echo timed through its bridge meanwhile:

| Delay | Window | Flood | Fell behind | Echo alone, median / p95 | Echo beside it |
|---|---|---|---|---|---|
| 40 ms | 256 KiB | 18.2 s, 4.3 MB/s | 3 | 46 / 52 ms | 66 / 86 ms |
| 40 ms | 1 MiB | 4.4 s, 18 MB/s | 0 | 48 / 147 ms | 45 / 57 ms |
| 40 ms | 2 MiB | 4.9 s, 16 MB/s | 0 | 47 / 52 ms | 44 / 87 ms |
| 150 ms | 256 KiB | 9.3 s | 36 | 158 / 360 ms | 159 / 300 ms |
| 150 ms | 1 MiB | 11.6 s | 4 | 160 / 379 ms | 157 / 293 ms |
| 150 ms | 2 MiB | 5.4 s | 2 | 159 / 165 ms | 160 / 627 ms |

With no delay the same link carried a 21 MB flood at 23 MB/s with either window. The echo columns
come from a machine at a load average of 11 to 15, and from as few as 14 samples beside the
shortest floods, so they say no more than that a larger window does not make the echo beside a
flood worse: at 40 ms the default window made it 20 ms worse, because the flood held the link
four times as long.

**A round trip longer than the grace** changes what a full window does. Credit cannot come back
within 100 ms, so each time the window fills the program stalls for the grace, the bridge falls
behind, and it is caught up once half its window is credited, a round trip later. The program runs
unthrottled while its bridge is behind, which is why at 150 ms the default window finished the
flood faster than 1 MiB, by falling behind 36 times: the surface showed a screen at each catch-up
rather than the output. A 2 MiB window fills eight times less often, so the same flood fell behind
twice.

A bridge still short of room when the grace ends is behind: it is told once, gets no output, and
the reader does not wait on it again until it is caught up. It is caught up once its
acknowledgements free half the window: with the screen only, not its history (section 5), and then
output resumes. Half, so a slow bridge is not flipped between behind and caught up at the window's
edge, each flip a catch-up composed under the pane's lock. Not zero: a bridge may acknowledge in
batches, and one that is behind is sent nothing more to finish its last batch with, so waiting for
every byte would leave its pane blank for good. So a bridge credits in batches of at most half its
window, and one that does is never wedged; one batching more can be left behind for good, holding
less than a batch it will never complete. Bytes that scrolled off while a bridge was behind are in
the daemon, where `muster pane read` reaches them, and not in the surface's scrollback. The wait
ends as soon as anything about the bridge changes - credit, a detach, a takeover, the pane closing
- and a pane with no bridge never waits.

Measured through a forwarded socket to the devenv container at the vertical slice, with no delay: a
pane flooding 5 million lines into a surface that reads 4 KiB a millisecond and stops 250 ms every
MiB fell behind only at those stops, was caught up with the pane's screen each time and ended
showing it exactly, and slowed a second pane's echo by nothing at the median. Its p95 rose 4 ms
(6.4 ms against 2.3 ms alone, at a load average of 14), where the daemon without the wait left the
link idle while its bridge was behind and fell behind 141 times. Every stream over one machine's
ssh connection shares its TCP stream, so a flood's bytes in flight are queued ahead of another
pane's echo.

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

**Kitty graphics pass through and are not replayed.** The daemon stores images so a program that
probes support gets a truthful answer. Its store is Ghostty's own default, 320 MB, and not smaller:
the daemon's answers are the ones a program sees, libghostty evicts the oldest image to make room
and fails only when one image is larger than the whole store, so a smaller store would refuse an
image the surface takes, or forget an id the surface still holds. The bytes are spent only by panes
whose programs send images, which the surface holds too. The daemon refuses the file, temporary-file and
shared-memory transmission media, which name paths on the daemon's machine that a remote surface
cannot read. Programs then fall back to sending images inline. Two costs come with this.
Ghostty's limit is per screen, so a pane can hold 640 MB at worst, on its primary and alternate
screens together. And the surface holds the same images, so the machine does too, twice. If the
app ever exposes Ghostty's `image-storage-limit`, the daemon takes the same value.

**A replay forgets every image, on both sides.** A replay carries no images, so a surface that
attaches, or is caught up, has none. So the daemon's terminal empties its own store whenever it
sends one, on both screens and virtual placements included, and a program that places an image by
id from then on is told it is not there, as a fresh terminal would tell it, and sends it again.
Carrying the images instead would cost up to the whole store per attach - 320 MB of decoded pixels
per screen, a third more as base64, through a 2 MiB window over ssh - and still could not put back
an image placed in scrollback or on the other screen, since the C API reports placements only
within the viewport. A daemon taking over rebuilds its terminals from replays, so it holds none
either.

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

1. a reset, then grapheme clustering (mode 2027) as the original has it, since it decides
   where every later cell lands;
2. the primary screen and its history, always, with soft wraps unwrapped and down to its last
   row, blank rows included, so the receiver's history holds the same number of rows. While
   the alternate screen is active, the primary screen's cursor and pen come with it, because
   entering the alternate screen saved them and leaving it restores them, and so does its
   cursor shape, which is its own;
3. if the alternate screen is active, the mode the program entered it with (1049, 1047 or
   47), the cursor home, and its content;
4. every terminal mode stated outright, after the content so insert mode and wrapping off do
   not apply to it; stated rather than diffed, because the defaults that matter are the
   receiver's. Left out: the screen switches, already made in step 3; 1048, which saves the
   cursor rather than holding a state; DECCOLM, which resizes; synchronized output, which would
   freeze the receiver; and origin mode, which comes in step 5. Then the mouse tracking mode
   and format in effect, set again last, since the bits in order would leave the highest in
   effect. Then the active screen's cursor shape, when a program changed it from a block: no
   formatter writes DECSCUSR, and a shell with Ghostty's integration sets a bar at every prompt.
   Left out otherwise, so the surface keeps the shape it is configured with - except on the
   alternate screen when the primary's shape is not a block, where `CSI 0 q` makes sure the
   primary's does not stay;
5. tabstops, the scrolling region and modifyOtherKeys, then origin mode, which is relative to
   that region;
6. only the palette entries and OSC 10/11/12 colors a program changed, then the title and the
   directory;
7. the cursor, relative to the region under origin mode, re-printing its cell when it is
   waiting to wrap, since no position sequence leaves it waiting;
8. the pen: style, hyperlink, protection, kitty keyboard flags and charsets, last, because the
   re-printed cell carries its own style.

The primary screen while the alternate is active, and state without content, need two
formatters the pinned C API does not expose, although the Zig formatter has both
(`formatter.zig:301-304` asks for them). Muster carries them as a patch on the pin,
`deps/ghostty-patches/0001`, for good and not for upstream: it only adds C API and changes
nothing that exists, and stays small so that a re-pin rebases it. It also carries three formatter
options, each off by default and turned on only by the screen formatter. The formatter writes the
gap before the next text on a row in whatever style the previous text left open, so a background
or underline would bleed across it, and the first option closes the style first. It drops a row
with no text as blank whatever its colors, and the second keeps a painted one, so a band a TUI
painted with no text in it replays painted. It writes OSC 8 only for HTML, and the third writes
it for VT, so text that was a link stays one. And the patch adds getters for what the C API does
not reach: each screen's cursor shape, since DECSCUSR sets only the active screen's, so the replay
states the primary's before entering the alternate screen and the alternate's after; and the
mouse tracking mode and format in effect, which the mode bits cannot say, since setting one
replaces the last without clearing its bit - the replay sets the one in effect last, so a program
that enabled 1002 and then 1000 does not come back reporting motion. The snapshot API was the
alternative, and reaches the primary screen but still cannot emit state without content.

Three things are not replayed, and the oracle pins each as its exact difference so a fix shows up
as a failing case: per-cell DECSCA protection, the kitty keyboard stack beneath its current
flags, and a cursor saved with DECSC. The first would take the formatter writing protection per
cell, as it now writes links, in both paths of its content loop; the other two are not readable
at all. They last until the program next protects those cells, pushes kitty flags or saves the
cursor.
`docs/observations/libghostty-9f9b8d1d.md` section 14 has the evidence.

The daemon registers the bridge at the recorded stream offset while holding the pane's lock, so
no byte between the replay and the live stream is lost or sent twice, and it formats the replay
under that lock. Measured on one pane, composing costs about 0.2 µs per 80-column row, and
parsing it in a fresh headless terminal, standing in for the surface, about the same: 3 ms each for 10,000 rows, 30 ms each for 100,000, twice that
at 200 columns. The lock stalls only the pane being attached, whose surface is waiting for the
replay anyway, and nothing waits on it while holding the session lock (section 4), so no copy of the terminal is taken, and history is not capped: the pane's
scrollback limit already bounds it. A replay can be larger than a frame may be, so it travels in
pieces of 1 MiB that the bridge writes in order.

**In a real surface the replay shows what the pane showed.** The vertical slice drew one pane in
libghostty surfaces through the real bridge: a surface fed live, a fresh one brought there by the
replay, and the second again after more output, on a primary screen of 3,000 styled rows of
history with wide characters, a wide character wrapped from the last column, a scrolling region
under origin mode, insert mode, a pen and a pending wrap, and on the alternate screen above it.
Read back through Ghostty's own screen dump and compared with the daemon's terminal, every row,
every cell's style and the cursor matched in all three, while a control compared across different
states did not. It found one gap: the cursor's shape, which no formatter writes and which is now
stated (step 4).

**A bridge that fell behind is caught up with the screen, keeping its history.** A replay opens
with RIS, which erases the receiver's history, so a catch-up resets instead, piece by piece,
whatever RIS would: the primary screen, the pen, hyperlink, protection and charsets, the cursor's
shape, margins and
the scrolling region, synchronized output, kitty flags and modifyOtherKeys, and every color a
program set. It then erases the screen, and composes the replay's own steps from the active area
alone, stating the title and directory even when they are empty. The screen formatter leaves
history out through one more field on the carried patch, `history`, which adds C API only.
`corpus/conformance/catch_up.json` is its oracle: a receiver left in a stale state and caught up
must agree with the source on every active row and everything else either can be asked, before
and after both receive the same bytes, and keep the history it had.

### 6. Input

The core resolves every keystroke against the keymap as it does today (`docs/architecture.md`,
input precedence). An event the keymap does not bind goes two places:

- **To the daemon**, on the core's input connection (section 9), as the structured event libghostty
  carries: key, modifiers, consumed modifiers, text, unshifted codepoint, composing state, and the
  app's option-as-alt setting, which travels with each key so it can never race the keys it
  applies to. The daemon encodes it with libghostty-vt's key encoder configured from the pane's
  modes (`ghostty_key_encoder_setopt_from_terminal`) and queues the bytes for the PTY.
- **To the surface**, through `ghostty_surface_key`, so the surface does what Ghostty does on a
  keystroke: scrolls its viewport to the bottom, clears its selection, hides the mouse, resets the
  cursor blink. The bytes the surface encodes are discarded by the bridge, as today. There is still
  one writer to the PTY.

The surface's generated configuration sets `keybind = clear`, so no Ghostty binding fires beneath
Muster's keymap. Muster's keymap offers Ghostty's binding actions in three groups: actions local to
the surface (scrolling, `jump_to_prompt`, `select_all`, search), actions the daemon performs
because they write to the program or change the pane's state (`clear_screen`; `reset`, which
resets the headless terminal and the surface and tells the program nothing; `text:`, `csi:`,
`esc:`), and actions not offered. `clear_screen` is Ghostty's own, done to the daemon's terminal
through the carried patch, since it needs the cursor's prompt state from OSC 133 and an erase the
C API does not expose: history goes; at a prompt the screen is scrolled away and the shell sent a
form feed to draw its prompt again; elsewhere the rows above the cursor go, with every kitty
image. The surface is then sent the cleared screen as a replay, since no byte in the stream says
what happened. On the alternate screen Ghostty does nothing and leaves the key to the program.
The app cannot tell which screen a pane is on, and a program can switch screens before the daemon
acts anyway, so the app always sends `Perform{clear_screen}` with the key that asked for it, and
the daemon, finding the alternate screen, encodes that key against the program's modes and sends
it rather than swallow it.

**Mouse and wheel events go to both, always.** The surface scrolls its own viewport or selects,
and its reports are discarded. The daemon decides from the pane's modes what the program gets: a
mouse report, arrow keys for alternate scroll when a program on the alternate screen asked for
none (`less`, `man`, git's pager), or nothing. That is the decision Ghostty's `Surface.zig` makes,
made once, by the side that writes, with Ghostty's counting: a discrete wheel tick is three rows,
and a precise turn moves a row per cell height of pixels. A click with shift held is reported
only to a program that asked for shift-clicks with XTSHIFTESCAPE, which the carried patch reads:
otherwise shift belongs to the surface's selection, as under Ghostty's default
`mouse-shift-capture`. The pane's record carries what the program said, `shift_capture`, so the
app can leave those clicks to the program rather than select with them; the replay states it too.
The app's own `never` and `always` settings are not passed to the daemon.

**Each pane has one writer thread and one queue**, carrying keystrokes, pastes, `muster pane send`
text and query answers in order. The encoder reads a copy of the pane's input modes that the
reader updates after each chunk, so encoding never waits on a parse or a replay. When the queue is
full, query answers are dropped rather than blocking the reader, which is what prevents the
deadlock where a program floods output containing queries while not reading its input.

**`muster pane send` uses this same path**, so text an agent sends and text a person types are
encoded by the same code against the same modes. The text goes as a paste, fenced when the program
asked for bracketed paste and never held, since a program or agent is speaking rather than a
clipboard; Return, when asked, follows as the key. Written raw, a multi-line send into a program
reading bracketed paste would submit at its first newline.

**Paste** goes to the daemon, which fences it for bracketed paste when the pane asked for it. When
the pane has not, and the text contains a newline, the daemon reports the paste as unsafe instead
of writing it, and the shell asks for confirmation as Ghostty does; a paste the person confirmed
comes back marked so, and is written.

A full queue drops a person's input for that pane too, with one warning per stall, rather than
stall the input connection, which carries every pane's input, behind one program that stopped
reading.

**The side effects arrive as assumed.** In the vertical slice, with `keybind = clear` and every
byte the surface wrote discarded by the bridge, a key event given to a surface scrolled up a page
brought its viewport back to the bottom and cleared a selection, and the program saw the key once,
from the daemon. Output alone left the viewport where it was, which is Ghostty's default
(`scroll-to-bottom = keystroke, no-output`), and a wheel event scrolled the surface's own viewport
and reached no program. The bridge has to read what the surface writes, key encodings and query
answers alike, and throw it away: left unread, the terminal's input queue fills and the surface's
writes block.

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
already consumes daemon events, and now receives these whether or not the pane is on screen: a
notification asks for somebody in the program's words, unless the pane runs an agent Muster
recognizes, whose state already asks at the moments it notifies; a bell marks the pane, and
progress is shown with its agent (`architecture.md`, attention routing). The shell ignores the surface's own effect
callbacks, and refuses its clipboard reads. A clipboard write
is applied according to a Muster setting whose default matches Ghostty's `clipboard-write`
default, allow. libghostty-vt has no clipboard-read effect, so a program asking to read the
clipboard gets no answer.

### 8. Agent detection

Detection answers two questions per pane, which agent is running and what state it is in. Where a
harness can say its own state it does, and that outranks the screen (**The agent's own word**,
below); the screen rules are the rest. They are a port of herdr's detection at v0.8.0, covering four parts: the manifest evaluator
(`src/detect/manifest.rs`), the state machine that decides when a state is published (`src/pane.rs`
and `src/pane/agent_detection.rs`), how the detection text and the OSC title and progress are
extracted, and process identification (`src/detect/mod.rs` and the macOS and Linux probes). herdr
spends about 4-5k lines on these. Its hook arbitration, plugin authority and remote manifest
catalog are not ported. Nor are the two agents herdr knew only through its plugins, `omp` and
`mastracode`: with no manifest, one would read idle while it worked, so it stays unknown.

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
unknown process is unknown. The daemon reports four states, working, blocked, idle and unknown.

**Done is the daemon's fact.** Done is idle plus unseen, and only the daemon can know it for
every window at once, or for no window at all: a pane whose agent finished while no Muster was
open used to come back idle. So a pane's record carries `finished_unseen`, set when its agent
stops working or waiting on you, by going idle or by ending, and cleared when the agent works or
waits on you again, or when a window with the keyboard shows the pane and says so with
`PaneRequest.Seen`. The first window to see it clears it for all of them. The daemon keeps it for
as long as it runs and hands it over with the record, and never writes it to the state file,
since a restored pane has no agent to have finished anything. A window showing the pane when the
fact arrives sends `Seen` at once, and draws the pane as seen meanwhile.

**Waiting is the agent's word, not a state.** An agent that ends its turn to wait on work it
started, a gate or a build in the background, reads idle, and has not finished. Only the agent
knows the difference: a background task still running looks the same whether it is being waited on
or was left behind. So before it ends such a turn the agent reports what it is waiting on,
`"$MUSTER_DAEMON" report --waiting "the full gate"`, and the pane's facts carry it. While they do,
`finished_unseen` is not set, and declaring it clears one already set. It lasts until a later
turn of the agent's ends without the agent saying it again, or until a person prompts it. When
the finished work wakes the agent, that turn's end clears the wait, unless the agent declares it
again because something is still running. The turn that declared it keeps it.

Which turn end counts depends on who reports the agent's turns. An agent that reports its own
state ends its own turns: the plugin's `Stop` hook reports idle, and that report settles the wait.
A working report is no turn: a background sub-agent's tool call can report working after the
agent's turn has ended, and so ends no wait. The plugin's `UserPromptSubmit` hook reports the
prompt with an empty wait, which clears it. For an agent with no hooks, detection's reading of a
turn's end (working or waiting on you, then idle) settles the wait instead. A pane whose
declaration this daemon did not see, handed over or restored, keeps its wait through one more turn
end, so the wait lasts a turn too long rather than too short.

One wait still outlives its work: a wait on something that never wakes the agent, a CI run
elsewhere, stands until somebody prompts it, and the pane is not called done meanwhile. The daemon
logs `daemon.agent.turn_ended` at debug with whether the agent had declared a wait, so how often
agents do can be counted from the run log, and `daemon.report.waiting_cleared` with what cleared
one.

**When.** A pane is checked every 500 ms with no agent identified and every 300 ms with one. A
newly identified agent gets three seconds of grace. Working to idle is debounced: an idle that
comes from no rule matching is confirmed by checks 100 ms apart and published only after three in
a row or 700 ms, while a matching idle or blocked rule publishes at once. A pane whose screen has
not changed since its last check is not read again. The headless terminal sets DEC mode 2027, as
herdr's patched libghostty does, so the detection text for a grapheme cluster matches what herdr's
manifests were written against.

**The agent's own word.** A harness update that rewords its screen breaks the rules for it, and
Claude Code's manifest already carries a spinner rule patched for one release. So a harness with a
documented integration point reports its own state, and that wins: Claude Code's hooks run
`"$MUSTER_DAEMON" report --agent claude --state working|blocked|idle` (`extras/claude-code/`),
through the same `Report` request that carries an agent's facts. The report reaches the pane's
reader, which gives it to detection at once, and it counts until one of these:
- a newer report replaces it;
- the pane's agent, identified from its processes rather than its screen, is not the one that
  reported, which covers a harness that exited or crashed. A report that arrives before detection
  has identified its agent waits two seconds for it;
- for working, ten seconds pass with no output from the pane. A working agent animates something,
  and one interrupted mid-turn, which no hook reports, sits still at its prompt. This asks only
  that the screen move, not that a rule match it, so it holds when the rules have broken. While
  a rule that sees a prompt on screen has read blocked for two seconds, or from the moment a
  working report comes if the prompt was already up, the report is set aside, and counts again
  once the prompt goes: one sub-agent can ask permission while another's
  tool calls go on reporting working, and the prompt is still waiting on you;
- for blocked or idle that the rules have read the same way, since the report came or as it came,
  the rules read something else for two seconds. No hook says a prompt went: Esc and a denial run
  none, and an approved tool runs none until it ends. What the rules confirmed and then stopped
  seeing has gone, however much else on the screen still moves. Claude Code's two permission rules
  that read only the dialog under the last horizontal rule rank above its working rules, so the
  rules read a prompt shown while sub-agents animate beside it as blocked;
- for blocked or idle that the rules have never read the same way, the pane produces output in
  each of three seconds running after the report. That is a prompt the rules cannot read, where
  the report is all there is to go on, and a prompt waiting on you sits still. An approved tool
  can run for minutes with the screen moving, and a background task keeps it moving after an idle
  report; either way the rules read that screen instead.

While a report counts, its state is published as it stands, with no startup grace and no idle
debounce, and the pane's record says `state_reported`. The rules still read the screen underneath,
and take over the moment the report stops counting. herdr arbitrated hooks too; nothing of its
arbitration was recorded here beyond its API, and this is written from scratch.

**A report is taken on the pane it names, whoever sent it.** Any process with the pane's
`$MUSTER_PANE` in its environment reports for that pane. A `claude -p` that the agent's Bash tool
starts inherits it, and runs any plugin installed at user level, so its `Stop` reports the pane
idle while the outer agent is mid-turn, and sets `finished_unseen`. A tmux server started in one
pane does the same for every session it later runs. Such a report stands until the outer agent's
next hook, or until the screen has moved for three seconds. Rejecting it would mean reading the
sender's pid off the socket and walking its ancestors to the nearest process that is an agent,
which has to be the pane's own; the process probe reads only a pane's foreground group, so that
check is not built. Starting the nested agent with `MUSTER_DAEMON` unset keeps its hooks quiet.

**Drift is shown, not guessed.** The rules also say when they have stopped reading an agent: for a
minute, the screen changed in at least half the seconds while either no rule matched at all, or
the agent reported working and the rules read every screen as idle. A still screen never counts,
since idle is its right reading. The pane's record then says `screen_unreadable`, and the daemon
logs `daemon.detection.unreadable` once as it starts, naming the agent and what to check. It
clears when a rule reads the screen again, or the agent changes.

Movement is what the agent does, not its echo: a change to the screen within half a second of the
daemon writing input to the pane does not count. So someone typing a long prompt into an agent
whose manifest has no idle rule of its own, such as codex's or gemini's, is not flagged, while a
reworded spinner is: for those manifests idle is whatever no rule matches, and that still counts
as unmatched. Typing into a working agent hides those seconds from drift too, and from a working
report's ten quiet seconds.

**Where it runs.** Each pane's reader thread ticks its pane's detection on its poll's timeout
(section 4), so detection has no thread of its own. A tick reads the terminal under the pane's
lock a piece at a time and probes processes with no lock held, and its publications reach the
session through the publisher like any other report: the pane's `agent` and `agent_state`
change on its record. What a manifest calls the title is the terminal's own; after an agent
changes it reads as empty until the next time a program writes one, even the same text, as
herdr's did. A resize counts as a change to the screen, so an idle agent that does not redraw on
SIGWINCH is still read again.

**Manifests travel with the app.** The daemon has built-in manifests, and the app sends its own at
connect, versioned by engine version, so a detection fix reaches a daemon the app adopted without
restarting it. A person's overrides in `~/.muster/agent-detection/` win over both, and are read
again whenever the app sends manifests. Only the panes whose agent's manifest in use changed start
their detection over, as herdr resets only the agents its catalog updated: starting every pane
over would publish each working agent idle through its grace, then working again, on every app
launch. A pane with no agent starts over whenever anything changed, since it may be running an
agent that only a new manifest names. A reset always ends in a publication of what is true, so a
pane whose agent went with its manifest reads as no agent rather than keeping its last state.

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
text in pages addressed by absolute row, with no row cap but a 4 MiB cap on a page's text, so an
answer never nears the largest message a client accepts, and the answer says how many rows it holds
(row 0 is the oldest row still held, so rows move up once history reaches the scrollback limit:
libghostty does not say how many it has trimmed); set the palette, the shell and the
scrollback depth; send manifests; report what a pane's agent says about itself; follow the
daemon's log (section 1); stop. There is no focus request: daemon focus existed for herdr's
own clients, and Muster never routes by it. Configuration arrives over the protocol, so no daemon
reads a Muster config file and `~/.muster/state/herdr.toml` has no successor. Requests are
namespaced by service (`pane.*`, `tab.*`, `session.*`), so a later message service takes a namespace
of its own.

**App and daemon can differ in version.** The app adopts a running daemon whose protocol version it
supports, so an app upgrade does not restart any agent. The version is a major and a minor. A client
talks only to a daemon of its own major, and the daemon refuses the handshake otherwise; the minor
grows with any change to the schema, so a client can tell whether an adopted daemon knows a request
before sending it. A daemon handed a request it does not know answers refused, never misreads it.
Within a major, a field keeps its number and type: `proto/muster_daemon.v<major>.baseline.proto` is the
schema as the last minor was published, recording which version that was, and a test fails when the
schema no longer reads it or the two versions disagree. The baseline moves forward with every minor
that ships, because it protects only what it holds: a field added in one minor and deleted without
reserving its number could otherwise come back with another type. Until a release ships the daemon, a
change simply replaces the baseline; after that an incompatible change means a new major.

### 10. Replacing a running daemon: handoff

A newer app keeps using the daemon already running, so a daemon fix reaches a machine only when
that daemon is replaced. Ending every agent to do that is a cost people will decline, and they will
run old daemons for weeks. So replacing a daemon hands its panes to the new one, and ends none.

**The request.** `SessionRequest.replace` names the program to start, by default the executable the
running daemon started from, and its data directory, by default whatever the new daemon finds for
itself. It is answered DONE once the new daemon serves the socket, just before the old one exits,
or REFUSED with the reason, and then the old daemon goes on exactly as it was. It is refused while
the daemon is stopping, while it is still restoring its saved tabs (it holds less than it will), and
while another handoff is under way. `muster-daemon replace` asks for it by hand; the app asks when it
finds an older daemon running.

**When the app asks.** Once, when it adopts a daemon at the socket its own install uses - never one
somebody named in the config, which is theirs to replace - and only when that daemon's `daemon_version`
is older than the one the app carries, compared as `major.minor.patch` numbers with anything after a
`-` or `+` ignored. An equal version is left alone, since two development builds share one and
handing over between them at every launch would buy no fix; a newer one is left alone and logged; a
version that does not read is left alone and warned about. The window follows the older daemon at
once, and the request is made on a thread of its own once it does, so the new daemon's first launch
never holds the window: the panes are there throughout, and come back through the ordinary reconnect
when the new daemon serves. On a machine attached over ssh, this build's daemon is installed there
first, because the older daemon is what runs it. A refusal is said once, as a warning in the
window's problems with the daemon's reason, and the older daemon goes on serving; the app asks again
at its next launch, not in a loop.

**A launch first.** Before it touches anything, the old daemon runs the program once with
`--version` and waits up to a minute for it to exit well, logging how long it took as
`daemon.handoff.launched`. A program that cannot start - a bad build, a missing library - is refused
there, with its stderr on the old daemon's. And macOS checks a binary the first time it runs, which
took up to 12.6 s for a fresh copy on a busy machine: inside the exchange that would outlast a
step's ten seconds and fail it late, where here it is only waited for. The minute is the one the app
gives a daemon it launches itself (`muster_daemon_proto::launch::LAUNCH_PATIENCE`), because the first
start of a new daemon after an update is one or the other, and 44 s was measured for it. Nothing is refused while
it waits: panes are made, resized, renamed and closed, and hooks report, as at any other time. The
checks above are made again when it answers, and the daemon is marked as being replaced only then;
a request they refuse is logged as `daemon.handoff.refused`, since nothing was started to fail. A
stop signal that comes meanwhile stops the daemon at once, closing its panes as at any other time,
and the request is usually never answered: its connection closes with the daemon. If the program
answers while the daemon is still closing its panes, the request is refused as the daemon
stopping.

**The exchange.** The old daemon starts the new one in a session of its own, with one end of a socket
pair as descriptor 3 (`--handoff 3`), and they speak `Handoff` frames over it, never over the
daemon's socket. Those messages are in `muster_daemon.proto`, so the baseline's compatibility check
covers them, and any daemon of a protocol major hands to any other of the same major. A frame that
brings descriptors follows a single byte that carries them as `SCM_RIGHTS`. In order:

1. `Offer`, from the old daemon: its protocol, version and pid, and how many panes follow. The new
   one answers `Accept`, or `Refused` with a reason, which it also does at any later step it cannot
   take.
2. `Session`, bringing the listening socket and the lock file: the state as the file holds it
   (section 2), read under the same rules - one in a newer format is refused - the detection
   manifests the app last sent, and whether the old daemon had stopped saving. A new daemon handed
   one that had stops too, since the file it would write over is one the old daemon was keeping.
3. For each pane, `Pane`, bringing its PTY master: the pane's record, its grid, its process's pid,
   and then a replay of its terminal (section 5) in pieces of a megabyte.
4. `Ready`, from the new daemon; `Commit`, from the old; `Serving`, from the new.

The old daemon first refuses any request that changes something, so what it hands over is what it
holds, and stops accepting: a connection made meanwhile waits in the listener's backlog, which the
two daemons share, for whichever serves next. It pauses its persister once a write under way has
finished. Each pane's reader is held at the top of its loop, with every byte it read already in the
terminal; the replay is composed after that, under the pane's lock, and whatever the program writes
from then on waits in the PTY for the new daemon to read. Its input is still written. A bridge's
resize waits too: the replay may already be composed at the old size, and the two daemons share the
PTY, so resizing it then would leave the new daemon's terminal at a size the PTY no longer has. A
failed handoff applies the waiting size; after a successful one the bridge attaches again with its
own.

The new daemon rebuilds each terminal by parsing the replay with whatever the parse asked for thrown
away, because a replay can provoke a reply of its own - setting mode 2033 sends a visibility report -
and nothing the replay provokes belongs on the pane's input. A replay rather than libghostty-vt's
snapshot format carries the terminal, because a VT stream means the same thing to both libghostty
versions and the snapshot format has no compatibility guarantee. Its readers start held, its log
keeps its records out of the log's file - writing them to `<name>.log.handoff` beside it, which the
old daemon writes into the log's file if the handoff fails, so a new daemon that refused or died
still explains itself there - and it neither accepts nor writes the state file. At `Commit` the old
daemon stops writing the log's file and the new one starts, with what it kept, so the file has one
writer at a time and stays in order; the new daemon releases its readers, accepts on the socket it
was handed, arms its persister, and says `Serving`.

**Failure.** Until `Serving`, the old daemon keeps its own copy of every descriptor and has only
paused. A new daemon that refuses, exits, hangs for ten seconds at a step, or says anything
unexpected is killed; the old daemon lets its readers go on, accepts again, resumes its persister,
takes the log's file back, and answers REFUSED, logging `daemon.handoff.failed`. No pane is ended at
any point, and at no point does no daemon hold the panes. A new daemon that fails after `Commit` may
have read some output the old one never sees. Once `Serving` arrives, the old daemon exits without
closing a pane, writing its state or removing the socket - once it has killed whatever of a pane
closed before the handoff still ignored its hang-up, since nothing would after.

**A stop signal waits for the handoff.** A SIGTERM or SIGINT to the old daemon while a handoff runs,
once the panes are being handed over, closes nothing: if the handoff succeeds the old daemon exits as it would have anyway, leaving the new
one serving every pane, and if it fails the old daemon then stops as it was asked. Closing a pane
mid-handoff would end a process the new daemon may already hold, and removing the socket would leave
the new daemon serving nobody. A signal to the new daemon before `Commit` waits, blocked, until it
serves, and then stops it like any daemon.

**Connections are dropped, not carried.** A bridge is told `Detached` with `REPLACED`, a subscriber
hears `Replaced`, and every connection then ends as the old daemon exits. Input the old daemon had
not yet written, a held paste among it, is lost. What the app does across a handoff:

- on `REPLACED`, `Replaced` or the connection ending, connect to the same socket again;
- a Welcome with a new `instance` means a new daemon: subscribe again, from its snapshot;
- a bridge attaches again, and draws from the new daemon's replay;
- follow the log again, from the new daemon's first record.

**A pane's process stays the old daemon's child.** The new daemon cannot `waitpid` it, so it learns
the process ended from the PTY closing, and says so without an exit status. When the old daemon
exits, whoever adopts its children reaps them: launchd on macOS, init or the nearest subreaper on
Linux, as for any orphan, including a daemon started with `setsid` over ssh. The Linux suite runs
under a reaping init for that reason. Each pane's agent detection goes with it: the reader, as it
is held, writes down where detection stands - the agent, the state it published, what is left of a
new agent's grace, an idle not yet believed - and the new daemon goes on from there, so a working
agent stays working rather than being recognized anew and shown idle through the grace.

**Permission prompts (TCC) are charged as before.** The new daemon is the old one's child rather
than a process Launch Services started. Even so, after a handoff tccd charges a request from a pane
that lived through it, or from one the new daemon made, to `dev.amterp.muster.sessions`, the
identity it charged before. Measured in `docs/observations/macos-26.4.1.md`, section 9. What is
not measured is a grant being honored afterwards, which needs a person to give one.

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
  callbacks, the encoders configured from a terminal, a plain-text screen read that does not
  cost three FFI calls per cell, and the replay. The daemon links libghostty-vt statically, by
  building with `MUSTER_VT_LINK=static`, so a daemon copied to a remote machine is one file.
  The app's side links the dylib the bundle ships, so libmuster and the bridge share one copy.
  A static copy would not collide with GhosttyKit there either: libmuster is a cdylib that
  exports only `include/muster.h` (MIP-1), and the duplicate-symbol failure in
  `docs/observations/libghostty-9f9b8d1d.md` section 8 needs both archives linked into one
  image.

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
rust-lld links each against the musl rustup ships with the target and zig compiles mimalloc, so no
Linux toolchain is involved. Stripped, a release build is 2.8 MB for x86_64 and 2.4 MB for aarch64,
about 1.1 MB each compressed; the data directory both share is 120 KB. The bundle carries both
under `Contents/Resources`, outside anything `codesign` treats as code, with the data directory
beside them. A remote install copies the matching binary and the data directory from the bundle to
`~/.muster/daemon/<version>/` on the far machine and starts it. A remote Mac gets the app's own arm64 daemon. Nothing is downloaded, so
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

**The replay has its own oracle**, `crates/muster-vt/tests/vt/replay.rs` over
`corpus/conformance/replay.json`. Feed a case's bytes to terminal A, replay A into terminal B,
and compare everything either can be asked - every row with its styles, links, protection and
wraps, the cursor, every mode, the colors - then feed both the same bytes and compare again. The
cases include, by name: the alternate screen and then its exit; trailing blank rows below history;
insert mode, autowrap off and origin mode; custom tabstops; an untouched palette and then a theme
switch; a program that set OSC 11; a wide character at the last column of a soft-wrapped row;
kitty keyboard flags on both screens. The gaps section 5 lists are cases too, each expecting its
exact difference.

**The headless terminal is fuzzed** with recorded pane output. It parses every pane's untrusted
output in one process, so a crash in it ends every agent on the machine. `muster-vt`'s
`tests/vt/fuzz.rs` mutates herdr's recorded frames and every string the replay and catch-up cases
feed, splices in the sequences where a parser keeps state across bytes or allocates on the
sender's say-so, and feeds the result in random chunks between the resizes, replays, catch-ups,
clears and formats the daemon interleaves with output. It passes when nothing crashes. It runs in
the gate on stable Rust with a fixed seed, 15,000 cases in about 3 s; `MUSTER_FUZZ_SEED` and
`MUSTER_FUZZ_ITERATIONS` run others. Each case has a seed of its own, from the run's seed and its
number, which decides its mutations and how it is run, so a case is its input and its seed alone.
One that crashes is written to `target/fuzz-crash.bin` with its seed in `target/fuzz-crash.seed`,
and the two placed in `corpus/fuzz/` under one name run first from then on, as they crashed. cargo-fuzz was not used because it needs
nightly, and `rust-toolchain.toml` pins stable.

libghostty-vt is built ReleaseFast, not ReleaseSafe. ReleaseSafe keeps Zig's safety checks, so a
bug the parser reaches aborts the daemon instead of running on past it, and muster-perf measured
what that costs, best of two runs each:

| cost | ReleaseFast | ReleaseSafe |
|---|---|---|
| `frame.vt_parse`, ns/byte | 3.72 | 5.02 - 5.21 |
| `vt.replay_compose`, ns/row | 233 - 239 | 297 - 311 |
| `vt.replay_parse`, ns/row | 219 - 224 | 268 - 289 |
| `vt.text_read`, ns/row | 188 - 198 | 240 - 241 |
| `input.encode`, ns/key | 39.3 - 39.4 | 49.3 - 50.6 |

Parsing, which every byte of every pane pays, costs about 35% more. 600,000 fuzz cases against a
ReleaseSafe build tripped no check.

**Responsiveness is measured against the floor.** The latency tier launches the daemon the way the
app does, through Launch Services, keeps its bare-PTY row, and gains a daemon row in place of
herdr's. The targets, at one pane and at fifteen:

| measure | target |
|---|---|
| input-to-glyph, median | within 0.5 ms of the bare PTY |
| input-to-glyph, p95 | within 1 ms of the bare PTY, with no second mode |
| bytes on the stream per echoed byte | 1, plus framing |
| keystroke echo on one remote pane while another remote pane floods | within 1 ms of the same echo with no flood, plus network time |
| attach to a painted replay, full screen and 10,000 rows of history at 200 columns | within 20 ms; composing and parsing measure 10 ms together headless |

The tier also gains a flood case: a pane running `cat` on a large file, attached to a bridge that
reads slowly. The gate holds the structure beneath these numbers without timing anything
(`crates/muster-daemon/tests/daemon/flood.rs`): a pane behind its flood is told once and sent nothing
more, and another pane's echo still comes back through a writer, reader and stream of its own. An
ignored test there prints the echo's latency, alone and beside the flood.

`crates/muster-latency` measures these rows in `./dev --latency`. At the vertical slice, on an
Apple silicon laptop at a load average of 9, the real bridge's echo was 0.08 ms over the bare PTY
at the median and 0.16 ms at p95, with no second mode; 0.15 ms at p95 in a window of fifteen with
the hidden panes attached, and 0.24 ms detached; one byte on the surface per echoed byte, and seven
on the stream, the byte and its framing. An echo beside a local flood was no slower than alone.
herdr's own client measured 1.4 ms and 22.6 ms. The remote row is in section 4.

### 14. What Muster deletes

- `crates/muster-herdr`, `deps/herdr.pin`, `tools/herdr-probe` once the migration diff is done,
  and every herdr step in `./dev` and CI. herdr's license and its NOTICE lines stay:
  `muster-detect` is an Apache-2.0 port of herdr's detection, and the license goes with the
  port. (This line first said they went too, which the port rules out.)
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
  wheel is no longer always an intent), the renderer seam, degradation, durability, and the
  diagnostic log, whose daemon writes a file of its own that each run follows (section 1).
- `docs/glossary.md`: adapter, backend, daemon, devenv container, frame, pane channel, tab.
- `docs/cli/limits.md`: a pane the daemon restored has `$MUSTER_PANE` but no `$MUSTER_SOCKET`
  (section 2), where today herdr's restored pane has neither.

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
- **Resuming agents after a daemon restart**, from the session reference an agent reports, instead of
  a bare shell.

## Open Questions

- **Whether a remote attach should replay less history than the pane holds.** A replay is
  about 2 MB per 10,000 rows at 200 columns, so fifteen deep panes attaching over a slow link
  wait on bandwidth rather than CPU. Locally there is nothing to cap (section 5). The ssh tier
  measures it once a daemon exists.
- **Size of a detached pane.** It keeps its last size (`a_29ryxUDCY`). Whether a pane should grow to
  a default when nothing is attached is undecided.
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
- 2026-09-26 The replay spike: the route to the primary screen is a patch Muster carries on the
  pin, the replay's order and gaps are as section 5 states, the attach target is set, and
  history is not capped.
- 2026-09-26 Streams, input and replies built: credit counts output only, the catch-up and its
  patch field (section 5), the kitty store's size, option-as-alt per key, `pane send` as a
  paste, shift and the wheel as Ghostty has them, `clear_screen` waiting for shell integration,
  and `pane read`'s row numbering.
- 2026-09-27 The Linux daemons and the pane's environment built: `TERM_PROGRAM` decided (section 3),
  the data directory beside the daemon, `command eval` for dash, and the release sizes in section 12.
- 2026-09-27 Detection wired into the daemon (section 8) and agents' own facts added (section 2).
  From the streams review: a bridge behind is caught up once it has room again, `pane read`
  pages stop at 4 MiB, and nothing holding the session lock waits on a pane's (section 4).
- 2026-09-27 The vertical slice: the replay matches in a real surface and now states the cursor's
  shape (section 5); input's side effects on the surface are as assumed (section 6); the bridge on
  the daemon measured against the targets (section 13). It found a burst larger than the window put
  even a bridge that kept up behind: a bridge crediting at once received 80% of a 3 MB burst and was
  behind 5,621 times, so the pane's reader now waits for credit for up to a grace period, and a
  bridge behind is caught up at half its window (section 4).
- 2026-09-27 Persistence built (section 2): the file's place and format, the settings kept as the
  protocol's own message, and what happens to a file from a newer daemon or a damaged one.
- 2026-09-27 The daemon's log decided and built (section 1): a bounded file of its own, followed
  over the control connection into each run's log.
- 2026-09-27 Remote flow control measured with real latency (section 4): the grace stays per
  read, one per episode having been built and rejected, and a bridge chooses its window, 2 MiB
  over ssh.
- 2026-09-27 Handoff built (section 10): the exchange, what each side pauses and when, what a
  failure at any step costs (nothing), and what the app does across one.
