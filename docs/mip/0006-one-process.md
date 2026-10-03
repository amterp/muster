---
mip: 6
title: Every window is a window of one Muster process
status: Draft
kind: Architecture
created: 2026-10-03
decided:
supersedes:
superseded-by:
related: 1, 2, 3
---

# MIP-6: Every window is a window of one Muster process

## Summary

Muster becomes one app process with many windows, which is how a Mac app is expected to
behave: one Dock icon, ⌘Q quits the app, and the Window menu lists every window. Today each
window is its own process (3ca8e4bb). Five pieces of machinery exist only so that separate
processes can agree: the holders record kept under a file lock, requests carried from one
window's socket to another's, activation handed over by pid, pid claims on arrangement files,
and a new process launched for every new window.

The decisions, each argued below:

- **One core session per process, shared by every window.** The daemon connections, the
  mirrors, the ssh masters and the panes' bridge links belong to the process. Each window keeps
  its own state beside them: which tabs it holds, which is on screen, its regions, its chrome.
- **The C interface between shell and core stays as it is, and the window travels in the
  protobuf.** A request can name its window, and an event names the window it is for.
- **One command socket per process.** `$MUSTER_SOCKET` keeps its name and now names the app. A
  request finds its window from what it names, or from the pane it came from.
- **Quitting is not closing.** After a quit, a crash or an upgrade, relaunch reopens every window
  that was open, holding the tabs it held and showing the one it showed. Only a window somebody
  closed stays closed, and it stays listed by name.
- **One process per install per home.** A second launch hands its request to the running app and
  exits. A window process from before this change is asked to quit, and its window reopens in
  the new app.

It lands in stages, and the first changes nothing a person can see: the core learns to hold
several windows while the shell still opens one per process.

## Context / Motivation

### What was asked for

amterp, 2026-10-03 (kan `a_2b6rC0K3Q`): "I think we wanna make all Windows part of one muster
process". This reverses a design that was deliberate. 3ca8e4bb made a window a process so that
"quitting one leaves the others working", and every cross-window mechanism since then was built
on that.

### What a window per process costs a person

The 0.6.0 upgrade is the worked example (kan `a_2KAFWbZBa`). Two windows were open, and `brew
upgrade` quit both. The relaunch brought back one, because a bare launch takes the most recently
written arrangement nobody has claimed, and that is all it does. Nine panes were running the
whole time, seven of them agents, on a daemon no window was showing.

The second window was harder still to find. It had been started with `--home ~/.muster-local`,
a workaround for kan `a_2IZ5TL6DQ`, so its arrangement sat under a `state/windows/` that a launch
from the Dock never reads. Getting it back took reading `StateLocation.swift` and a dead
process's launch log, then running

    open -n /Applications/Muster.app --args --home ~/.muster-local

Nothing was lost: the daemon held every pane and the arrangement file was intact. What was lost
was knowing that the window existed.

This MIP fixes the first half for every window in one home: relaunch reopens all of them. The
second half, a window in another home, is fixed only if the two-home workaround is retired (Open
Questions).

The rest of the cost is the Mac conventions a process per window breaks:

- There is a Dock icon per window, and ⌘-tab lists each window as an app.
- ⌘Q quits one window, and there is no way to quit Muster.
- The Window menu cannot list the other windows, because they belong to other processes.
- Closing a window quits its process (`applicationShouldTerminateAfterLastWindowClosed`), so
  closing and quitting are the same act. That is why nothing can tell a window somebody closed
  from one an upgrade ended.

### What it costs the code

Every window follows the same daemons, but each process follows them on its own. Two windows
attached to a devenv hold two subscriptions to each daemon, two mirrors of each, and two ssh
masters to the devenv.

Everything one window needs to know about another crosses a process boundary:

- **Which window holds a tab** is the holders record, `~/.muster/state/holding/tabs.toml`, read,
  changed and written inside an `flock` (`crates/muster-seam/src/shared_file.rs`). Each window
  watches the record's directory to hear that another one moved a tab.
