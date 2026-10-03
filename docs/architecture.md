# Architecture

How Muster is shaped, and why. This document constrains the load-bearing structure: layers, seams, ownership of
truth, and how traffic flows. It deliberately does not choose module layouts, type names, wire framings, or
concurrency mechanisms - implementing agents research those at build time and will make better local decisions than
this document could. When a decision here proves wrong in practice, change it: update this document in the same
change, and put the why in the commit.

The desiderata in `AGENTS.md` are the requirements. `origin.md` holds the founding history and `glossary.md` the
terms; large decisions are recorded as MIPs in `mip/`, routine rationale lives in commit messages, and open
questions live on the kan board. This document is the bridge between the desiderata and code.

## The shape

    ┌─ native shell (per-OS; macOS first) ────────────────────────────────┐
    │  windows · split chrome · key capture · sidebar · notifications     │
    │  renderer surfaces (libghostty), one per pane                       │
    └──────┬───────────────────────────────────────────────▲──────────────┘
           │ key/mouse/scroll/resize events, actions       │ pane output
    ┌──────▼───────────────────────────────┐               │ (data plane:
    │  core (headless view-model)          │               │  output only,
    │  mirror · dispatcher · keymap ·      │               │  one stream
    │  attention · config                  │               │  per pane)
    └──────┬───────────────────────────────┘               │
           │ Muster vocabulary (control plane:             │
           │ events, state, intents, input)                │
    ┌──────▼───────────────────────────────────────────────┴──────────────┐
    │  backend adapter (muster-daemon-client)                             │
    └──────▲──────────────────────────────▲───────────────────────────────┘
           │                              │
     muster-daemon (local)         muster-daemon (remote, SSH)

Three layers, two seams.

**Native shell** (per-OS; macOS first). Windows, split chrome, key capture, sidebar, notifications, and one renderer
surface per visible pane. Deliberately thin: it wires OS events into the core and renders what the core says,
nothing more. Its failure modes should be wiring failures - that is what makes it testable by a small smoke layer
(see `testing.md`).

The macOS shell is AppKit, and SwiftUI appears in one file: the find bar, which was ported from Ghostty's rather than
rebuilt. **Either toolkit is a fair choice for a given piece of the window** - this layer exists to feel like the
platform, and both of them are the platform. A second toolkit in one window does cost integration work, and the find
bar is the record of what it cost: its layer background is forced clear because a GPU surface sits
underneath it, it carries a workaround for a SwiftUI crash on macOS 15, and it reaches its own text field through a
`NotificationCenter` hop because `@FocusState` cannot be read from outside a view body. Weigh that bill when adding
the next SwiftUI view. It is a cost, not a line nobody may cross.

**Core** (headless, OS-free, Rust). The view-model and the only place decisions live: the mirror of each daemon's
state, the action dispatcher, keymap policy, attention routing, configuration. The core never touches an OS API, a
real clock, or a socket directly - those arrive through injected edges.

Rust rather than the shell's language, decided in `mip/0001-portable-core.md`. The short version: a boundary
between shell and core has to exist the moment a second platform appears, because a Linux or Windows shell will not
be Swift either. Putting a portable core on one side of it means macOS pays a well-supported FFI direction and every
other platform pays nothing.

**Backend adapter** (`muster-daemon-client`). Translates the Muster vocabulary to muster-daemon's protocol, and is the
one place the core's requests become that protocol; the core never sees it. The daemon has been Muster's own since
MIP-3 replaced herdr, so its protocol is part of Muster's contract, and a second backend would be a second adapter.

**Muster ships its daemon and runs it, and talks to no other.** It is what makes the rest of this document mean
anything. The suite runs against the daemon built from the same commit, so a Muster attached to some other daemon is a
Muster whose every behavior is unverified. So the app finds its daemon beside its own executable rather than on PATH,
and it listens on a socket named for the install, `~/.muster/daemon/<install>.sock`: a development build never adopts
the release daemon, two checkouts never adopt each other's, and tests start one on a socket of their own. A release
does adopt the release daemon an earlier version started, if it speaks that daemon's protocol, which is what keeps an
upgrade from ending every agent (MIP-3, sections 1 and 9). A stranger is then not something to detect; it is something
that cannot arise.

**How it runs it is a permissions decision rather than a packaging one.** In a bundle the daemon is a helper
application of its own, `Contents/Library/MusterSessions.app`, and Muster starts it through Launch Services rather
than spawning it. macOS charges a pane's protected request to the *responsible* process; a spawned child inherits its
spawner's, and only a process Launch Services started is its own. Spawned, the daemon is charged to Muster until
Muster exits and to nothing nameable after that, so a permission behaves one way before the first relaunch and
another way after it. Opened, it is charged to its own bundle for as long as it lives - which, since it is never
stopped, is across every relaunch - and that bundle carries the same usage strings the app's does, so macOS has both
a name for the prompt and a reason to put in it. Measured with the arrangements side by side in
`observations/macos-26.4.1.md`. A plain `swift build` stages a bare binary and keeps the spawn, which is also what
every test uses, so both paths have to stay correct rather than one replacing the other.

Started, never stopped, because sessions outliving the app is the point. Naming a `socket` in Muster's config file is
the way to ask for a particular daemon on purpose; nothing in the environment chooses one, so the
`MUSTER_DAEMON_SOCKET` every pane carries cannot quietly redirect a window opened from inside a pane.

**The daemon's environment is built, not inherited, and that follows from it being started and never stopped.**
Whatever shell launched Muster is a moment; the daemon is not, and every pane's program is its child - so anything
carried in at birth becomes state handed to every agent, on a process that outlives the app that carried it. An
allowlist rather than a denylist, because a denylist is wrong until somebody notices, and the way you notice here is
an agent behaving strangely for reasons nothing on screen explains. It stays short because a pane runs a shell and a
shell rebuilds its own world from the user's files: what has to survive is only what a shell cannot work out for
itself - where home is, what to run, the machine's locale, and the person's own ssh agent. The launch says in the run
log what it carried and what it dropped, by name and never by value.

Not hypothetical. Launching Muster from inside a coding-agent session put that session's markers and its messaging
credentials into the daemon and from there into every pane, where a fresh agent read them, believed it was a child of
another session, and stopped saving its transcript - and it persisted after that Muster had quit.

**An allowlist can only carry what exists, so a little is supplied.** Muster is meant to be launched from the Dock,
and launchd hands a GUI process `HOME`, `PATH`, `SHELL`, `USER`, `LOGNAME`, `TMPDIR`, `SSH_AUTH_SOCK` and little
else - no locale at all, which is the exact absence `LANG` is on the list to prevent. Nothing looks broken today, and
the reason is a loan rather than an answer: building the renderer derives a locale from the platform and puts it in
the whole process, so the environment Muster reads a moment later has a `LANG` in it that no shell set. That is the
same borrowing as the fonts and colours Muster used to take from a Ghostty config file, and less visible - it depends
on the renderer being built before the daemon is started, and the day a renderer changes, every pane drops to the C
locale in silence. So Muster answers it: the shell reports what the platform says, because only it can ask macOS, and
the core decides whether a daemon gets it - only when nothing in the environment named a locale, since one half
inherited and one half supplied is the split the allowlist already refuses to create. The run log names supplied
variables as a third list beside carried and dropped, so "where did this come from" has an answer.

`TERM` is not one of them, and its absence is the more useful fact. The daemon sets `TERM`, `COLORTERM` and
`TERM_PROGRAM` on every pane itself, because a pane is a Ghostty terminal whatever launched the app, so no pane has
ever seen the daemon's. Dropping it also gives the daemon the same environment whether Muster was started from a
terminal or from the Dock, which never hands it one. When Muster ran herdr, the daemon's `TERM` also fed herdr's
host-terminal detection, and a Muster launched from Ghostty had its daemon posting notifications as Ghostty, to a
terminal that was not there.

**The daemon's settings are Muster's, and travel over its protocol rather than in a file.** A daemon of Muster's own
that took its settings from a stranger's config file would not be a daemon of Muster's own: when Muster ran herdr, a
`default_shell` somebody set for their own terminal decided what every Muster pane ran. So the shell a pane runs, how
much scrollback it keeps, its palette, its cursor, whether a program may write the clipboard, and how far a wheel
notch scrolls are read from Muster's file and sent as requests on the control connection - each one when it changes,
and all of them on every connect (MIP-3, section 9). No daemon reads a config file, so there is nothing for a pane to
inherit and nothing to hand back. The daemon saves what it was sent with its tabs, so one that restarts with no window
attached starts panes the way a window last asked (MIP-3, section 2).

**The guarantee reaches the far machine too, by putting the daemon there.** For a while a remote daemon was
whatever somebody had installed, and a window's two halves could run different versions with nothing saying so. On
attach Muster asks that machine what it is with `uname -sm`, and sends it the daemon this app carries for it over the
master it already holds open, to `~/.muster/daemon/<version>/`: a static Linux build for x86_64 or aarch64 from
`Contents/Resources/daemons/`, or for a remote Mac the app's own daemon with the libghostty-vt it links. The daemon
and its data directory travel as one archive, and a SHA-256 of that archive stays beside them, so a machine holding
any other build - two development builds share a version - gets this one instead. Then the same sequence as here
follows: adopt a daemon already answering, and otherwise start this one, in a session of its own.

**Nothing is downloaded, by either machine.** A devenv is often a container or a build box with no route out, and
the app already holds every daemon it can install: two stripped static builds of 5 and 6 MB, where bundling
the four herdr assets a download once fetched would have cost 72 MB. So there is no pin and no checksum file, and an
app and the daemon it installs are one tested unit - which a pin could not make them, since a daemon built from this
repository has no checksum until the commit that would record it is built.

One consequence worth stating: a `socket` named in the config file still attaches whatever is listening there, on
either machine - that is the deliberate way out of the whole arrangement, and it
would be no escape hatch if a remote one behaved differently from a local one.

**Reaching a remote daemon is a transport concern and stops there.** A remote muster-daemon speaks the same socket
protocol a local one does, so an SSH master forwards that socket onto a path on this machine and the adapter is handed
a path like any other - the control connection, the input connection and every pane's stream are unchanged. The data
plane rides the same forward: a bridge dials the forwarded socket and asks for its pane's stream, so a remote pane
costs no ssh exec of its own and a remote bridge restarts as cheaply as a local one (MIP-3, section 4). A tunnel that
drops is reopened onto the same path, so recovery is the adapter's ordinary reconnect rather than a mechanism of its
own.

The master is opened before the daemon exists, which is what lets everything after it ask "does it answer" through
the forwarded path rather than through a second mechanism. Measured against the devenv: ssh binds the near end when
it connects and reaches the far one per connection, so a remote socket that is not there yet costs nothing until
something dials it.

