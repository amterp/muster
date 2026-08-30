# macOS permissions at 26.4.1

Who macOS holds responsible for a pane's program, and what that does to a permission somebody
grants Muster.

Measured 2026-08-17 and again on 2026-08-30, on macOS 26.4.1 / arm64, by `tools/tcc-probe/probe`
- which builds four arrangements of Muster's process shape and records what each part is charged
to. The transcripts are `corpus/macos-26.4.1/responsible-process.txt` and
`local-network-from-here.txt`, and every claim below reads off one of them. Raised by kan
`a_28Tme94w3`, which reported prompts attributed to Muster.app that did not stay granted, and
settled by `a_29i4bxafd`, which asked what should start the daemon.

## 1. The responsible process, and where it goes

macOS charges a protected request - a folder, the camera, AppleScript, the local network - not to
the process making it but to the *responsible* process. Muster's arrangement is three deep: the
app starts the daemon, the daemon owns every pane's program.

What decides the answer is how the daemon was started. Four arrangements, measured together:

| the daemon is | started by | daemon is charged to | every pane is charged to | a prompt is headed |
|---|---|---|---|---|
| a bare binary | the app | the app, then itself | the app, then itself | Muster, then nothing |
| a bundle's executable | the app | the app, then itself | the app, then itself | Muster, then nothing |
| the same bundle | Launch Services | **itself, throughout** | **the daemon, throughout** | **the daemon, always** |
| a bare binary | a launchd job | itself, throughout | the daemon, throughout | nothing, always |

The first row is what Muster does today, and its first half is the arrangement working: while the
Muster that started the daemon is alive, every pane in the window is charged to Muster.app, so
granting once covers all of it. It is what every other terminal does - anything run inside
Ghostty or Terminal prompts as Ghostty or Terminal.

**The second half of that row is the finding.** When the app exits, responsibility does not pass
to the daemon and does not stay pointing at the dead app: each surviving process becomes its own
responsible process. A pane the daemon makes afterwards is its own too, because the daemon now
is.

**Being a bundle does not by itself change anything, and that is the second row.** A bundled
daemon spawned the way Muster spawns one today is charged to the app exactly as a bare binary is.
After the app exits the daemon becomes its own responsible process *and keeps its bundle name*,
which is a real difference for the daemon and none at all for a pane: the panes under it are
still charged to themselves, still nameless. Responsibility is inherited from the spawning
process, not read off the executable.

**Launching through Launch Services is what changes it, and that is the third row.** A daemon
started with `open` is its own responsible process from its first instant, with `ppid=1`, and
every pane it ever makes is charged to it - before the app exits, after the app exits, and for
panes made long afterwards. One subject, permanently, and a subject with a bundle behind it.

The fourth row does the same and has nothing to put a name to, which is section 8.

## 2. What that costs Muster specifically

Sessions outliving the app is the point of this project, so the second half of the first row is
the ordinary case rather than a corner one. Quit Muster, reopen it, and the panes come back - but
they are now charged to themselves.

- **A prompt from a pane names the program, not Muster.** After a relaunch, a request from an
  agent is attributed to `node`, `claude`, or whatever binary the pane is running.
- **A grant given to Muster.app does not cover it.** The subject is different, so the answer
  somebody already gave is not consulted.
- **A later Muster cannot take it back.** Responsibility is fixed when a process is spawned,
  and there is no call to adopt another process's. `posix_spawnattr` can only *disclaim* -
  hand responsibility down - so nothing lets an app claim a daemon it did not start. Attaching
  to a surviving daemon changes nothing about the daemon's own attribution, and therefore
  nothing about any pane it will ever make.

So the answer to the card's question - re-prompt, deny, or carry the grant - is **re-prompt,
under a different identity each time**, with each grant landing on whatever binary happened to
ask. That is worse than any of the three, and it is invisible: the prompt looks ordinary and
names something plausible.

## 3. Local Network, a denial that does not look like one