- **A request about another window's tab** is wrapped in a `Carried` and sent over that window's
  socket (`crates/muster-seam/src/forward.rs`). Whether a window is open is asked by dialing its
  socket.
- **Going to another window's pane** means handing activation to its process by pid
  (`RaiseWindow`).
- **A new window** is a new process: ⌘N runs `NSWorkspace.openApplication` with
  `createsNewApplicationInstance`, and `muster window new` runs `open -n` and polls until a socket
  it has not seen before answers (`crates/muster-cli/src/opening.rs`).
- **Arrangements are claimed** with a `.held` file carrying a pid, so two launches cannot take
  one, and a launch clears claims whose pid has died or been reused
  (`Sources/MusterMac/StateLocation.swift`).

None of this is wrong, and the suite covers it. But each piece exists only because the windows
are in different processes. Each piece's tests also have to fake the other window, because the
core keeps its session in process globals and a test process can therefore hold only one
(`crates/muster-seam/tests/seam/holding.rs`, `carrying.rs`).

### The names record is already gone

The card that prompted this names another mechanism: a shared record of pane names, kept under a
file lock so two processes agree what a pane is called. That went with MIP-3. Pane and tab names
are now minted unique across machines (`crates/muster-core/src/names.rs`) and the daemon keeps
them, so no window has to agree with another about a name. The holders record is the only file
the windows still share under a lock.

## Decision

### 1. One session per process, and state per window

The core keeps one session for the process, holding what every window shares:

- the daemons it follows: each one's follower, mirror, request channel, input connection and ssh
  master
- the panes it has attached and their bridge links, keyed by pane (a pane is in one tab, and a
  tab is in one window)
- the links between daemons that carry messages across machines
- the name minter, per-pane font sizes, when each pane's agent last changed state, and which
  panes' bridges have ended recently
- the configuration: bindings, appearance, daemon settings
- which agents have been seen, and which pane has been told it has the keyboard

Beside that, the core keeps the state of each open window:

- its composition: the tabs it holds, their order, which one is on screen, and the regions of
  that tab
- its chrome: the sidebar and its width, the text size, the frame
- its arrangement file, and the part of that arrangement still waiting for a daemon to answer
- which machines it has asked for a first tab, and a numbered chord it has armed
- the last view and roster the core sent the shell for it
- its history of which pane had the keyboard, walked by the mouse's back and forward buttons

A window's view is computed from the shared mirrors and its own composition, which is
"view = f(daemon state)" (AGENTS.md) with one more argument. Because windows partition the tabs,
building every window's roster costs what one window holding every tab costs today. What grows
with the number of windows is a small fixed cost per window: one arrangement written, and the
new view and roster compared with the last ones sent.

**Attention and pane focus stay app-wide, and the front window feeds them.** "Seen" means a pane
is on screen in the front window while Muster is focused. A program that asked to hear focus
changes has focus when its pane has the keyboard in the front window. If every window fed them,
whichever window published last would win, so on each publish they get only the front window's
panes on screen and the pane with its keyboard. The check that reports a pane whose bridge never
dialed in is app-wide too, but a pane in a background window still needs its bridge, so it gets
every window's panes on screen.

### 2. The window travels in the protobuf

`include/muster.h` stays three functions and a callback. Three fields join `proto/muster.proto`,
each outside its message's oneof so an older CLI or core skips them:

- `Request.window`: the window a request is for, by its name (`window-2`). Empty for a request
  that is about no window, or that leaves the choice to the core.
- `Request.from_pane`: the pane a CLI command is running in, from `$MUSTER_PANE`.
- `Event.window`: the window an event is for. Empty for an event about the whole app:
  appearance, bindings, configuration problems, agent state, attention.

The core resolves a request's window in this order:

1. A request that names a tab or a pane is about the window that holds it. If that window is
   closed, the core asks the shell to reopen it, as going to a closed window's tab does today.
2. Otherwise `Request.window`, if it names a window this process has open.
3. Otherwise the window holding the tab of `from_pane`.
4. Otherwise the front window.