**Reopening it is a connection that answered, not a process that started**, and the difference is the whole of
what made a plugged-in laptop unusable. Spawning ssh succeeds whenever ssh is on the PATH, so a host that is
unreachable, a key that wants a passphrase, and a forward something else is holding all come back as success with a
process about to die - and calling that a reopen produced 97 of them in two minutes while nothing reconnected
(kan a_2IRdZK6Un). A reopen is announced when the forwarded socket is bound *and* a command has come back over the
master's own control path, which is transport proving itself and needs no vocabulary from any daemon. The retry
interval escalates on the same rule the bridge policy uses - a run of failures ends when a connection has held for
half a minute, not when one came up - so nothing about a process existing can reset it
(`crates/muster-core/src/reconnect.rs`). It reports and keeps trying rather than giving up, which is where it
deliberately differs: a bridge that gives up costs one pane, and a tunnel that gave up costs every pane on that
machine until the app is relaunched, which is the failure this whole arrangement exists to prevent.

**A master is addressed by its control path, not by its pid.** The child Muster spawned is the master only while ssh
stays in the foreground, and `ControlPersist` in somebody's own ssh config makes `ssh -N -M` fork once it has
authenticated - the process Muster watches exits and a different one carries the forward. Believing that child
declared a working connection down 250ms after every confirmed reopen for thirteen minutes, each false down
unlinking both paths from under a master that was carrying every pane's traffic; and it left eighteen authenticated
connections running on the far machine after one quit (kan a_2J1KZ9FbM, a_2J1KYPWhZ). So `ControlPersist=no` joins
the options a config cannot override, the supervisor asks `ssh -O check -S <path>` rather than the child, and a
master is ended with `ssh -O exit -S <path>` - on the reopen path as well as at `Drop`, because the reopen path is
where they accumulate. Killing the child remains the fallback for a master that never answered. Both the option and
the question are kept: the first holds a daemon to one ssh process rather than two, and the second is right whatever
a future ssh does about forking.

**A check that times out is asked again before the master is ended.** `-O check` is answered on this machine and never
touches the network, and a master whose connection dies exits once `ServerAlive` gives up, after which the check fails
at once. So a check that fails at once ends the master, and one that times out is a slow or wedged master, or a
checking ssh slow to start on a loaded machine: it is logged as `tunnel.slow` and asked again on the next poll, and
only a second timeout ends it. Ending on the first cost seven panes their bridges on a master that was working
(kan a_2YQCqiInL).

**Only Muster's master can hold Muster's control path.** Every ssh that runs a command over a master pins
`ControlMaster=no` and `ControlPersist=no`, because under a personal config's `ControlMaster auto` a client that finds
nothing answering its `-S` path becomes the master for it, with no forward, and `-O check` then calls a tunnel healthy
while every request is refused (kan a_2NnC4pyPm). The bridges run no ssh at all; they dial the forwarded socket.

**No master outlives the Muster that started it.** Quitting ends every master, after stopping the daemons if that
was asked, because the process exits without dropping its session and nothing else would; a master left running keeps
the quit window's socket answering on the devenv (kan a_2YAdjRtMB). A crash cannot end them, so the first tunnel a
Muster opens also ends every master in `$TMPDIR` whose `muster-<pid>-<daemon>.ctl` names a pid that no longer runs. A
pid that runs is left alone whatever it is, since it may be a second window on this Mac with a master to the same host.

## The vocabulary

The backend contract speaks Muster's terms, not any backend's. Nouns: backend session (one daemon connection), tab
(the unit that owns one pane tree), layout (a tab's tree, as proportions rather than cells), pane, pane channel (the
output stream feeding a surface), agent state.

**A layout is proportions, never geometry.** The daemon holds each tab's tree as splits with an axis and a ratio, and
sends it as a tree. What crosses the seam is the tree and its ratios; the shell lays that out at whatever size it has,
and the pane's own geometry follows from the controller as below. The rule predates muster-daemon: herdr sized panes
for a fixed 54x23 viewport of its own whether a client was attached or not (`observations/herdr-0.8.0.md` section 13),
so the cell rectangles it published described nobody's window. Verbs, as intents: attach, split, close, focus, resize,
send input, spawn. Small on purpose - everything the view needs, nothing any particular backend happens to offer. The
contract corpus at this seam is the executable form of this vocabulary and the definition any replacement backend
(fork or wholesale) must satisfy.

**Find is not one of them.** Find in a window is a view action, performed by Ghostty's search in the pane's
surface, so the marks on screen and the count in the bar come from one matcher. It reaches what the surface holds:
the history its bridge was replayed on attaching, and everything since, on whichever screen is showing - so behind a
full-screen program it reaches that program's screen and none of the history under it. A pane's text read by the CLI
or an agent comes from the daemon instead, and has a reach of its own (`cli/limits.md`).

Agent states are working / blocked / waiting / idle / done / **unknown** - six; unknown renders as itself, never as
success. State is daemon truth, but two of the six are computed in the window, so the vocabulary has to carry what
they are computed from. `waiting` is the other: an idle agent that said, through the daemon's `report --waiting`, that
it ended its turn to wait on work it started itself. It has not finished and nobody is holding it up, so it is neither
idle nor done, and a caller waiting for `idle` is not answered by it.

`done` is an agent that finished while nobody looked, and its two halves have different owners. **The daemon holds the
finish; the window decides the look.** muster-daemon reports four states and never `done`, and marks a pane's record
`finished_unseen` when an agent that was working or blocked goes idle or leaves the pane. It keeps that for as long as
it runs, so a window opened after an agent finished still paints it `done` - quitting and coming back is the ordinary
case, and agents finish in between. Only a window can see its own focus, so the shell reports that focus across the
seam, and a pane is seen when it is on screen in a window that has it. The window then tells the pane's daemon with
`PaneRequest.Seen` (MIP-3 section 8), which clears the fact for every window. herdr, the daemon Muster ran before its
own, derived `done` itself from whether the foreground client's window had focus, which its JSON API had no way to
report (`observations/herdr-0.8.0.md` section 3); so it answered for a window it could not see.

A finish on a seen pane is `idle` at once; anywhere else it is `done` until somebody looks, and gaining focus and
bringing a pane on screen both settle it. The window paints a pane it has just reported as `idle` before the daemon
answers, so the border never contradicts somebody reading the pane for a round trip. Looking away does not un-see what
was already seen. A daemon that reconnects may never have heard a report, so the window takes its reports to that
daemon back: what is on screen is reported again, and the rest read `done` until somebody looks. A report the daemon
refuses, as it refuses every change partway through a handoff, is taken back the same way, and the pane reads `done`
until the next look reports it again. It is not sent again at once, since a daemon still handing over would refuse
that too.

**One legend, and the window holds it.** working cyan, blocked orange, waiting indigo, done green, idle grey, and
unknown a fainter grey rather than a hue of its own. The window's palette is canonical because that is where attention lives: a person
reads borders and dots all day and reads `muster window` when something has already gone strangely, so the surface
with the smaller audience is the one that moves. The two did disagree once, working and done inverted between them,
and what it cost was not a wrong pixel - it was somebody learning that the colours could not be trusted, in the one
vocabulary this product is about.

**Working is cyan because two separate arguments landed on moving it off blue.** It collided with the focus ring,
which follows the macOS accent and is blue on the default one; and plain ANSI blue is the least legible of the sixteen
on a dark background, which costs more on this row than any other because working is the state a window spends most of
its time in. Cyan is legible in both mediums, distinct from green and orange at a glance, and calm - which is what the
busy-but-not-waiting state should be while blocked is the loud one.

**Waiting is indigo because it is the calmest thing that is not resting.** An agent waiting on its own gate needs
nobody, so it must not read as blocked; it has not finished, so it must not read as idle grey or done green; and it is
not working either. Indigo sits apart from all three and from the accent ring's blue by weight. The CLI spells it
blue, the nearest of the sixteen, and blue's poor legibility matters less on a row nobody has to find.

**The two rings differ in kind, not only in hue.** The outer ring is the agent's state, at full weight on all four
edges; the focus ring is thinner and sits inside it with a gap. That is not a decoration: the focus ring follows
`controlAccentColor`, which a person chooses in System Settings and Muster cannot know, so *any* fixed state palette
collides with somebody's accent - green with done, orange with blocked. Painting the two different colours fixes
whichever collision you happen to have and leaves the same bug for the next person, who reports it as a different one.
Weight and a gap read whatever the accent turns out to be. The state ring is the one that keeps its full weight,
because it is what this product is about; the focus ring says one small thing about the window and is the one that
gives way.

**Alignment is hue for hue where both surfaces paint a hue, and no further.** The ANSI sixteen have no orange, so the
CLI spells blocked yellow - the medium's limit rather than a second opinion, and what it names is a slot, so a user
who repaints their palette repaints the legend. Idle and unknown it leaves uncoloured: no colour at all is the
resting rendering of a list of words, the row already prints the state, and the sidebar needs two greys only because
a dot has to be some colour in order to exist.

**The legend is a default, and the window's half of it is a person's to change.** `[colors]`
carries the six agent states and the focus ring (`configuration.md`), and only the window
honours them: the CLI keeps the terminal's fixed sixteen. That answers the question a
configurable legend raises, which is what "one legend" can still mean once anybody can repaint
it. `muster window` reads the same on every machine - which for an agent parsing it is a feature
- and nothing has to map a hex triple onto the nearest of sixteen, a judgement that would be
wrong for somebody and would leave Muster no longer knowing what the legend was.

**What crosses the seam is a value somebody wrote, never a legend.** `a_2HPHq3Zck` rejected a hue
table in `muster-core` on the grounds that a hue never crosses, and half of that survives. A
configured colour does cross - it arrives in the file the core parses, and rides the same
`Appearance` message `divider` already does - but the core gains seven optional strings and no
opinion: it paints nothing, holds no table mapping a state to a hue, and could not say which of
them is the default. The defaults stay in the shell because the window is the canonical surface.
So the core learns what a person chose and still never learns what a state means.

**The values live in two files, and nothing checks one against the other.** `PaneAppearance.borderColor` in
`Sources/MusterMac/PaneChrome.swift` and `agent_style` in `crates/muster-cli/src/render.rs`, each citing this section
and the other, each with a test that fails if its own row moves. The Swift side pins its
*defaults*, which is exactly what the CLI is fixed against, so configurability costs the tripwire
nothing. A tripwire rather than a mechanism: six rows that
change almost never do not earn a generator, and a lint reading both files across the language line is a regex worth
reaching for the first time a tripwire fails to fire.

## Control plane, data plane

Two kinds of traffic, opposite needs, different paths. The split is between *output* and *everything else* - not
between "bytes" and "control":

- **Output rides the data plane.** Each pane has its own stream from the daemon to its surface, bypassing the
  core: the bridge attaches, receives a replay of the pane's whole history composed from the daemon's terminal,
  and then gets every chunk the program writes, unchanged. The surface keeps its own scrollback, built from that replay.
  Hidden panes stay attached, which costs the daemon a socket write per chunk and the surface a parse, the same as
  a background tab in Ghostty (`mip/0003-own-daemon.md` section 4). The core never sits in this path; per-byte
  work in the core is a defect (desiderata: fast is a feature).
