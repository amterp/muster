# macOS permissions at 26.4.1

Who macOS holds responsible for a pane's program, and what that does to a permission somebody
grants Muster.

Measured 2026-08-17 on macOS 26.4.1 / arm64 by `tools/tcc-probe/probe`, which builds a
throwaway app around Muster's process arrangement and records what each part is charged to.
The transcript is `corpus/macos-26.4.1/responsible-process.txt` and every claim below reads off
it. Raised by kan `a_28Tme94w3`, which reported prompts attributed to Muster.app that did not
stay granted.

## 1. The responsible process, and where it goes

macOS charges a protected request - a folder, the camera, AppleScript - not to the process
making it but to the *responsible* process, which is the nearest bundled app up the ancestry
when the chain was formed. Muster's arrangement is three deep: the app starts the daemon, the
daemon owns every pane's program.

| moment | process | responsible for it |
|---|---|---|
| app running | the app | itself |
| app running | the daemon it started | **the app** |
| app running | a pane under that daemon | **the app** |
| app gone | the same daemon | **itself** |
| app gone | the same pane | **itself** |
| app gone | a pane made afterwards | **itself** |

The first three rows are the arrangement working as the card assumed, and they are the good
case: while the Muster that started the daemon is alive, every pane in the window is charged to
Muster.app, so granting once covers all of it. It is also what every other terminal does -
anything run inside Ghostty or Terminal prompts as Ghostty or Terminal.

**The last three are the finding.** When the app exits, responsibility does not pass to the
daemon and does not stay pointing at the dead app: each surviving process becomes its own
responsible process. A pane the daemon makes afterwards is its own too, because the daemon now
is.

## 2. What that costs Muster specifically

Sessions outliving the app is the point of this project, so the second half of that table is
the ordinary case rather than a corner one. Quit Muster, reopen it, and the panes come back -
but they are now charged to themselves.

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

## 3. It also explains the screen-recording incident

While a Muster is running, `screencapture` from inside one of its panes is charged to
Muster.app. Re-signing the app - which `./dev --bundle` does, ad-hoc, on every build - changes
its code identity, so the grant no longer matches and screen recording stops working for
*everything in every pane*, not just for Muster. That was reported as a whole terminal losing
its permission for no visible reason, and this is the mechanism.

## 4. Every worktree that builds is the same app, as far as TCC is concerned

Follows from section 3 rather than being separately measured, and worth stating because this
repo is developed in several worktrees at once. `BUNDLE_ID` is one constant, so every checkout
that runs `./dev --bundle` produces an app claiming `dev.amterp.muster` with a code identity of
its own. TCC has one entry for that identifier, and whichever build it was recorded against is
the only one it matches.

So a slot rebuilding its own bundle is indistinguishable, from the outside, from somebody's
running app being replaced - and the symptom is the one in section 3: things stop working in
panes, with nothing on screen connecting the two. Recorded on 2026-08-17: after a bundle build
in a second worktree, `screencapture` from inside a pane failed with `could not create image
from rect`, while `~/Library/Application Support/com.apple.TCC/TCC.db` showed no write for the
preceding hour and three quarters. No write is what a code-identity mismatch looks like - the
stored row is untouched and simply stops matching - so that is consistent with the mismatch and
does not on its own prove which build caused it.

`tccutil reset ScreenCapture dev.amterp.muster` clears the entry so the next request prompts
again, which is the way back.

## 5. What a stable signing identity does and does not buy

The card's first item, and it is necessary rather than sufficient. `codesign -dv` on the bundle
reports `Signature=adhoc` and `TeamIdentifier=not set`; TCC keys such a grant to the cdhash,
which changes on every build, so every rebuild is a new app and every permission is asked for
again. A self-signed certificate fixes exactly that, and it is a step in the keychain of
whoever is building rather than a change in this repo.

What it does not fix is section 2. Panes whose Muster has quit are charged to themselves
whatever Muster is signed with, so a stable identity makes the good case stay good and leaves
the common case untouched.

## 6. The measurement's own trap, worth knowing before re-running

`responsibility_get_pid_responsible_for_pid` is the call TCC consults, and unentitled it
answers questions about *other* processes by returning the pid it was given. That is
indistinguishable from a truthful "this process is its own responsible process", and it read
as a finding: asked about a real running Muster and its daemon, every answer was "itself",
which would have said the card's premise was wrong. Every line in the transcript is a process
asking about *itself*, which is the case that always answers truthfully.

The same shape of caution applies to launching the probe: a binary started from a terminal has
no bundled ancestor, so all six rows read "itself" and the run looks meaningful while measuring
nothing. `tools/tcc-probe/probe` uses `open` for that reason.

## 7. What this leaves open

Whether the daemon should be started by launchd rather than by the app. It would not put
Muster's identity on panes - a launchd-started daemon is its own responsible process from the
first moment - but it would make attribution *consistent* instead of Muster-while-this-app-
lives and then the-pane-itself. Consistency is worth something on its own here, because the
present behaviour asks for the same permission twice under two names. Against it: the daemon
stops being Muster's own child, which is what makes "Muster ships its daemon and talks to no
other" simple to enforce (`architecture.md`), and it adds a job somebody has to install.

Carried on the board rather than decided here.