`OpenWindow` gains the arrangement to open and what to show, so the shell can open a second
window in the same session. `Startup` keeps opening the first.

### 3. One command socket per process

The process listens on one socket, `state/command-<pid>.sock` as today, and every window is
reached through it. `$MUSTER_SOCKET` keeps its name and its place in every pane's environment,
and names the app. `--socket` now means which Muster rather than which window, which matters for
a second home or a development build beside the release.

A command in a pane is about that pane's window by rule 3, and a command outside every pane is
about the front window. `muster window` outside every pane lists every window, each with its
tabs under it, as it does today with several processes listening.

A pane made before this change holds the socket of a process that has gone. `muster` already
falls back to sockets in the same directory sharing its name up to the pid
(`crates/muster-cli/src/dial.rs`, `siblings`), and that reaches the new app.

On a devenv, a pane is told `window-<install>-<name>.sock`, a reverse forward that the app's ssh
master to that devenv carries back. The name is minted once per process today, so it becomes one
per app per devenv with no change.

### 4. Quitting is not closing

With one process, ⌘Q ends every window at once. That costs no agent, because the daemon owns
the PTYs, but it costs the windows unless relaunch brings each of them back. So:

- **Relaunch reopens every window that was open when Muster quit**, each holding the tabs it
  held and showing the one it showed.
- **A window somebody closed stays closed**: the red button, or a new Close Window menu item
  whose default chord is settled in stage 3. It keeps its tabs and its agents keep running, as a
  closed window does today.
- **Closing the last window is a quit.** The app quits, as it does today, and that window comes
  back on relaunch. Otherwise closing the only window would leave a relaunch with nothing to
  open.
- **A crash and an upgrade count as a quit.** Nobody chose to close anything in either.

The holders record already has a row per window, carrying its pid and socket while it is open,
and it is written as windows open and close rather than on the way out. Quitting no longer marks
a window closed; only closing it does. A launch then opens every window whose row says open. A
crash and `kill -9` need nothing extra, because nothing had marked those windows closed.

**A closed window is reachable by name.** The Window menu gains a Reopen submenu listing each
closed window by name, with its tabs. `muster window list --closed` lists them, and `muster
window reopen NAME` brings one back. A bare `muster window reopen` keeps today's meaning, the
most recently closed.

### 5. One process per install per home

An install is one copy of Muster, and each runs its own daemon at
`~/.muster/daemon/<install>.sock`: a development build and the release are two installs. They
share `~/.muster/state` unless `MUSTER_HOME` says otherwise. So the unit that gets one process is
the install within a home, not the home alone.

The process holds `state/app-<install>.lock` for as long as it runs, and writes its socket's path
beside it. A second launch of the same install and home finds the lock held, sends what it was
launched to do (a fresh window, a named one, a tab to show) to the running app as an
`OpenWindow`, and exits. `open -n` and a `muster window new` from before this change both end as
a new window of the running app. A Dock click with the app running never starts a process; the
app answers it by bringing a window forward, or opening one if none is open.

Two installs never share a daemon, so their windows never share a tab. Each install's
arrangements and holders record move under `state/<install>/`, so a development build stops
opening the release's arrangements. The release adopts the existing `state/windows/` and
`state/holding/` on its first launch after this lands, once any process from before this change
has quit (section 6), so nothing is still writing to the old place.

### 6. A window process from before this change

When the new app launches, older window processes may still be running: a manual launch while
they were open, or an upgrade path that did not quit them. The new app finds `.held` claims whose
pid is alive and whose process has the same bundle path as its own (`NSRunningApplication`'s
`bundleURL`; after `brew upgrade` the path is unchanged and the version differs). It asks each
of them to quit with `NSRunningApplication.terminate()`, the same request ⌘Q makes, waits for
their claims to clear, and opens those arrangements itself. A claim held by a different bundle,
such as another checkout's development build, is left alone.

Quitting one of those processes costs nothing an ordinary quit does not. Its arrangement is
written as it settles, so the file is current, and the daemon keeps every pane.

### 7. What goes away

Once every window of an install is in one process, no window needs to reach another across a
process boundary:

- Carrying a request to another window's socket (`forward.rs`, `Carried`). A request about
  another window's tab is answered from that window's state in the same session.
- `RaiseWindow { pid }`. Going to another window's pane brings that window to the front.
- The `flock` around the holders record. One process writes it, so it becomes a plain file kept
  for the next launch.
- `.held` claims. The app lock replaces them.
- Launching a process from `muster window new` and `reopen`. Both become requests to the running
  app, which launch it first only when nothing is listening.

The CLI keeps asking several sockets when it finds several, since two installs can listen in one
home. What stops is one install's windows answering from separate sockets. The field number of
`Carried` stays reserved rather than reused.

## Rationale

**One session, because every window follows the same daemons.** Two windows following one daemon
hold two mirrors of it, and the holders lock, carrying and the socket dial exist because those
windows cannot reach each other's state. With one session, "which window holds this tab" is a map
lookup, and moving a tab between windows changes two compositions under one lock.

**The window in the protobuf, because the protobuf is the contract.** A window handle in the C
ABI would be a second way to say something a protobuf field can say, and every new request would
have to remember to pass it. As a field, it is one more thing a request can carry, a test can
set, and the CLI and the shell set the same way.

**One socket, because the window a pane belongs to can change.** A socket per window inside one
process was possible and would have kept the CLI unchanged. But a tab moves between windows and
its panes keep their environment, so the socket a pane was told names the window it used to be
in. Today carrying corrects for that. Resolving the window from the pane's tab inside the core
gives the right answer with nothing to carry.

**Quit is not close, because one process makes every quit a quit of everything.** Without it,
the 0.6.0 upgrade's lost window becomes every window but one, on every quit.

**Asking older processes to quit, because coexisting would keep everything section 7 removes.**
Keeping the cross-process machinery working beside the in-process one would mean two ways of
holding a tab, two ways of carrying a request, and tests for both, all to serve a transition that
happens once per machine.

## Alternatives Considered

**A core per window, in one process.** Each window keeps a full session of its own, and the seam
grows a handle saying which. This is the smallest change to the core: the session struct stays as
it is and its globals become a map. It was rejected because it keeps every cost under "What it
costs the code". Each window still follows every daemon on its own, still holds its own ssh
master, and still needs the holders record and carrying to agree with the others, only now
through memory instead of a file and a socket.

**One socket per window, in one process.** Keeps `$MUSTER_SOCKET` meaning one window and leaves
the CLI as it is. Rejected for the reason under Rationale: a tab that moved leaves its panes
holding the wrong window's socket, so carrying stays.

**Keep a process per window and fix relaunch alone.** Write which windows were open at quit and
launch a process for each. That fixes the 0.6.0 upgrade losing a window and nothing else on the
list: still a Dock icon per window, still no way to quit the app, still every mechanism under
"What it costs the code". Built first, it would be thrown away once one process lands, since
launching N processes at startup is exactly what one process removes.

**AppKit's window restoration** (`NSWindowRestoration`). The platform's own way to reopen windows
on relaunch. Rejected because what a Muster window is lives in the core, not in AppKit: its
composition, which `muster window` reads and the CLI drives. Restoration would be a second record
of which windows exist, kept by the shell where no test reaches it. It also does not run after ⌘Q
while the system setting "Close windows when quitting an application" is on, so it would answer
a crash and not a quit.

**A separate file of windows open at quit.** Written on the way out. Rejected because a crash and
`kill -9` never reach the way out, and those are the cases where nobody chose to close anything.

**Coexist with pre-change processes.** Treat a running older window as one more window reached
over its socket. Rejected under Rationale: it keeps every piece of section 7 alive indefinitely.

**One process per home.** Simpler to state, and wrong while a development build and the release
share a home. A development build launched beside the release would hand its window to the
release app, which follows a different daemon.

## Consequences & Trade-offs

**Quit ends every window.** That is what a Mac app does. Relaunch brings them all back.