- **Everything else rides the control plane, through the core** - daemon events (structure, agent states,
  titles, a program's clipboard write), intents, configuration, and *input*. The daemon holds each pane's
  terminal, so it knows the program's modes (kitty keyboard, bracketed paste, application cursor keys, mouse
  tracking) and encodes against them. The shell reports key, mouse, wheel and text events with full fidelity, the
  core's keymap takes what is Muster's, and the rest goes to the pane's daemon to be encoded with libghostty-vt.
  The surface is handed the same keys the program gets, for what Ghostty does on a keystroke - scroll to the
  bottom, clear a selection - and whatever it writes to its PTY in return is discarded by the bridge, so the
  daemon is the only writer to a pane.
- **The control plane is not one connection.** A window holds two per daemon: one for requests, their answers
  and the daemon's events, in order, and one for input alone, so typing never queues behind a request.
- **The app is never napped.** It hosts the command socket agents drive the window through and the reader of
  every daemon's answers, and agents drive it hardest while nobody is looking - which is when macOS naps an app
  and runs every thread at the background priority, below a niced build. So the app holds a user-initiated
  activity for as long as it runs, which still lets the Mac sleep when idle. Napped beside three gates, a pane
  read once took 13 s, nearly all of it a whole-history answer waiting in the socket for the app to read it. A
  read that names a count now asks the daemon for only those rows, and twenty rows fit in a socket's buffer where
  a whole history does not. Unnapped, a hidden window would draw its panes at full rate for nobody, so each
  window tells its surfaces when it cannot be seen, as Ghostty's does, and libghostty stops drawing them.
- **The wheel goes to both.** The surface scrolls its own scrollback, by `scroll_multiplier`. The same event goes
  to the daemon for the pane under the pointer, which scales it by the same multiplier after rounding a notch up to
  one, as Ghostty does, and gives it to the program only where a terminal would: as a
  mouse report when the program tracks the mouse, as arrow keys when it is on the alternate screen, and otherwise
  not at all. Clicks and drags take the same path, and a shift-click is reported only to a program that asked for
  shift with XTSHIFTESCAPE; otherwise it selects, as it does in Ghostty. The daemon decides that, since it is the one
  holding the program's modes.
- **A right-click opens a menu only when the program does not want it.** The surface is offered the click first,
  and consumes it when the program tracks the mouse, so the program gets the click and no menu opens. Shift gets past
  that on the same terms as a shift-drag. Otherwise AppKit asks the surface for a menu, and so does a ctrl-click
  unless the program has captured the mouse. That is Ghostty's rule. A menu on a pane, a tab caption or an agent row
  sends the requests the keyboard sends, naming the pane or tab that was right-clicked rather than the one with the
  keyboard (`ContextMenus.swift`).