The fourth kind of protected request, and it fails worse than the three above. A folder or camera
denial is recognizable as a denial. This one arrives as a network error: every mDNS advertisement
returns `OS Error 0x02000041: No route to host` - `EHOSTUNREACH` on the multicast send - and
nothing says "permission". Reported from a Matter controller built in a pane, where interfaces,
routing and `IP_MULTICAST_IF` were all investigated first, because that is what the error points
at.

**Two things make it hard to recognize, and both are worth knowing before chasing one.**

`dns-sd -B` keeps working throughout, so the obvious sanity check confirms the wrong thing. It
asks mDNSResponder rather than sending multicast itself, so "mDNS works on this machine" is true
and says nothing about whether your own process can advertise.

And the error names routing, which is why the probe sends a second datagram beside the first.
From a pane on 2026-08-30, in one process at one instant
(`corpus/macos-26.4.1/local-network-from-here.txt`):

```
this shell      pid=61578 ppid=61530 responsible=61578 prompts-say=nothing - no bundle
local network   multicast=REFUSED errno=65 No route to host  unicast-to-gateway=sent
```

The datagram to the LAN gateway went. The one to the mDNS group on the same LAN did not. Routing
is not what refused it, and `tools/tcc-probe/probe --here` is how to establish that in five
seconds rather than an afternoon.

**What the arrangements do not show is which of them can obtain the permission.** Across three
runs on 2026-08-30 the same arm answered differently: everything refused on the first, everything
succeeded by the third. What a subject has already been granted moves between runs and the
arrangement does not, so section 1 is where the finding is and this section is about recognizing
the symptom.

**Muster already words this prompt and it already cannot be read.** `dev`'s
`usage_descriptions()` declares `NSLocalNetworkUsageDescription` among nineteen strings, and
macOS reads a usage string from the *responsible process's* bundle. In the post-relaunch case
that process is a bare `python` with no bundle, so there is no Info.plist to read any of them
from. The wording work is intact and simply not consulted.

## 4. It also explains the screen-recording incident

While a Muster is running, `screencapture` from inside one of its panes is charged to
Muster.app. Re-signing the app - which `./dev --bundle` does, ad-hoc, on every build - changes
its code identity, so the grant no longer matches and screen recording stops working for
*everything in every pane*, not just for Muster. That was reported as a whole terminal losing
its permission for no visible reason, and this is the mechanism.

## 5. Every worktree that builds is the same app, as far as TCC is concerned

Follows from section 4 rather than being separately measured, and worth stating because this
repo is developed in several worktrees at once. `BUNDLE_ID` is one constant, so every checkout
that runs `./dev --bundle` produces an app claiming `dev.amterp.muster` with a code identity of
its own. TCC has one entry for that identifier, and whichever build it was recorded against is
the only one it matches.

So a slot rebuilding its own bundle is indistinguishable, from the outside, from somebody's
running app being replaced - and the symptom is the one in section 4: things stop working in
panes, with nothing on screen connecting the two. Recorded on 2026-08-17: after a bundle build
in a second worktree, `screencapture` from inside a pane failed with `could not create image
from rect`, while `~/Library/Application Support/com.apple.TCC/TCC.db` showed no write for the
preceding hour and three quarters. No write is what a code-identity mismatch looks like - the
stored row is untouched and simply stops matching - so that is consistent with the mismatch and
does not on its own prove which build caused it.

`tccutil reset ScreenCapture dev.amterp.muster` clears the entry so the next request prompts
again, which is the way back.

## 6. What a stable signing identity does and does not buy

Necessary rather than sufficient. `codesign -dv` on an unsigned build reports `Signature=adhoc`
and `TeamIdentifier=not set`; TCC keys such a grant to the cdhash, which changes on every build,
so every rebuild is a new app and every permission is asked for again. A Developer ID fixes
exactly that, and `MUSTER_SIGN_IDENTITY` is how a release gets one.

What it does not fix is section 2. Panes whose Muster has quit are charged to themselves whatever
Muster is signed with, so a stable identity makes the good case stay good and leaves the common
case untouched. Section 8 is what fixes that half.

## 7. The measurement's own traps, worth knowing before re-running