**A crash ends every window.** Today a crash in one window costs only that window. With one
process a fault in the shell or core takes them all down. No agent is lost, since the daemon owns
the PTYs, and relaunch reopens every window, but a shell bug now reaches every window instead of
one. The suite is what guards against that, and the run log is how a crash gets diagnosed.

**Closing a window no longer ends a process.** The app keeps running while any window is open,
and ⌘Q quits it.

**Windows share one main thread.** The shell routes each event by `Event.window`, and an app-wide
event goes to every window. A slow window's main-thread work now delays the others, which it
could not do as a separate process.

**Tests can hold two real windows.** A seam test opens two windows in one session against a real
daemon, where today the other window is a stand-in socket and a hand-written row in the holders
record.

**Compatibility.**

- `$MUSTER_SOCKET` and `--socket` keep their names, and name the app rather than one window.
- A CLI from before this change sends no `window` or `from_pane`. Its requests resolve to the
  front window, except one naming a tab or pane, which still reaches the window holding it.
- A pane holding a dead socket from before reaches the new app through the existing sibling
  fallback.
- `muster window list` keeps its output, and gains `--closed`.
- `muster window --json` keeps `other_windows[]`. Its `pid` becomes the same for every open
  window. So `muster tab move --window` takes a window's name rather than a pid; a pid is still
  accepted and means the front window of that process.

## How it lands

Each stage leaves `main` working.

1. **The core holds a map of windows.** One session holding what windows share, and state per
   window. The protobuf fields from section 2. `OpenWindow` can open a second window in the same
   session, which only the tests use. The shell still opens one window per process and sets no
   window, so nothing a person sees changes. Seam tests with two windows against a real daemon.
2. **Requests find their window.** The four resolution rules, Move Tab and a request about
   another window's tab handled inside the session when both windows are in it, and the CLI
   sending `from_pane`. Still one window per process in use.
3. **The shell opens every window.** Many NSWindows, with menus aimed through the responder chain
   rather than at one window, and events routed by `Event.window`. Close Window apart from quit,
   relaunch reopening every window open at quit, the app lock, one socket, arrangements under
   `state/<install>/`, pre-change processes asked to quit, and ⌘N, `muster window new` and
   `reopen` opening windows in the running app. This is the stage a person notices, and the
   largest. If it has to split, it splits after the windows and the menus, with relaunch and the
   app lock second.
4. **Removals, and the rest of the CLI and docs.** Section 7, the Reopen submenu, `muster window
   list --closed`, and the docs: README "More than one window", `docs/cli/window.md`,
   `docs/cli/overview.md` "Which window", architecture.md "One action path" and "Durability".

## Open Questions

- **Is the two-home workaround still needed?** It came from `a_2IZ5TL6DQ` and `a_2I6h18OU6`,
  which 0.6.0's tab model fixed. If nothing needs it, the second-home case in `a_2KAFWbZBa` stops
  arising. If something does, a second home is a second Muster process under this MIP, and
  `muster window list` should at least say that other homes exist. Check before stage 3.
- **Does the CLI need a flag naming a window?** Rules 1 and 3 cover a command that names a tab or
  pane and a command run in a pane. What is left is a command outside every pane that names
  nothing, such as `muster pane new` from a plain terminal, which goes to the front window. A
  global `--window` would collide with `muster tab move --window`, which already names a move's
  destination. Lean: add nothing until somebody needs it.
- **Where do windows open on relaunch when the display they shared is gone?** Each window's frame
  is fitted to the screens on its own today, so two windows fitted onto one remaining screen may
  land on top of each other. Settle in stage 3.

## References

- kan `a_2b6rC0K3Q` (this), `a_2KAFWbZBa` (quit is not close), `a_2Mhi0EZlv` (a tab belongs to one
  window), `a_2IZ5TL6DQ` (the two-home workaround).
- 3ca8e4bb, "a second window, and one set of names between them": the decision this reverses.
- MIP-2: the window as a unit with an arrangement of its own.
- MIP-3: the daemon that made the names record unnecessary.
- `docs/architecture.md`, "One action path" and "Durability".

---

## History
- 2026-10-03 Draft