- **A pane is dragged only by its handle.** A small view at the top middle of each pane, Ghostty's, takes the press
  there before the terminal sees it, and nowhere else in a pane starts a move - a modifier that did would be one more
  key no program in a pane could have, and a plain drag is a selection or a report to a program tracking the mouse.
  Dropped on another pane's side it sends `ArrangePane` with that side, the request `muster pane move --onto X
  --down` sends (`PaneDrag.swift`).
- **The mouse's back and forward buttons are the window's, never the program's.** The window takes them before
  any view sees them, wherever the pointer is, and walks the core's focus history with them, the same walk as
  `focus_back` and `focus_forward`. No program in a pane would hear them anyway, because a terminal reports only
  three buttons, so taking them even from a program that tracks the mouse costs that program nothing.

## The shell/core seam

The shell and the core are different languages in one process, so the boundary between them is real and has to be
narrow. It is one C ABI symbol - `muster_dispatch(request_bytes) -> response_bytes` - carrying protobuf-encoded
messages in the vocabulary above, plus a callback the shell registers so the core can wake it unasked (an agent
changed state, a notification is due). `include/muster.h` is the whole contract and is hand-written, because a shell
on another platform implements against it and should be able to read it without building anything. Details and
alternatives in `mip/0001-portable-core.md`.

Two properties keep this from being a bottleneck or a maintenance tax. **It carries events, never bytes**: the data
plane runs adapter to surface and never enters the core, so this seam sees keystrokes and daemon events, not output.
And **the schema is generated on both sides** from `proto/muster.proto` and committed on neither, so a shell and a
core that disagree is not a state the repo can hold.

The core answers every request, including the ones it refuses. A shell cannot otherwise tell "the core said no" from
"the core is gone", and those want opposite reactions - so a refusal is a `Failure` carrying prose written for
whoever finds it in a log, not an error code to branch on.

One answer is neither a success nor a refusal: `Unanswered`, a change the core passed to a daemon that never said
what came of it. The request reached the daemon, so the change may well have happened, and a caller that reads this
as a refusal sends it again - which is how a delivered message arrived twice (kan a_2LOHfLmsL). It is a payload of its
own rather than a kind of `Failure` because that difference is the one a caller has to act on, and the CLI exits 4
for it. A wait on a pane whose daemon stops answering ends with one too: waiting again repeats nothing, but the window
still cannot say whether the pane got there.

Backpressure has no design yet, and the starting answer is a property of this architecture rather than a mechanism:
because view = f(daemon state), a queued update can be **coalesced** rather than dropped or blocked on. That is what
lets this seam afford a bounded queue when there is finally state worth queueing.

The same schema is the CLI and the agent-facing API. "One action path" stops being a discipline and becomes
codegen - a surface that cannot express an action is a missing message, visible at build time.

## Ownership of truth

- **Daemons own structure**: tabs, pane trees, panes, scrollback, agent states, process lifetimes. View = f(daemon
  state). Owning the scrollback buffer is not the same as deciding how deep it goes: that answer, and what a pane
  runs, are Muster's, sent onward as settings the daemon keeps (the shape, above).
- **The core owns a mirror**: a derived, disposable cache of daemon structure, bootstrapped from an authoritative
  snapshot plus event subscription, rebuilt after any gap, never patched across one.
- **A daemon's events say what changed, and its answer says when.** Every event is numbered, and the events a request
  produced are delivered before its answer, which names the last of them (MIP-3, section 9). So the mirror applies
  events and nothing else, and a request waits on its answer only to learn that its change is already there: by the
  time a submit returns, the mirror shows it. Muster never writes anything it was not told. When Muster ran herdr, an
  answer arrived about a hundred milliseconds before the broadcast of the same change, and the mirror applied answers
  and then recognized each broadcast as one it had seen (`observations/herdr-0.8.0.md`, section 14); with answers
  after their events there is nothing to reconcile.
- **The core owns composition**: which daemons are attached, which tabs the window holds and in what order, which of
  them is on screen, and how each divides between the machines holding panes in it. Mixing is at tab granularity: a
  tab holds a region per machine with panes in it, each rendering that machine's pane tree from daemon truth, side by
  side. One region for every tab until somebody groups two. Muster does not own an outer split tree over panes - that
  would make it a multiplexer (non-goal) - and a pane can never move between daemons: the process lives where it
  lives, and grouping moves which tab it is in rather than the process.
- **Composition is resolved against the mirror, never patched by events.** It names daemon things - a tab, a
  pane - and those go away without asking: a tab closed from another client, a pane whose program exited. Every
  such way ends in a window that ignores the keyboard and cannot say why, so composition is brought back into line
  with a daemon's mirror whenever that daemon's structure moves. A region whose tab is gone closes; view-local
  focus falls to a pane that exists.
- **Muster names its own panes.** A name is `p` and nine characters - `p1w3r07bsd` - minted by Muster, and it is what
  every message in the schema means by a pane id. The reason is not tidiness: Muster has to be able to tell a pane
  which pane it is, so the name has to exist before the pane does. A name Muster mints goes into the request that
  creates the pane, reaches it as `MUSTER_PANE`, and is what the daemon knows the pane by from then on (MIP-3, section
  2). herdr forced that order on Muster by assigning its `w1:p3` only in its *answer* to `pane.split`, after the new
  pane's environment had been sent; muster-daemon has no id of its own to assign. The registry that mints names is
  `crates/muster-core/src/names.rs`. A name is unique across every attached machine, which is what makes it an answer
  on its own: a caller naming a pane on the devenv has no way to know and no reason to say which machine holds it. So
  a request that names a pane and no daemon finds the daemon from the pane. The empty pane means "the one this
  window's keyboard is on", which is what a keybinding means and what every menu item sends. The one daemon detail the
  shell is handed is `ViewRegion`'s `daemon_socket`, for the bridge, which streams a pane from the daemon directly.
- **Muster names its own tabs too, for the other half of the reason.** `t1w3r07bsd`, from the same registry and passed
  in the request that makes the tab, as a pane's name is. Nothing has to tell a tab which tab it is, so there is no
  `MUSTER_TAB` in any pane's environment. What the name buys is uniqueness: a request that names a tab and no daemon
  finds the daemon from the tab, exactly as a pane request does, and `muster tab focus` and `muster tab rename` are
  sayable. It is also the whole of what makes a tab hold a laptop pane beside a devenv one: each machine's part of the
  tab carries the same name, so the two daemons record between them that they are one tab without either knowing the
  other exists (MIP-3, section 2).
- **Health is per connection, and so is what a window says about it.** A laptop and a devenv have two answers and one
  title bar. The unhappiest is what shows, named - reporting one state for the window would let a dropped VPN read as
  though every session had gone.
- **Where the keyboard is belongs to the window.** muster-daemon keeps no focus of its own, so which pane Muster's
  keyboard feeds is view-local and nothing another window does can move it. Rendering follows the same rule: the
  daemon records which pane of a tab is zoomed, but a region fills a zoomed tab with the pane its own keyboard is on,
  and uses the daemon's zoomed pane only when its keyboard is on none (`zoom_filling` in `composition/view.rs`).
  Following the daemon there would let another window decide what this one paints, and would leave ⌘2 inside a zoomed
  tab typing into a pane nobody can see.
- **A pane is shown once a tree places it.** The daemon announces a pane as opened before the change to the tab that
  places it, so for a moment a pane belongs to no tab. The mirror holds it aside rather than showing it, and adds it
  when a tree names it (`mirror/state.rs`), because everything that draws or lists a pane does so by its tab.
- **Geometry follows the window.** Pane cell dimensions are daemon truth. The shell converts pixels to cells, and a
  pane's bridge reports its surface's grid to the daemon on every resize, so the pane's PTY is sized to what the
  window draws (MIP-3, section 4). A pane is drawn by one bridge at a time, so no two windows contest its size, and a
  pane keeps its last size when its bridge detaches. When Muster ran herdr, it handed every pane back to herdr's own
  layout on quit, because herdr kept a PTY at the size its last client set and published no other; muster-daemon has
  no layout of its own to hand back to, so nothing is handed back.
- **The shell owns nothing.** Surfaces are disposable renders of a pane channel. A surface attaching to a live pane
  starts with a full repaint and never assumes it saw the start of the stream. Closing a window destroys surfaces
  and touches no session.

## Event model

State changes only by applying events and intents in one place, in one order per daemon connection; rendering reads
the result. Pane content is not state in this sense - it is a stream the surface renders. The daemon's events are
built for this (MIP-3, section 9):

- **Application is convergent.** Every event carries a whole record - a pane as it is now, a tab with its tree and its
  zoom - never a delta, so applying one twice is applying it once, and snapshot-plus-events converges.
- **Events arrive in order, and a gap is a resubscribe.** Subscribing answers with a snapshot and the number it is
  current to, then every later event in order, with nothing replayed from before it. A client that sees a number
  skipped subscribes again, and the fresh snapshot is the whole repair: the mirror reports only what differs from what
  it held (`muster-daemon-client`'s `follow`). A reconnect, a daemon that restarted and one replaced by a handoff are
  the same case.
- **One event removes a pane**, whether a request closed it or its program ended.
- **Agent state travels with the pane.** It is a field of the pane's record, on the one stream every other change
  uses, so an overview of N panes costs no connection of its own.
- **Cross-daemon order is core order.** Streams from different daemons have no mutual order. Composition and
  attention are ordered by the core's own application sequence, and nothing may depend on cross-daemon event order
  for correctness.

What herdr's events forced on the mirror - rejecting replayed events, remembering removals, a subscription per pane
for agent state - is recorded in `observations/herdr-0.8.0.md` sections 10 and 11, and went with it.

Rendering is driven by diffs scoped to what changed: an agent-state change costs that change, not a walk of every
pane (desiderata: fast is a feature, the per-event half).

## Attention routing

Attention is computed in the core from control-plane events - agent-state transitions and a program's bells,
notifications and progress all arrive there, so the data-plane bypass costs nothing here. The core owns the unread and urgency
ordering; the shell only delivers notifications and renders indicators. `focus_asking` (⌘⇧A) is that ordering's head
as an action: it goes where clicking the most urgent banner would. Activating a notification dispatches an
ordinary focus intent through the one action path - which may change composition first, because the pane that asked
may not be visible in any window. Surfacing the hidden is part of the feature, and the core owns it.

**Two states ask, and they ask the same question from opposite ends.** `blocked` is an agent that has stopped and
wants an answer; `done` is an agent that has stopped and nobody has noticed. So both notify by default, both can be
switched off on their own, and one key silences both without forgetting which of them you wanted
(`configuration.md`, `[notifications]`). A quiet path is in the first version rather than a later one because
everything notifying is the same as nothing notifying, and somebody running fifteen agents finds that out on their
first afternoon.

**A program in a pane can ask too, and a bell never does.** A program's own notification (OSC 9, OSC 777) asks with
its own words, which is the notification saying why; it stands until somebody looks, the program's later notifications
raise nothing until then, and a blocked agent outranks it.
A pane running an agent Muster recognizes is the exception: an agent notifies when it has sat idle or wants permission,
the moments `done` and `blocked` already ask about, so its own notification raises nothing and its state asks instead.
A bell only marks the pane until somebody looks, because shells ring for trivia. Progress (OSC 9;4) asks nothing and
is shown with the pane's agent. None of the three is in a daemon's snapshot, so a window that reconnects holds a
program's progress no longer: it cannot know the work is still running.

**A message for the human asks too, and no pane asks it** (MIP-4, section 10). The daemon on the machine the app runs
on, the human's home, tells each window that attends it what waits for the human in each group, as state in its
snapshot and events, and the core asks by that group. It comes after a blocked agent and before a program's
notification, because addressing the human is the one interruption an agent chooses to make. Every message that
waits announces again, one banner per group replaced each time, and a group met at launch announces where a pane
does not: a message to the human waits for the next window rather than being history. Going to it opens the group's
transcript, a pane running `muster msg log --follow`, found by the command it runs or made in a new tab. Going
there is the human reading the group, so the daemon is told, and the banner comes down when it says nothing waits.
A transcript somebody is reading in a focused window reads what arrives there, as a pane on screen raises nothing.

**A pane the window is focused on and showing raises nothing.** That is what the border is for, and it costs no new
rule: seen-ness is already computed for exactly that pane, and a finish there is reported seen rather than announced.
So the notification set is the same fold the state is, with the file's answer laid over it.

**Notifying and seen-ness are two sets, deliberately.** Seen-ness decides what a pane *is* and is not a person's to
configure; the unread set decides what Muster does about it and is. A muted window still paints `done` on its
borders, still lists it in the roster, and still prints it from `muster window` - what mute takes away is the
interruption. Folded together, a preference about banners would quietly change the state vocabulary the product is
built on.

**A request is withdrawn as well as raised**, and the event carrying it says which. An agent that goes back to work,
a pane somebody looks at, a pane that closes, and a state switched off in the file are all moments the core already
computes, and a banner outliving any of them is a keystroke that lands somebody on an agent which stopped needing
them - the exact failure notifying exists to prevent, arriving one step later. A pane that was already `done` when
the window attached raises nothing at all: Muster witnessed no transition there, so a banner would be Muster
announcing history at launch.

**Everything the daemons hold is published as well as everything on screen.** A window shows what fits in it, and the
pane most likely to have finished unnoticed is the one no region is showing - so a roster travels beside the view:
every pane every attached daemon holds, ordered and named for a reader, each row saying whether anything is showing
it. Order and label are decisions and live in the core, so the sidebar, the CLI and an agent all get the same answer.
Structure only, like the view: what an agent is doing keeps its own per-pane message, because a roster is stable and
a state blinks.

**A pane is named twice over, and the two age differently.** A name somebody typed is durable identity - the daemon
writes it down, so it survives a daemon restart - and it wins over anything derived, because it is the only line
written for that pane rather than worked out from where it sits. Under it, when there is something to say, goes what
the agent calls itself: volatile status, lost on a restart because the process that would set it again is new
(MIP-3, section 2). Between them a row has a first line stable enough to learn and a second
that changes as work happens, which is what fifteen rows reading `<directory> · claude` could never do.

The second line is drawn only for a pane with a detected harness whose title says something the first line does not,
and that rule is the core's on the same terms as the ordering. A plain shell titles itself too - usually with the
path the row already leads with - so "draw it when it exists" would double the height of the list to repeat it, and
suppressing that by matching shell prompt conventions would be a guess about somebody's dotfiles where a detected
harness is a fact the daemon reports. A row is one line or two and never taller: a height that varied with the length
of what an agent wrote would move the rows below it while somebody was reading them.

Naming is an ordinary intent through the one action path, so a chord, a menu item, the CLI and an agent all reach it,
and nothing is rendered optimistically - a rename is applied from the daemon's event, the way a split is, so a rename
made in another window arrives by the same route. Clearing a name, a pane's or a tab's, leaves it called after what it
is again: its directory, or its place.

**The roster is a tree, because a tab is what a person navigates between.** Tab, then pane: a flat list of panes
cannot say which of them sit side by side, and a window shows one tab at a time, so "where has that agent got to" is
a question about tabs. A tab also says whether the window is showing it, which is not the same question as its panes
being on screen: a zoomed tab is on screen while all but one of its panes are not.

**The machine is not a level of it, and that is the change stage three of MIP-2 made.** A tab may hold panes on two
machines, so a heading over it would be wrong for some of its panes and the list would stop describing the window
beside it. Which machine holds a pane is on the pane's row, drawn only while more than one is attached - on one
machine the answer is the same on every row and says nothing. It is drawn as a swatch in the machine's color, which
the core decides - the config file's choice, or one drawn from the machine's id - so every window agrees about it. Beside the tabs, the roster carries the machines
themselves, for the two states no pane row can hold: a machine that is unreachable, and a machine holding no panes at
all. Without them a machine you asked to see would vanish from the window entirely the moment you closed its last
pane, which is the state kan a_2HpkpfIfq was about.

Naming is the core's, on the same terms as the ordering: a tab nobody named is captioned by its place in the window's
count. Each row carries the tab's Muster name beside its place, so reading the roster and acting on what it says are
the same vocabulary.

**The order of tabs is the window's, and so is the numbering over it.** Which tabs a window holds, and in what order,
is its composition (above), so the list walks tabs in that order, and one number runs across every attached daemon -
which no daemon could produce, because no daemon knows the others exist.

**A chord names a tab, and the press after it names a pane in that tab.** Every tab carries a place in one count
across every attached daemon, and every pane a place within its tab. ⌘1 to ⌘9 name a tab, and the next press, made
with the modifier still held, names a pane inside it. The core holds the half-typed chord, because on macOS a chord
is a menu item and the round trip into the core is the only place both presses meet; anything that changes something
takes it back, and so does the shell reporting that the modifier came up. A window of one tab numbers its panes
instead, because there a first press carries no information. Each row carries the whole chord that reaches it, built
from the same functions that resolve a press, so the digits drawn and the keys pressed cannot disagree.

Panes also keep a place in the whole window's order, which is what `muster window` prints and `muster focus --place`
takes. A script needs a number that means one thing, and a chord means one thing or another depending on the press
before it, so the two are separate requests.

This replaced an earlier scheme where ⌘1 to ⌘9 named panes down the whole window in one press. Both were driven side
by side, and amterp chose this one (kan a_2A6T9c2r4).

The numbers are positional and they move when a tab or pane before them closes. That is the cost of numbering the thing that
churns, and it is the right trade once the order is the user's to arrange: a stable number would keep its value when
you moved the row, which is the opposite of what the gesture asked for. What it does not fix is a number going stale
between reading it and pressing it, and the answer to that is elsewhere - a notification names the agent, not the
chord.

**Arranging the list is arranging the window, and Muster stores no order of its own.** Dragging a row is an ordinary
intent through the one action path: the daemon rearranges its own tree and the list is a view of that, so
`view = f(daemon state)` holds and the order survives a restart the way the panes do. The alternative - a
presentation order in a window's saved arrangement beside tab order and widths - buys free-form insertion at the price of a list
that no longer says where a pane is on screen, and of Muster owning an ordering the daemon has never heard of.

One gesture, two requests, and the choice is the core's. Two panes in one tab exchange places; a pane dropped on a
row in another tab moves into that tab behind it. The shell knows only which two rows were involved, so it sends
both and the core picks the verb from where the panes are - a shell that chose would be a second place that rule
lives, and it would have to read the tree to do it. An exchange rather than an insertion because an arrangement has
no "between".

A side is the other answer, for the gesture that can say one. A pane dropped on another pane's edge names the side
it goes to, so `ArrangePane` carries that side and the core sends a move whichever tab either pane is in: the pane
leaves its place and shares the target's, half each. That is how a split changes direction, and the daemon has
always been able to do it - what was missing was a request that could say which side.

A drop across daemons is refused in the shell, before it becomes a request. A pane is a PTY its daemon owns, so
moving one to another machine means killing a process on one host and starting a different one on another - not a
move, and nothing the core could honestly do with the intent.

**A drop onto a tab caption is the one that may cross machines**, and it is a third request rather than a loosening
of the refusal above. It names a tab and nothing inside it, so nothing is ordered and no tree is involved; the pane
stays on its own machine, and what changes is which Muster tab it belongs to. When that tab has no part on the pane's machine
yet, the adapter makes one there and binds it as that tab's member - which is how a tab comes to hold a laptop pane
beside a devenv one (MIP-2, stage four). `muster pane move --tab` is the same request from a script.

**Moving the keyboard comes in two axes, and the second is what makes the first a guarantee at all.** Panes and
tabs are different questions: the *relative* pane moves reach everything the window is *showing*, and the tab moves
reach what is behind it. Without the second, a pane in a tab the window is not showing would be reachable only by
clicking its row - and the list can be put away, which would leave those panes with no door. The numbered chords are
the third route and cut across both, because a chord reaches a pane whether or not anything is showing it. Tab moves have no
geometry, because tabs are a list and nothing is to the left of a tab; both directions wrap.

**Within the panes on screen, moving comes in two kinds.** Next and previous walk reading order across every region
and wrap, so between them they reach every pane - that is the guarantee. The four directions are geometric: the core lays the whole
window out from the ratios it already publishes plus the region weights, and picks the pane actually in that
direction, requiring it to overlap the source across the direction of travel. They do not wrap, because reachability
is already covered and predictability is worth more. Asking the daemon was rejected: `BackendChannel::submit` is
write-only by design, and every future backend would owe us a read to answer a question about an arrangement Muster
already holds. A tree walk was rejected too - on a perpendicular split it has to pick a child by position in the tree
rather than by where it is, so in any asymmetric arrangement it lands somewhere the user did not point at.

**The arrangement over regions is Muster's, and only Muster's.** Each region carries a weight and the tab divides
its width by their sum, so equal shares are what a tab that has never been dragged looks like. A weight per region
rather than a ratio per boundary, because regions are a list and not a tree - owning a tree over them is what would
make Muster a multiplexer. There is a line to drag only in a tab holding panes on more than one machine, which is a
tab somebody has grouped; dragging it moves only that pair's share of itself, so nothing further along the tab moves,
and it is the one drag that settles in the core rather than being asked of a daemon: no daemon knows the other one
exists. It is also the one share that is clamped, because nothing sits behind it to
refuse an impossible one, and a region dragged to nothing would leave no divider to grab.

**A focus request surfaces the pane it names.** Naming a pane the window is not showing brings its tab on screen
rather than being refused - a list of panes that cannot be reached is a display, not routing. The tab it left keeps
its regions, their widths, and which of them the keyboard was in, so switching away and back lands where it was
left; and switching costs no bridge, because a surface belongs to its pane and is parked rather than torn down.

## Input precedence

A keystroke resolves in fixed order: first the Muster keymap - if the chord is bound to an action, dispatch it and
stop; otherwise it is reported, with full fidelity, toward the focused pane via the control plane. The wheel is the
exception: it goes to the surface and to the daemon both (see data plane). The keymap is data in the config file, not
code.

## One action path

Every operation - keybinding, menu, CLI, socket API - dispatches the same action into the same core dispatcher;
invocation surfaces carry no logic of their own (desiderata: parity by construction). The Muster CLI talks to the
running app over a local IPC endpoint, and it covers everything Muster does - panes and tabs as well as focus and
arrangement.

**The endpoint is the same schema on a different transport, not a second path.** A window binds
`~/.muster/state/command-<pid>.sock` and answers a length-prefixed `Request` through the same `dispatch` the C ABI
calls, one request per connection, a thread each so a request waiting on a slow daemon does not hold up a
caller asking what the window looks like. Nothing there decides anything; a second entry point that made its own
decisions would be a second Muster.

**One request is answered more than once.** A `WatchPanes` keeps its connection open and is sent each change to a
pane's agent as the window hears it - the same changes the shell is sent as events - until the watch ends or the
caller hangs up. The one-answer rule was about a caller that runs one command and exits; a caller waiting for an
agent to finish is the one that does not, and polling `ReadWindow` for it was late by the interval and blind to a
finish and a new turn between two polls (kan a_2M9T8O6dL). The endpoint routes a watch away from `dispatch`, which has
one answer to give, and checks once a second whether a quiet watch's caller is still there, so an interrupted `muster
pane wait` does not hold a thread. A watch is also told when a daemon it follows stops answering and when it is back,
because nothing about that daemon's panes reaches the window in between (kan a_2P5njTPcm).

**A request about another window's tab is carried to that window.** A tab belongs to exactly one window, and any
verb works from any window, so the window a caller reaches checks whether a change it was asked for names a tab - or a
pane in a tab - another open window holds, and if so sends it over that window's socket wrapped in a `Carried` and
relays the answer. Here at the endpoint rather than in the CLI, because the socket is a door for anything that speaks
the schema; and not in `dispatch`, because the shell calls that on its main thread and only ever shows its own tabs. A
carried request is answered where it lands and never carried again. Questions are answered wherever they arrive,
since every window follows the same daemons.

**A pid in the socket name, because two Musters are two windows.** A caller has to be able to reach the one it means,
and a single fixed path would mean the second window to open silently took the first one's callers. Which window a
pane belongs to is settled when the pane is made: Muster puts `MUSTER_SOCKET` in the environment of that request,
beside the `MUSTER_PANE` that says which pane it is, and between them a program inside a pane can drive the window it
is drawn in without being configured. The pane outlives that process, so a pane whose window has quit asks the sockets
beside its own that share its name up to the process - the other windows of the same Muster on that machine - and a
change names its pane, so whichever answers carries it to the window holding the pane's tab. A name kept per
arrangement was the other way, and would have left every pane already running unreachable after the relaunch that
brought it in.

**A devenv pane is told a path on the devenv, which the window's ssh master carries back.** A unix socket path means
nothing on another machine, so the window asks the master it already holds for that daemon to forward its socket to
`window-<install>-<name>.sock` beside the daemon's own socket over there, and that is what the pane is told. Beside the
daemon's socket because that directory is Muster's however the daemon was configured; with the install, the daemon
socket's own name, because a development build and the release can both forward there and a pane asking its quit
window's neighbours must meet only its own Muster's; and with a name minted the way a pane's is, because two laptops
can attach one devenv and a pid is unique only on its own machine. Asked of the master
through its control path rather than given at its start: the master exits on any forward it cannot make, and a far
sshd that refuses this one should cost a program over there its window, not every pane on that machine. Made again
at the same path whenever the master reconnects, so a pane told it before a dropped VPN reaches the window after, and
taken off the devenv when the window closes. Full trust in both directions is the default, and the socket sshd makes
is readable only by the devenv's own user.

**The command has to be findable, or the surface is taught rather than discovered.** Muster keeps `~/.muster/bin`,
points a link in it at the CLI the running app shipped, and gives the directory to every daemon it starts as the
front of that daemon's `PATH`. A pane is a child of its daemon, so that is every pane. The link is refreshed at each
launch rather than installed once, because the app it points at moves.

An install now puts a second `muster` on the `PATH` outside Muster, and what keeps that from mattering is
`$MUSTER_SOCKET` rather than the prepend. Prepending is best effort and measurably so: a login shell rebuilds `PATH`
from a profile after the daemon has handed one over, and on the machine this was measured on `~/.muster/bin` came
49th while Homebrew's directory came 20th. It costs nothing, because a pane's window is named by the environment and
not by which binary answers - either CLI drives the window it is sitting in, and the only difference left is which
build does the driving, which is a question only across versions. Wanting the prepend to hold would mean Muster
rewriting a person's `PATH` after their own profile had, which is not a thing a terminal should do.

A profile can also drop the directory altogether: Debian's `/etc/profile` sets `PATH` outright for every login shell,
which is what every devenv pane is. What Muster does after the profile is what Ghostty does, append and never reorder:
the daemon turns on the shell integration's `path` feature, which appends the `bin/` of its data directory when it is
missing, and writes the same line into the script a pane's command runs in, which has no integration. That directory
holds a `muster` which runs the one in `~/.muster/bin`, so the CLI that answers is the one the prepend would have
found.

The one thing an install does owe the link is cleanup: uninstall deletes the bundle the link points into, and the
app that would have repaired it is the one that just left.

**It is not a view-layer CLI beside the backend's own.** That was the earlier plan, on the reasoning that herdr had a
good CLI already and Muster should not reimplement it, and three things sank it. A window can be attached to more
than one daemon, and a backend CLI inside a pane reaches that pane's daemon and no other - so it cannot put a pane on
the devenv, and cannot answer what the window is showing, because no single daemon knows. Making a pane and landing
on it is one intent to whoever asked, and splitting it across two CLIs makes the second half unsayable: an
arrangement made behind Muster's back appears, because view = f(daemon state), but nothing focuses or zooms it and
the caller has no way to ask. And a documented backend-shaped surface becomes the contract whatever this document
says, because every script and skill written against it is what a replacement would have to provide - which is the
one thing "we never let one own our contract" rules out.

So Muster's CLI is the agent surface, and the daemon offers no other: `muster-daemon` has no verbs for panes or tabs,
only `report`, for a harness in a pane to say what its agent is doing, `replace`, which hands a running daemon's
panes to a successor, and `ssh`, Muster's port of `ghostty +ssh`, which Ghostty's shell integration calls when a
pane runs `ssh`.

What that CLI is *not* is a verb-per-backend-verb translation. It is shaped to intents, one call each, because that is
where the knowledge lives: `muster pane new --run` puts the command in the request that makes the pane, so the daemon
starts it before anything could be typed, and nobody scripting "make a pane and run this in it" has to find out that
typing it into a fresh pane races the shell's first prompt.

Reads are half of it. A person driving the GUI can see which panes are on screen and where the keyboard is; an agent
has to ask, and a CLI that only writes leaves one arranging a window it cannot look at. `ReadWindow` answers the
view, the roster, every pane's agent state and each daemon's health as one message, built by the same builders that
produce the events a shell is sent - so a read cannot contradict what is on screen. Health is in the answer because
the rest of it is a mirror, and a mirror nobody has heard from in an hour looks exactly like a current one.

On macOS a keybinding *is* a menu item, because that is where the platform dispatches a key equivalent. Matching
chords before the pane sees them would take shortcuts the user rebound in System Settings and make them mean
something else, and would hide from every menu what the app can do. So the menu is where Muster's own actions live,
and each item does nothing but dispatch.

The menu is therefore also where a press is written down. A chord Muster consumes never reaches a pane, so
`input.key` has nothing to record and the action's own effect is the only trace it leaves. Eight `pane.font_size`
records 100 ms apart read as a replay bug for an evening; they were one person holding ⌘+. So a dispatch writes
`input.bound.action` before it dispatches: the action's name, the chord as it was actually pressed, whether the key
repeated, and whether it arrived as a shortcut or as a menu somebody picked. A reader then has the cause in front of
the effect rather than inferring it from the spacing.

**The intent is parameterized; the action is not.** `CreateTab { tab, cwd, run, name }` takes arguments, and
`new_tab` is
a parameterless name that dispatches it with defaults. That split falls out of the menu: an item has exactly one key
equivalent, and that is also the handle System Settings needs to rebind it, so an action name has nowhere to put an
argument and `[keymap]` stays keyed by action. Ghostty's chord-keyed form - `cmd+shift+h=resize_split:left,150` -
lets a config name two chords for one action, or a binding with no action name at all, and neither is something the
menu can represent or the platform can rebind.

What that costs is paid in the config file rather than in the vocabulary: an amount a chord would have carried
becomes a root key, which is what `resize_step` and `scroll_multiplier` are. Where an action genuinely has a small
closed set of arguments, it becomes that many actions - `focus_pane_1` through `focus_pane_9` are nine names the
config file and the menu can both say, over one `Action::FocusPane(u8)` in the core. Nine menu items need nine
actions; the core still holds one intent.

The CLI does not inherit any of this. It names intents directly and passes arguments, because nothing about it is a
menu - which is the whole point of separating the two, and why the constraint stops at the keymap.

Something the app notices on its own is a *trigger* for an action, never a second way of doing it. The config-file
watcher is the worked example: saving the file dispatches `reload_config`, the same action a chord, a menu item and
a CLI dispatch. That is what keeps "one action path" true of things nobody pressed - the alternative is a second
implementation that drifts from the first, and a bug report where the answer depends on how the reload was asked
for.

**A failure the person caused is reported to the person, as a condition rather than a message.** The core holds a
list of problems keyed by what is wrong (`problems.rs`), publishes it whole, and the roster draws it at its foot;
raising the same condition twice is one problem, and it clears when the condition does. Keyed and whole because both
alternatives break the same way: a stream of messages lets a window go on showing a config refusal after the file
was fixed, and an add-and-remove protocol lets it disagree about how many there are. The disappearance is also the
only acknowledgement a fix ever gets.

The run log is not that surface, and mistaking it for one cost a whole evening: a `resize_step` written without its
unit refused a config file at 18:55, every setting in it went inert, and the window said nothing until somebody
opened a JSON file the next morning. So a run log entry answers "what happened here" for whoever is debugging, and a
problem answers "what do I do now" for whoever is typing - the same fact, twice, because the two readers arrive by
different doors. The log does hold what the person was told: every raise is a `problem.raised` record carrying its
sentence, and every clear a `problem.cleared` record saying why it went, because a problem going away is not always
the thing it was about being fixed. Severity exists to decide interruption and nothing else: an error opens a roster
somebody closed, a warning waits to be found.

**The list also carries failures nobody caused, and a pane that never becomes typeable is the first of them.** A
pane's output reaches its surface through a bridge that dials a socket the core bound for it, and until that
connection arrives the pane shows what it last drew and nothing typed into it appears. Three separate bugs ended in
exactly that
state - the bridge failed to dial, the socket path had moved, the channel could not be opened - and every one of them
was found by somebody typing. Both ends of the wait were already known to the core, which binds the socket and runs
the callback the accept fires, so what was missing was a deadline between them: five seconds, one problem per pane,
cleared by a bridge that arrives late and by the pane closing. **What keeps the accusation about something is that
both ends are scoped to what the window is drawing**: a socket is bound for the panes a region shows rather than for
every pane its tab holds, and the wait is counted only while one is being shown. A pane nothing draws renders
nothing, so it cannot be swallowing anything - and one drawn again waits a whole deadline from the moment it is
drawn rather than carrying forward a silence nobody was in a position to notice. A zoomed tab is where that was
found: a window opening onto one accused the three panes the zoom covered, every launch, as a notification each.
An error rather than a warning, even though nobody
misconfigured anything, because severity is about interruption and a warning waiting to be found would be found the
old way - by typing into a pane that had stopped drawing. The decision is a fold in `typeable.rs` and the clock is
a single parked thread in the seam, so an idle window costs no wakeups and the rules are answerable by a case.

**And it says which of those it is, because the pane looks the same in all of them and the remedy does not.** A
pane whose bridge lost its connection recovers on its own once the machine is reachable; one whose attach was
refused is taken over by a reattach; one another bridge took over is being shown in another window and wants nothing
done at all. Until the bridge reported how it ended, all three raised one sentence pointing at a log
file - true, useless, and asking the person to open the one surface this list exists to replace. The endings arrive
on the pane's own control socket, and the sentence for each is a case in `corpus/conformance/typeable.json`, where
prose somebody reads under pressure can be reviewed as prose.

**The same watch asks for another bridge, timed from when the shell started the last one.** The shell builds a bridge
when the number a view carries for a pane moves, and reports `BridgeStarted` once it has acted on it. A pane whose
started bridge has not dialed three deadlines later is asked for again. Timed from Muster's own ask instead, a loaded
machine whose shell ran minutes behind was asked nine times in two minutes and had bridges replaced that were only slow
to spawn (kan a_2YBZU4Ujx), so Muster also asks nothing more on its own for a pane whose last ask has not been
started. A person's `muster pane reattach` always asks, since it is the way back for a shell that never started one,
and so does a replacement for a bridge that ended.

**And a daemon that answers again gets bridges for its dark panes at once.** Bridges started while a devenv's tunnel
was down fail to attach, and the watch would not ask again for fifteen seconds. So the moment the daemon answers,
every pane of its that nothing has dialed is asked for, and its wait starts over, so a pane still dark says nothing
has dialed rather than telling the person to check a connection that is back (kan a_2YQD5xCFq).

Nothing renders an intent optimistically. A split, a close, a focus and a divider drag are all requests, and what
came of them arrives as the next published view - so a window can never show an arrangement no daemon agreed to. The
one thing an intent may settle locally is where Muster's own keyboard lands, because that is Muster's state and not
the daemon's: a split hands back the pane it made, and that pane takes the keyboard, because that is what pressing
the key meant.

**A request may also be waited for off the main thread, and two gestures have to be: a divider drag, and the
window being moved or resized.** Every other gesture is one request; these are one per event, about a hundred a
second, and the seam is entered synchronously - so the window spent whole gestures inside a round trip and had no
time left to draw what was being dragged. The request is handed over instead (`LatestRequestSender`): one in flight,
the latest remembered, and what arrived while a request was out goes next. A gesture then runs at whatever the round
trip allows rather than queueing behind itself, and the position it ends on is always sent, because the remembered
one is always the last asked for. This is contained to those two rather than made general - the other drag in the
window moves a region boundary, which is Muster's own composition and never reaches a daemon.

**Input never waits at all.** Every keystroke, click and paste goes to the daemon on the input connection, which is
never answered: events queue for a writer thread, and a queue full enough to mean the daemon has stopped reading drops
an event rather than freezing the window (`muster-daemon-client`'s `input`).

## Messages between agents

Agents post to each other through the daemon on their own machine, not through a window (MIP-4). The rules -
participants, groups, each group's log and policy, every participant's place in it, the guard, and who is due a
wake - are
`muster-msg`, a crate that knows a pane only as a name and knows nothing of protobuf or files, so they are tested as
conformance cases (`corpus/conformance/messaging.json`) with no daemon running. `muster-daemon` hosts it behind the
`msg` requests, under a lock of its own so that a post never waits behind the pane tree, keeps each group's log in
`<install>.msg/` beside its socket, synced before a post is answered, and delivers wakes after releasing that lock,
because a wake is a connection to another process that may be slow or gone.

**An agent in a pane is woken through the pane, and whether it is there is the pane's agent state.** Each request
reads every pane's agent and state with the session held, lets it go, and only then takes the messaging lock, so the
two are never held together. The wake is typed in only when, just before the write, the
pane's screen reads as its agent at an empty prompt: detection's winning rule for the screen carries a `prompt`
pattern, and only text drawn faint follows it. So only a harness whose manifest has such a rule is ever rung, which
today is Claude Code alone. A wake for an urgent post may also be typed into a working agent's prompt box, read the
same way by a working rule, since Claude Code takes a line queued there into the turn it is running. A wake that
cannot be rung yet waits in the daemon for a thread of its own, woken by posts and agent state changes and otherwise
by the next deadline - a quiet period ending, a Return due again, or five seconds while a prompt holds a draft (MIP-4,
section 6).

**An agent whose own hooks fetch its messages is never rung while they run.** A Claude Code session given
`extras/claude-code/messaging-hooks.json` reads what arrived after each tool call and, when its turn ends, waits in
the background with `muster msg wait --due`, whose answer starts its next turn. While that wait is connected, or it is
working and one of its verbs ran in the last five minutes, a post marks it woken and types nothing; otherwise it is
rung as any agent in a pane is. An agent seen idle with no wait connected is rung for what its hooks were counted
on to fetch, after two seconds in which a `Stop` hook starting late can connect and be told instead.

**A group is kept on the daemon it was made on, and other machines hold replicas of it** (MIP-4, section 11). A
window attached to this machine's daemon and an SSH one holds a `msg.peer` request open on the local daemon for each
pair, naming the local end of the far daemon's forward, and the daemon links to it for as long as the request lasts.
The two call each other over that one connection (`messages/peer.rs`), since ssh forwards it one way only. A daemon
sends its members' joins, leaves and posts in a group kept elsewhere to the group's home, which numbers the entry,
checks the guard against the cursor the post brought, and sends the entry on to every other machine with a member.
Each daemon wakes only its own members. Each machine writes its own members bare and another's as `name@machine`,
and whatever crosses the link is turned into the receiver's names on arrival. That happens in `muster-msg`, so the
rules are tested with two services and no daemon (`crates/muster-msg/tests/msg/across.rs`). With no window there is
no link: a change to a group kept elsewhere is refused at once, naming the machine. Replicas are not kept on disk,
so a daemon that starts holds each group a cursor names as an empty replica until a link comes up and refetches it,
a page of at most 8 MiB of bodies at a time, since a link carries 16 MiB in a frame and a log is never cut short.
Every name a peer sends is checked before it is turned into this machine's, so a far daemon cannot act as this
machine's participants or write into a group kept here.

**A group's policy binds at its home, whichever machine a request came from.** A forwarded post runs through the
same `post_as` as a local one, so the home checks whom its author may address and whether the group is paused, and
a forwarded join or leave is checked against the group's membership. Every batch of entries the home sends carries
the policy, so a replica wakes its members as the home would, and a pause or resume there pauses or wakes the
replica's members too. A policy, its members and its pause are changed only at the home; on a replica those verbs
are refused, naming the home. The human is homed on the machine the app runs on, and a policy's `@human` means that
person on any machine, so the human on the laptop is rung in a group kept on the devenv as in one kept on the
laptop. A daemon that a link dialed learns from it which machine the human is homed on, and from then on has no human
of its own: a person's shell there is the laptop's human, which may post in and change a group kept there but reads
and waits only at home, and an agent's `@human` there means the laptop's. A window takes what waits for the human
only from the daemon on its own machine, and the guard never holds the human's post on either machine.

**Messaging is one of two request paths that do not run through the core.** Messaging has to work with no window
open, and the core lives in the app, so `muster msg` dials the daemon itself: `$MUSTER_DAEMON_SOCKET`, which every pane has,
or else this install's daemon. A window that shows messages will send the same requests, which is parity by the same
construction the core gives everything else. The command, its namespace and its verbs are spelled once, in
`muster-daemon-proto`'s `messaging` module, which the CLI's grammar, the daemon's wake text and every refusal naming
the next command all read - so a rename is one edit.

**The other is a window's questions about panes when no window answers.** `muster window`, `pane read`, `pane send`
and `pane wait` ask the same daemon when `$MUSTER_SOCKET` names a window that is not there, or none is listening: on
an SSH devenv nothing forwards a window to, or in a pane whose window has quit with no other window of that Muster
open. The CLI builds the window's own
answers from the daemon's records, so one renderer prints both. It stays a second path in transport only: the rules
the window applies on the way - paging to a pane's newest rows (`muster-daemon-proto`'s `pane_text`), counting rows
and confirming a send (`muster-core`'s `pane_text`), and which states end a wait (`AgentState::counts_as`) - are
the same functions on both paths rather than copies of them.

## The renderer seam

The renderer gets the same treatment as the backend: a narrow contract in Muster's terms - create a surface in a
region, run a pane channel into it, resize it, search it, read its grid (the test oracle) - and nothing
libghostty-shaped escapes the seam.

**Find is the renderer's own.** A surface parses the program's own bytes and keeps its own scrollback (the data plane,
above), so libghostty's search covers everything the surface holds: the shell hands it a needle, and libghostty
searches, marks and counts on a thread of its own. What the renderer refuses comes back rather than throwing, on the
same terms as sizing text: a renderer that cannot search costs the marks and nothing else, and that is a line for the
log.

Today the only way to feed an embedded ghostty surface is the command it spawns; the embedding header has no byte-feed
API. The pane channel is therefore delivered by a bridge subprocess the surface runs. That is a fact about current
libghostty, not a choice - re-verify on upgrades, and revisit if upstream grows a direct feed.

**A surface belongs to its pane, not to the region showing it.** One per pane per window, held by the shell for as
long as the pane's daemon holds the pane, and lent to whichever region is showing it. It follows from the line above:
a bridge is a surface's command, so a surface torn down when its tab went off screen took the bridge with it, and
every switch back paid for a new attach and its replay. When Muster ran herdr a remote bridge was also an `ssh` exec,
measured at 444-561ms a switch against 29-59ms for a local pane. The rule also keeps a pane drawn in two regions from
becoming two bridges dialing one pane, which the daemon allows only as a takeover. What it costs is one bridge and one
stream per pane rather than per pane on screen; the roster is what says a pane has closed, and that is when its
surface goes.

**Appearance crosses this seam in Muster's words, and reads no file belonging to another application.** Muster
called `ghostty_config_load_default_files` until 2026-08-16, so a Ghostty config on disk decided what a pane looked
like - which left the renderer the one dependency not behind the contract, since a replacement would have had
nothing to read. What replaced it is `[font]`, `[colors]` and `[cursor]` in `~/.muster/config.toml`, parsed and
refused by the core, published on one `Appearance` read, and translated by the shell for whichever renderer is
behind the seam. One function in `MusterRenderer` knows a ghostty config key exists, and no ghostty spelling
appears in `crates/`, in the schema, or in the corpus - `hollow` is Muster's word and `block_hollow` is the
translation's problem.

The vocabulary names what a person may change, and nothing else: every value is optional, and absent means the
renderer's own default rather than one Muster invented. That is deliberate and it is a stated limit rather than a
gap. Muster has no opinion about which monospace font a machine has, and a sixteen-entry default palette written
into the core would be a transcription of somebody else's rather than a decision - so a replacement renderer
supplies its own defaults for anything unnamed.

How the values get there is a fact about libghostty rather than a choice, and the same shape as the bridge: the C
API has no setter, so the shell writes a derived config file and hands over its path
(`docs/observations/libghostty-9f9b8d1d.md` section 9). A synthesized argv works too and needs nothing on disk, but
`ghostty_init` assigns process-global state and so can only be done once - a file serves both the first launch and
every reload after it, and one mechanism cannot disagree with itself. The derived file is state, lives in
`~/.muster/state/` beside the saved arrangements, and is rewritten every launch; it is also the answer to "what did Muster actually tell the
renderer", which is the first question when a colour does not take.

The backend seam needs no such file, because muster-daemon takes its settings as requests on its control connection
(the shape, above).

## Degradation

Health is per-connection *and* per-channel, and it is state, not an error path:

- **connected**: live control plane, live pane channels.
- **stale**: the control plane is silent or wedged (SSH up, daemon unresponsive), or a pane channel dropped. Render
  the last mirror and last frames, marked stale. Pane-channel recovery is a forced full repaint; control-plane
  recovery is a fresh snapshot. The two recover independently.
- **disconnected**: render the labeled last mirror; reconnect resyncs everything.

A daemon that has not answered by the time the window opens is the same case from the other end. The window waits
one second for the daemons its config names, each attached on a thread of its own, and opens without the rest; a
daemon that arrives later is reconciled into the open window like any first snapshot, and one that cannot be reached
is tried again on the reconnect backoff until it answers. Its part of the saved arrangement waits for it: the file
keeps that part while the daemon is on its way, and when it answers its tabs and halves of tabs go back where they
were, with their widths, without taking the keyboard from whoever is typing by then. Waiting on every daemon before showing anything made one
slow devenv a window that did not appear for a minute and a half.

Liveness needs an active probe - the control plane is legitimately silent when nothing happens - and how it probes
is an implementation choice. Version skew between Muster and a daemon is detected at attach and surfaced plainly.
Sessions survive anything Muster does: a broken Muster must never strand a session (see also geometry, above).

**A fourth state, and the one that is easy to get wrong: the daemon answered, and the session it describes is not the
one we knew.** A daemon that restarts brings back every tab, name and directory from its saved state, with a new shell
in each pane and none of the old processes, and says in a `restored` event what it could not bring back; a daemon
whose saved state it could not read starts empty (MIP-3, section 2). Every test for "connected" passes in each case,
and rendering an empty session as though the user closed everything is the worst available answer. This is a distinct
state with a distinct response - say what was there, offer to rebuild it - and it belongs to Muster because no daemon
can know what a window was showing.

**A connection dropping costs a pane its bridge, and the bridge is what has to come back.** The row below saying a
dropped connection loses nothing is about the daemon, which keeps running with the agent in it; the near side gives
up, because a remote bridge's stream rides the ssh forward and ends with the route. So a bridge that ended is replaced
while its daemon still holds its pane, and the interval between one ending and the next tells a connection that
blinked from a pane nothing will fix - three replacements inside half a minute and Muster stops and says why
(`crates/muster-core/src/respawn.rs`). Two things make that harder than it sounds. The exits arrive before the tunnel
is reported down rather than after, so nothing may key on the reopen. And the far machine refuses the replacement: a
pane has one bridge at a time, and the stream from before the drop stays open over there until its ssh notices, so a
replacement re-attaches with `--takeover`, which a first bridge never does, because the pane it would take could be
one another window is showing.

**A bridge whose stream the daemon dropped attaches again on its own, once, before any of that.** The daemon hangs up
on a bridge whose stream write makes no progress for 30 seconds, and a bridge stops reading its stream whenever its
surface stops reading the pty - which every surface in a window does while the window's main thread is paging, because
libghostty's readers wait on a mailbox only that thread drains. Nothing is wrong with that bridge or its surface, and
exiting would cost a new surface for each, so the bridge attaches again and the replay redraws what was missed
(`crates/muster-bridge/src/daemon.rs`). A stream lost again within ten seconds, or an attach that fails, ends the
bridge as before, saying the loss rather than the refusal, so the replacement policy above still judges a bridge that
keeps losing its stream.

**A pane can also stop painting while every layer below reports health: a pane that was asked for something and
painted nothing.** A wedged bridge, a transport that dropped without closing, a daemon still answering requests while
one of its terminals went quiet - each leaves the same picture and nothing to read it by (kan a_2LMRCug0P). The
trigger is an intent that actually reached the pane rather than silence itself, because an idle agent paints nothing
all afternoon and is perfectly healthy: what makes quiet wrong is that somebody typed. Neither process knows both
halves, which is why this is joined above the seam - the app is the sender, and the bridge is the only thing that sees
output reach its surface, so it says it painted on the control socket it already holds, at most four times a second
and not at all while nothing is arriving. Output inside that quarter second is reported when it ends rather than by
the next arrival, because the echo of the last keystroke before a pause has nothing after it, and waiting for more
accused a healthy pane on every pause (kan a_2PeXwg4fA). Two guards keep the sentence true: only while the window is
drawing the pane, and only while its daemon is answering. The first guard is about raising, not keeping: a warning
already raised stays when its pane leaves the screen, and goes only when the pane paints, closes, or its machine goes
away - so a warning that went away means one of those happened, not that somebody looked elsewhere (kan a_2LWqtPd8E).
It is a warning rather than an error - it may clear by itself, and a program that turned echo off for a password looks
exactly like this until it paints again.

**How the app finds out a bridge has died is Muster's own business, not the renderer's.** libghostty offers a
`close_surface` callback and it does not arrive: a dead pane sits on libghostty's own "Process exited. Press any
key" screen, which is the surface being held open rather than the host being asked to close it, so for two releases
the replacement policy above was written, covered by the corpus, and reachable from nothing (kan a_2IRcMjFs0). What
Muster watches instead is the socket it bound for the pane and the bridge dialed back on
(`crates/muster-seam/src/bridge_link.rs`). That connection ends when the process does, whether it exited, was killed,
or lost the machine it was running on, and it needs no cooperation from the renderer or the daemon. The bridge writes
one sentence there before it goes, saying which ending this was, because the answer differs: a refused attach is worth
taking the pane over for, a pane another bridge took over is not, and a pane the daemon says no longer exists -
which is what a bridge hears when its pane is closed under it - is worth no bridge at all. The renderer's callback is still wired, as a
second source rather than the one that matters, and a pane whose bridge is already known gone ignores the second
arrival.

**Every answer says what happened, and a decline is never dressed as a success.** muster-daemon answers each change
with one of four outcomes: done, already so, refused because the thing named does not exist, or refused for a stated
reason (MIP-3, section 9). The adapter reads the first two as success and the others as Muster's own `Refusal`, one
decision serving the keyboard and the CLI, because both reach the adapter by the same route. Already so is a success
because the state the caller asked for holds - a resize against a pane already at its limit, say - while a change that
did not happen is a refusal, and answering it with a success would tell the caller something untrue. When Muster ran
herdr, this took reading a reason herdr put beside an ordinary answer, since herdr answered a zoom, swap, move or
resize it had considered and not performed as a success, and the one symptom was a window that did not move.

## Durability

What survives what. Written down because "sessions outlive everything" reads as one guarantee and is really four,
and because the layer that can honestly answer each is different.

| | what is lost | who can help |
|---|---|---|
| Muster quits or crashes | nothing | nobody needs to; the daemon owns the PTYs, and holds the panes' permissions with them |
| the connection drops (VPN, lid, SSH) | nothing; the view goes stale and resyncs | the degradation model above |
| the daemon restarts | every process; scrollback; titles | the daemon restores its tabs, names and directories |
| the machine reboots | the same, plus the daemon must come back | as above |
| the machine is gone | local work only | remote daemons keep running |

**A tab that spans two machines is written down on both, so no tab depends on a file of Muster's.** A tab holding a
laptop pane beside a devenv pane is a part on each daemon, and each part carries the tab's one Muster name, so each
daemon restores its part under that name (MIP-3, section 2). Only the order and widths of the tab's regions are
Muster's, in the window's arrangement. That closed the weaker tier MIP-2 accepted, where the grouping lived in
`~/.muster/state/names.toml` and nowhere else.

Two states it answers either way. **A daemon restarts:** it returns every tab, each pane's name and each pane's
directory, not the processes, and a grouped tab is whole again as soon as both daemons have spoken. **One of a tab's
machines is unreachable:** the tab opens showing the panes it can reach rather than refusing to open, which is the
rule the mirror already follows for a stale daemon applied to a tab.

**The first row used to have an exception, and what closed it is the daemon being a helper application.** macOS
charges a protected request - a folder, the camera, AppleScript, the local network - to the *responsible* process.
For a pane's program that used to be the Muster which started its daemon, and only while that Muster was alive:
every surviving pane became its own responsible process the moment the app exited, a later Muster could not adopt
them, and a permission therefore behaved one way before the first relaunch and another way after it, under two
names and with no prompt saying which case you were in. Starting the daemon through Launch Services makes it its own
responsible process from its first instant, so every pane it ever makes is charged to one subject that outlives every
launch (`observations/macos-26.4.1.md`, section 8).

Two things it does not fix, and both are worth knowing before reading the row as absolute. A daemon restart is row
three and takes the permissions with the processes, because the new daemon is a new subject. And a build whose
signature changes is a new subject too, which is why an ad-hoc `./dev --bundle` still re-prompts and a Developer ID
does not.

Two things follow.

**Persist intent, never observation.** The mirror is an observation - these panes exist, this agent is blocked,
focus is here - and it is explicitly disposable. A restore description is an intent: make a right-split with these
two directories. Writing down observations would tempt a restore to reinstate things that are meaningless after a
restart (agent status, scroll offsets, revisions), so the mirror is deliberately **not** serializable and gains no
persistence hooks. What gets written down is what someone would ask for again.

This also resolves an apparent gap in "view = f(daemon state)": restoring looks like it needs an inverse, and does
not. The daemon writes down what someone would ask for again - tabs, trees, names, directories, settings - and never
an observation such as a title or an agent's state, then rebuilds from that file when it starts (MIP-3, section 2). A
window reads the result the way it reads any other daemon state.

**Muster's own durable state is composition, plus what the window looks like.** Composition is which daemons are
attached, which tabs the window holds and in what order, which of them is on screen, and how each divides between the
machines holding panes in it. Beside it, in the same file and under a table of its own, is the window's own chrome: whether the roster is open and how wide, how far the text is sized from what the
config asked for, how big the window is, and whether it is full-screen. Everything else Muster holds is derived.
That is a few hundred bytes, and its smallness is the point: the shell owns nothing, so there is nearly nothing to
save.

The two are kept apart because they answer different questions. A tab is a wish about a session that may have moved
on, checked against what the daemons turn out to hold - a machine that no longer holds its half loses that region,
and a tab that loses every region is not restored; a chrome setting has nobody to check with, so it comes back as it
went in. The window's frame is the one thing in either half that is checked against neither: the display
it was measured on may be gone, so the shell reports the screens it has and the core answers where the window should
open. That split is the same one `locale` draws - only a shell can ask the platform, only the core decides what to
do about the answer - and it is what keeps a rule about displays somewhere a test can reach.

Written as it settles rather than at quit, on both halves, because quitting is not how this is usually lost: the
whole durability table above is about crashes, reboots and dropped connections. What follows from that is a rule
easy to break in one line - **nothing writes the file before the window has opened**. A composition nobody has
opened is empty, and a shell reports its frame the moment the window exists, which is before it asks the core to
open anything.

**One arrangement per window, and a window claims one for as long as it runs.** They live under
`~/.muster/state/windows/`, one file each, and a window writes its pid beside the one it took, as a file that fails
to link when one is already there, so two launches cannot claim one arrangement. A launch drops the claims of
processes that are gone, and of pids macOS has since given to a process that started after the claim was written. It
then takes the most recently written arrangement nobody is holding - which is the
window Muster comes back to when none is running, and the window that was just closed when one is. `muster window
new` and ⌘N say `--fresh`, and a fresh window takes an arrangement nothing has ever held. Going to a closed window's
tab from another window names the arrangement outright, and that launch takes it.

Two things stand on that. The file has a single writer, where before every window shared one and whichever published
last decided what came back. And a window that closes leaves something to come back to, which is what `muster window
reopen` reads.

**Every tab belongs to exactly one window, and that is written down where every window reads it** (kan
a_2Mhi0EZlv). `~/.muster/state/holding/tabs.toml` says which window holds each tab, and every window reads, changes
and writes it inside a lock of its own, so two windows cannot write over each other's change. A pane is drawn by one bridge at a time, so a tab two windows both listed was a
tab whose panes the second took from the first at a click - and before this every window listed every tab, so a
window holding nothing drew the next tab anybody made. Now a window lists the tabs the record gives it and no others,
and its arrangement names only those.

A window here is its arrangement's name, `window-2`, not its process, so a window keeps its tabs across a quit and
`muster window reopen` comes back to them. Whether a window is open is asked by dialing its socket rather than read
off a pid. A tab a window asks for is recorded as its own before anybody else can take it: the window writes that it
is waiting on that machine before it asks, and one write takes the tab and clears the wait, because the daemon
announces a new tab to every window before the asking one hears its answer. A tab nothing asked for - made by another
client, or held by a window whose arrangement has gone, or every tab on the first launch after this existed - joins
the window that was in front most recently. The shell watches the record's directory and tells the core when it moves, so an idle
window costs no wakeups.

But composition is the piece nobody else can save. A daemon's saved state is scoped to itself: it can record that its
part of a tab carries a name, but not which window shows that tab, where the tab sits in the window's list, or how
wide each machine's region is, because no daemon knows the windows or the other daemons exist. Muster is the only
layer that sees across them, which makes that part of durability genuinely ours - and it follows that restore is
per-daemon and partial by nature, since after a reboot the local daemon comes back from its own file while the remote
one never noticed.

What Muster must not do here: keep its own session store, or infer an agent's resume token by reading its output.
The first is the multiplexer non-goal and the second is the agent-framework one. Reporting a session reference the
harness hands over is metadata about a pane and is fine; `muster-daemon report` already carries what a harness says
about its agent, and that is where a real "resume this agent" story lives - in the harness's own session, not in the
terminal.

## The diagnostic log

One run, one file, every process. The app names it, and every bridge it spawns inherits the path, so a keystroke
leaving the app and arriving at a daemon reads as consecutive lines rather than as a correlation exercise across
clocks. Records are one JSON object per line - time, level, process, pid, a dotted event name, then fields - which
makes the log greppable by hand and an assertion surface for tests: a launch smoke test can assert that
`channel.connected` appeared and that no `error` record did.

**It is that surface on purpose, and `./dev --contract` is what reads it.** Worth stating outright because the
alternative reading - that the log is a debugging convenience tests happen to use - would make a record's name and
fields free to change, and they are not. What a check asserts on is the answer to a question (`did the roster open
over a refused config`), and a record that stops answering it is a change to a contract rather than to a log line.
The line this does *not* cross is the one below: the log is not how a **person** is told something is wrong. That
mistake cost an evening - a config refused at 18:55 went to the log and nowhere else - and the roster is where a
person finds out. The two are different audiences for the same fact.

What makes it worth asserting on rather than a poor substitute for a real test is that it spans processes a test
cannot otherwise see at once: the app, its bridges and its daemon write to one file, so a check can state that a
gesture reached a pane without a window, a keyboard or a screenshot. Its limit is the same shape - it says what the
app *did*, never what it *drew*, so pixels, layout and legibility stay outside it and stay manual.

Events are named for the question they answer, not for the code that emitted them. The load-bearing ones are the
boundaries where a process can silently stop mattering: the control socket binding, a bridge dialing back, the first
frame painted, and the reason a pane's stream ended.

On by default in every build, and `MUSTER_LOG=0` turns it off. A terminal multiplexer's logs are unusually
sensitive, so what the user typed is recorded only under a separate switch: by default a keystroke record carries
its shape - which key, how many bytes - and not its content. The default must stay the one that cannot leak a
password into a file destined for a bug report. It used to be opt-in outside debug builds, which made no difference
while every bundle was one; the optimized bundles of 0.9.0 would otherwise have stopped writing the file bug
reports are made of.

Where the file lives is an OS question and therefore the shell's; nothing in the core knows the path. On macOS it is
`~/Library/Logs/muster/`, where Console lists it, with `latest.jsonl` naming the newest run. A run with `MUSTER_HOME`
set logs to `logs/` under that home instead, so an isolated run - a test, a bug being reproduced - neither mixes its
records into the person's own runs nor takes `latest.jsonl` from them. `MUSTER_LOG_FILE` names one file outright and
wins over both.

## Seams and test hooks

The injected edges, matching `testing.md`: the clock, and the renderer seam (tests feed pane channels through
libghostty-vt and assert the resulting grid). The backend connection is deliberately *not* one of them - tests
spawn a real muster-daemon built from the same commit rather than a stand-in, so the adapter is judged against the
daemon itself. What is injectable there is narrower and lives in the code's own shape: the message reader takes a
reader rather than a socket, so a recorded stream can be cut anywhere, and the connection loop takes a socket path,
so a killed daemon is the disconnect case. The perf harness measures at the same edges, at 1 and 15 panes (desiderata budgets).

## Deliberately open

Left to implementing agents with better information at build time:

- The concurrency mechanism, as long as the event-model property holds.
- Wire framing of the app's CLI and IPC endpoint.
- Reconciliation cadence and the liveness probe.

Project-level undecideds - the language split, optimistic UI, reproducible presentation
state - are tracked on the kan board.