`responsibility_get_pid_responsible_for_pid` is the call TCC consults, and unentitled it
answers questions about *other* processes by returning the pid it was given. That is
indistinguishable from a truthful "this process is its own responsible process", and it read
as a finding: asked about a real running Muster and its daemon, every answer was "itself",
which would have said the card's premise was wrong. Every line in the transcript is a process
asking about *itself*, which is the case that always answers truthfully.

The same shape of caution applies to launching the probe: a binary started from a terminal has
no bundled ancestor, so every row reads "itself" and the run looks meaningful while measuring
nothing. `tools/tcc-probe/probe` uses `open` for that reason.

The `prompts-say` column is derived rather than measured. It is the responsible process's own
bundle name, which is what macOS puts in a prompt's heading; a process that is its own
responsible process and has no bundle is one nothing can put a name to.

## 8. What starts the daemon, decided

**Launch Services starts it, from a helper bundle Muster ships inside its own.** That is row
three of section 1: the daemon is its own responsible process from its first instant, every pane
it ever makes is charged to that one subject whether or not a Muster is running, and the subject
is a bundle - so macOS has a name for the prompt heading and a usage string to read out of its
Info.plist. Muster still starts the daemon, from the binary it shipped, on the socket it owns, so
`architecture.md`'s "Muster ships its daemon and runs it, and talks to no other" is untouched and
there is nothing for anybody to install.

**launchd was the proposal and it loses on the name.** A launchd job makes attribution just as
consistent, and its subject is a bare binary. Nothing can head a prompt with it, and a missing
usage string is not a worse prompt but a kill - macOS terminates a process that makes a protected
request when the responsible app declares none, which is why `dev` declares nineteen of them. It
also has to be installed, listed in Login Items & Extensions, and announced by macOS the first
time ("probe-bare can run in the background", measured while probing this), on a project whose
answer to "how do I get herdr" is "you don't".

**Bundling the daemon without changing who launches it does nothing for panes**, which is row two
and was the cheapest thing that could have worked. Responsibility is inherited from the spawning
process rather than read off the executable, so a bundle spawned by the app is charged to the
app, and its panes go on being charged to themselves once the app exits.

**Re-exec'ing an orphaned daemon on relaunch** would give consistent attribution by making the
daemon young again, and costs every live process to do it: a daemon restart keeps the pane tree,
the pane ids and each cwd, and loses every terminal id, all scrollback, and every running agent
(`herdr-0.8.0.md`, section 12). That is the one thing the durability guarantee exists to prevent.

Two costs come with the answer rather than being arguments against it. `open` takes an
environment as repeated `--env` flags rather than replacing it, so the daemon starts from
launchd's GUI environment with Muster's allowlist laid over the top instead of from `env_clear`;
that is the same environment a Dock-launched Muster gets, and it is what `daemon::carried`
already assumes. And `open` returns before the daemon does, so a daemon that dies at startup no
longer reports its exit status - `--stderr` to a file Muster names in the run log is what
replaces it.

**Confirmed on the real app rather than only on the probe.** A bundle built from this change,
launched into a scratch home, started its daemon from
`Muster.app/Contents/Library/MusterSessions.app` with `ppid=1`, and a program run in one of its
panes reported `responsible=` that daemon's pid. The daemon's own environment held fifteen
variables, none of them the launching agent session's.

That last part is a trap worth naming, because the first attempt fell into it. `open` hands the
app its own environment and applies `--env` on top, so unlike the spawn's `env_clear` it carries
whatever launched Muster: measured, ninety-seven variables including the launching agent's
`CLAUDECODE` and a `HERDR_SOCKET_PATH` pointing at somebody else's daemon, which herdr obeys over
everything else. That is the bug `a_28YgGqYq7` fixed arriving again through a different door, and
it is invisible - the window opens and works. Clearing the environment of `open` itself is what
stops it, and it leaves exactly what launchd gives any GUI process.

The SSH case is untouched. A remote daemon has no TCC subject on this machine, and `remote::start`
keeps starting it with `nohup`.
