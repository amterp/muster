# Testing

Code written largely by AI agents needs the suite, not the author's memory, to be the proof it works. Classic
integration-test discipline - real internals, fake edges, deterministic, asserted black-box - is the substrate, but a
native GUI over daemons we do not own bends it in specific ways, and the neighbors show how. ghostty's Zig core
carries hundreds of test blocks (324 in Terminal.zig alone) while its macOS shell ships none: even the best
native-app team treats the AppKit layer as untestable and survives by keeping it thin. herdr's ~3,500 test functions
name their integration suites after user-visible behaviors (detach_reattach, multi_client, live_handoff) and feed
them recorded corpora (keyboard_protocol_corpus.tsv, per-OS terminal-variant tables). cmux's CI once passed with
every test silently skipped ("Executed 0 tests"); they now lint for it.

Muster's principles, adapted to that evidence:

- **Thin shell, thick core.** Testability is structural, not bolted on (a rule borrowed from
  [radish](https://github.com/amterp/radish)). Every decidable behavior - layout mirroring, input translation, state
  tracking, intent - lives in a headless core with no I/O, no clock, no OS types. The shell only wires OS events in
  and surfaces out, so its failure modes are wiring failures, covered by a small smoke layer. If something is hard
  to test, it is in the wrong layer: move it, don't mock around it.
- **Do not fake the backend. Run a real one.** This started as "fake only the seams, and audit the fakes against
  reality," on the reasoning that a real daemon would be too slow for the default gate. Measured, that reasoning was
  wrong: herdr, the daemon Muster ran first, cost 25 ms to spawn and answer, a session snapshot 0.7 ms, and a full
  mirror bootstrap - snapshot, subscribe, first event - 0.7 ms. Thirty repeated runs across serial and parallel
  execution produced no failures. muster-daemon answers its first request in about 4 ms. The seconds-per-test cost
  worth fearing belongs to agent *detection*, which reads a pane's screen: a test that needs an agent state pays
  the detector's three-second startup grace once per pane, and only those tests pay it.

  So the backend seam is not faked at all. Tests that need a daemon spawn a real one through
  `crates/muster-harness` - one daemon per test, killed on drop including on a panic, isolated by a scratch root
  holding its socket, its HOME and its log. What this buys is the removal of a whole category: there is no
  hand-written daemon in this repo, so there is nothing to drift, and "a drifted fake daemon is Muster's top
  false-green risk" stops being a risk we manage and becomes one we do not have.

  It also catches what a stand-in cannot. Building the subscription against real herdr turned up two facts no
  invented one would have contradicted: a subscription is requested by a dotted name and answered with a snake one,
  and half-closing the write side - which is how every other herdr call signals it is finished - ends a subscription
  on the spot. Both fail as silence rather than as an error, which is the shape of bug a fake is worst at.

  The daemon is built from the same commit rather than pinned: the daemon's own tests hand the harness
  `CARGO_BIN_EXE_muster-daemon`, and cargo gives that only to the daemon's own package, so a test anywhere else -
  the seam's, the CLI's, the bridge's - calls `Daemon::start_built()`, which finds the daemon in the target
  directory the test runs from. `./dev` builds the workspace before testing, so that daemon is
  from the same commit; a narrowed `cargo test -p` builds only its own package and gets whichever daemon was built
  last. The request builders the daemon's tests read with, such as `make`, `beside` and `until_text`, are in
  `muster_harness::requests` for the same reason. A spawned
  one answers its first request in about 4 ms, and a test holds that under the 25 ms that keeps daemon-backed tests
  in the default gate. A Muster config naming the daemon is `muster_config()`, and a window's test points the seam
  at it the way a person's file would. Agent state is driven the way a real agent drives it:
  `Daemon::start_detecting()` gives the daemon a home holding the herdr probe's fake agent under the name `claude`
  and an override manifest that reads its markers, `run_agent` starts it in a pane, and `set_agent_state` tells it
  what to paint - so no test sets a state the detector did not reach. Beside the control connection it drives the other
  two the way a bridge and the app will: a `Stream` that attaches and gives or withholds credit, and an `Input`.
  What a pane's program received is read from inside it - a program in raw mode copying its input to a file - and
  the bytes it should have received come from libghostty's own encoders, configured from a terminal fed the same
  modes, so no test spells an escape sequence the encoder is the authority on. The flood test holds the structure
  that keeps one pane's flood from delaying another's echo and times nothing; an ignored test beside it prints the
  latency. `./dev --latency` times the same keystroke through the daemon with `crates/muster-latency`: the pane's
  stream read directly, and the real bridge (`muster-bridge --daemon-socket`) onto a PTY where a surface would be,
  beside the bare PTY measured in the same run, idle, in a window of fifteen panes and beside a pane flooding into
  a surface that reads slowly. It starts its own daemon from `target/release`, directly rather than through Launch
  Services, until stage 3 puts the daemon in a helper bundle. `--socket` measures a daemon already running instead,
  which is how a devenv's is measured through a forwarded socket until the SSH tier installs one itself, and
  `--flood-surface fast` floods a surface that keeps up, so the flood's time is what the link and the daemon's flow
  control allow. Agent detection is checked the way the herdr probe checked herdr's: its `detection` scenario runs
  again against the daemon, the same fake agent and override manifest, and prints each state's settle time beside
  the one recorded in `corpus/herdr-0.8.0/detection/`.

  A lost answer is staged the same way. `Daemon::withholding_answers_where` puts a relay in front of the real
  daemon that passes every connection through and, for the requests a test picks out, reads the daemon's answer and
  never delivers it. The daemon does the work and every byte a caller receives is one it sent, so what is staged is a transport
  fault - the thing a loaded machine produces and no request can ask for - rather than a daemon of Muster's
  invention.

  Nothing consults PATH for a daemon: a test that resolved its own could quietly run against one nobody built
  from this commit. herdr is still pinned (`deps/herdr.pin`, fetched into `deps/herdr/` and verified) for the
  app the shell builds, the contract and latency tiers and the corpus probe until they move to muster-daemon, and
  no Rust test runs it.
- **Detect wire drift mechanically, not by waiting for a test to fail.** herdr generates a canonical JSON Schema of
  its whole API from its own request types, fails its own build when the two disagree, and embeds it in the binary
  (`herdr api schema --json`). A copy sits in `corpus/herdr-<version>/api-schema.json`, and `./dev` diffs the two
  before running anything. A daemon that changed its wire is named as such, with the diff, instead of surfacing as
  a puzzling failure three layers up.

  muster-daemon, which replaces herdr (MIP-3), is built from this repo, so its wire cannot drift from the code under
  test. What can drift is the wire between two builds, because an app adopts whichever daemon is running. So
  `proto/muster_daemon.v<major>.baseline.proto` records the schema as its last minor version was published, and a
  test in `muster-daemon-proto` fails when a field's number or type moved, a number was freed without being
  reserved, or the schema changed without its minor version moving past the baseline's.
- **Inject at the seams the code already has, not by impersonating a daemon.** Three different things get called
  fault injection, and only one needs machinery. *Daemon state* - a blocked agent, fifteen panes, a pane whose
  program died - is driven through the daemon's own requests and the fake agent, which can produce all of it on
  request. *Daemon-internal
  timing* is not injectable at all, so nothing may depend on it. *Transport faults* are the real case, and they
  enter at two places: a parser that takes a reader rather than a socket, fed recorded bytes cut wherever a test
  wants, covers truncation, split reads and malformed lines offline; and process control covers the rest, since
  killing a real daemon ends a held-open subscription in 0.8 ms. A proxy that corrupted real traffic was
  considered and rejected - it would reintroduce byte-level protocol emulation, which is the thing being deleted.
- **Record reality, replay it as data.** Oracles come from capture, not belief: ANSI streams from real agent
  sessions, key encodings from real terminals, daemon event logs. Cases are text files a reviewer can read, in the
  style of [go-snap](https://github.com/amterp/go-snap); adding coverage means adding data, not test code.
- **Cases outlive implementations.** The core's tests are a conformance suite: one corpus of cases, and a thin
  driver per language that feeds them in and compares what comes out. The shape is Web Platform Tests', or
  CommonMark's - and herdr's own `keyboard_protocol_corpus.tsv`, which puts input and expectation in the same row.
  What this buys is that a core rewritten in another language (MIP-1) is verified by cases a working implementation
  already passed, rather than by reading the old tests and hoping. It is the same argument as the backend contract
  corpus, one layer further in: the corpus is the executable definition of what a replacement must provide.
  Roughly four fifths of the suite fits this; the shell's does not and should not try (see below).

  That port is done, so there is one driver again rather than two, and the cross-language check the corpus was
  briefly performing is gone with it. Worth saying plainly: what remains is a suite of readable cases that outlived
  a rewrite, which is what it was for. The next thing to run them will be a second backend or a second shell, and
  the cases are already waiting.
- **Assert what the user sees and what the daemon receives.** The user-facing oracle is the terminal grid, computed
  in the harness by libghostty-vt - the production engine. The daemon-facing oracle is the exact intent messages on
  the wire. Never pixels (GPU-flaky), never internal structures (false confidence in both directions).
- **Deterministic or it does not merge.** Injected clock, event-driven waits (`events.subscribe`,
  `pane.output_matched`), no sleeps, nothing reaches the network. Async byte streams are replayed, never raced. A
  real daemon does not weaken this: what makes a test flaky is waiting on wall-clock time, not talking to a
  process, and herdr's own integration suite is built the same way.

  **"No sleeps" means no fixed wait standing in for a condition**, and two things in the suite look like sleeps
  without being one. A poll interval inside a deadline-bounded `until` is not a wait - what the test waits for is
  the condition, and the deadline only decides how long it takes to fail. And *proving a negative* needs elapsed
  time by construction: `split_sides.rs` waits past herdr's own second publish, measured at 104.5 ms, so that a
  mirror which merely got there first and then walked backwards fails rather than passing on timing. There is no
  event for "nothing further arrives". Both are legitimate; both need a measured number and a comment saying which
  measurement, because a wait sized by guesswork is the flake this rule exists to prevent.

  **There is one `until`, in `muster-harness`, and it has one deadline.** There were twenty-four, one per test file,
  because the way a test gets written is by copying the nearest one - and they had drifted to deadlines of two, ten,
  fifteen, twenty and thirty seconds, with not one of the outliers saying why. A single number is the honest answer
  because a deadline here bounds a failure rather than tuning anything: a genuine wedge shows up as runs that either
  finish well inside a second or sit at exactly the deadline, so no value would have made the difference and only
  the shape of that distribution says what is wrong. A wait that truly needs longer takes `until_within` and states
  its reason at the call site, which is the one place a reader can check it. Every wait also carries a slot for what
  was true instead, because a timeout saying only that a condition never came true sends whoever hit it back to add
  exactly that and run again.

  **The distribution is measurable, so argue from it.** Four cards accumulated arguing whether daemon-backed tests
  wedge or run out of room, and each argued from a stopwatch held around `cargo test` - which times the build, the
  process start and the wait together. On a loaded machine that is almost all process start: the measurement that
  read as 2.4 seconds of headroom under a 20-second deadline was a 12 ms wait inside a 10-second invocation.
  `MUSTER_WAIT_LOG=1` makes every wait record what it cost and `tools/wait-margins.py` reads them back, and the
  answer that ended the argument was that the whole family had over 19 seconds of margin and was failing on a
  **500 ms** socket timeout three layers down, which is why four rounds of looking at deadlines never found it. A
  deadline here is not a suspect until the numbers make it one.
- **Tiered by what a tier can reach, not by what it fakes.** Most of the core is pure - a keymap, a fold over
  events, a byte-stream parser - and needs no daemon in any tier, so those stay microseconds. Tests that need a
  daemon spawn one and stay in the default gate, because 25 ms is not a tier boundary. What remains genuinely out
  of the gate is what needs something a developer's machine cannot be assumed to have: `--contract` needs a
  logged-in GUI session to launch the app - and to draw the one Swift test that stands up a real libghostty
  surface, `ClickRedriveTests`, which skips itself in an ordinary run and says so - `--latency` and `--perf`
  measure timing and would be flaky as
  assertions (`--latency` prints verdicts against MIP-3's targets and fails only when it cannot measure),
  `--corpus-linux` and the SSH tier need the devenv container - where the SSH tier also puts this build's Linux
  muster-daemon, at the path a remote machine keeps it, since Muster does not yet copy it over itself - `--linux`,
  which runs the daemon's and detection's suites on Linux, needs docker, and `--claude-code` needs the network and
  a model: it drives the Claude Code installed here for one turn, in a pane with Muster's hooks and one without,
  and checks both read working and then idle. It runs with `ANTHROPIC_API_KEY` and `--bare` when that is set, and
  otherwise with `claude`'s own login and only project settings, so nobody's own hooks take part; with neither it
  fails and says which is missing. The gate still compiles the Linux daemons and lints their Linux code,
  so what `--linux` alone catches is behavior: dash as `/bin/sh`, `/proc`, `close_range`. That is the real line, and it is
  narrower than the one drawn when the backend was going to be faked.

  **A tier that measures time reads the machine before it judges one.** Everywhere else a loaded machine only
  makes a run slow; in `--perf` it makes the run lie, because those numbers are judged against a checked-in
  baseline and a benchmark that spent its time waiting for a core reports a regression nothing in the code caused.
  So `--perf` refuses above a one-minute load average of the machine's *performance* core count - past that many
  runnable threads the scheduler starts handing work to the slower cores - and `--perf --anyway` measures without
  gating for anyone who wants the numbers regardless. Refusing is defensible there and nowhere else, so nowhere
  else does it.

  Every run says what the machine was doing, narrowed flags included, because the failure this guards against is
  not a red run but a plausible one: seventy-four minutes of a machine with seven of its ten cores eaten by
  orphaned background loops read as nothing worse than a gate taking half an hour, which is not obviously wrong
  for a run that builds two toolchains. The baseline records the same sample, so a comparison can say whether the
  two runs were measured under conditions that resemble each other at all. `./dev --doctor` is the other half:
  load says the machine is busy, and the doctor says what is on it - the daemons and containers this repo's own
  tooling leaves behind, and what is currently eating the CPU.
- **One test binary per crate.** A crate's integration tests are modules of `tests/<name>/main.rs`, beside the
  `support` module they share, rather than a binary per file. Every binary links the crate and its dependencies
  again, and macOS scans each new one before its first run, so 154 of them spent much of a gate linking and listing
  rather than testing. What it costs is a shared process: whatever one test sets for the process, every test in its
  binary sees. So no test sets the environment. The settings the seam reads from it for the suite's sake are set
  through `muster::testing` and put back by `fresh_session`, and a test that uses the seam's one session takes that
  turn first; a bridge test's `Typing` takes it for you. A test that needs a process of its own stays a top-level
  file, which cargo builds as its own binary, and says why at the top: `muster-seam/tests/named_daemon.rs` is the
  one. A file without its `mod` line compiles to nothing and the gate stays green, so `tools/test-mods.py` fails
  the gate on one.
- **A Swift test that points the seam somewhere holds it while it does.** `Core.dispatcher` is one mutable global
  for the process, so a test that swaps it is writing where every other test reads. These tests all run on the main
  actor and so are never truly concurrent - but a test that awaits gives the actor up, and another test's recorder
  can be installed underneath it before it comes back. A find test stayed flaky that way until its author stopped
  awaiting between setting the global and using it, and the failure read as the feature under test being broken
  (kan a_2LMRCjcSV). So `@Test(.ownsTheSeam)` - or one annotation on the suite - holds the seam for the length of
  one test and puts back what it found, and `seam(_:)` is the only door: it reports a test that swaps the global
  without holding it, rather than leaving the next flake to say so. What that gives up is interleaving rather than
  parallelism, which is the bug rather than the throughput.
- **The suite proves itself.** A bug fix lands as a failing test first, then the fix - two commits, so CI shows red
  then green (cmux's discipline). Guard against silently skipped tests. Performance is measured against cardinality
  budgets separately; a functional green is never a performance claim.

The seams these tests inject at, and the oracles they read, are defined in `architecture.md` (seams and test hooks).

## The conformance corpus

Three things go wrong with tests-as-data, and each is answered by something that fails the build rather than by
good intentions.

**The reasoning evaporates.** A test named "an arrow is handed to the daemon, not encoded here", with a comment
explaining that application cursor mode is invisible from the app and a guess produces bytes a pager rejects, is
the best documentation in this repo. A row in a table is not. So every case carries a `why`, and a driver **fails
a case whose `why` is missing or empty** - the same standing as an empty suite. Cases are JSON, one file per
concept, because prose survives it and both languages parse it without a dependency.

**A wrong oracle gets agreed on twice.** If the corpus is wrong, every implementation passes and every
implementation is wrong. Each file declares its `source`:

- `recorded` - captured from real herdr, real libghostty-vt, a real terminal. Carries the command that regenerates
  it, so it can be re-derived rather than believed.
- `ported` - lifted from an existing suite. Trusted exactly as far as that implementation was, which is the honest
  label for most of an extraction.
- `authored` - our own policy, with a citation. Muster's keymap defaults have no oracle beyond ghostty's config;
  saying so is better than implying a verification we do not have.

And the rule that makes the corpus a spec rather than a record of one implementation's habits: **when two
implementations disagree on a case, the corpus is never edited to match whichever is louder.** The answer comes
from a recording or from a dependency's source, and the commit says which.

**A file whose subject is narrower than JSON says so.** JSON has one number type, and a case written `0.05`
against a quantity the wire carries as an `f32` never matches - the driver answers `0.05000000074505806`, and the
only decimals left writable are the ones binary spells exactly. Putting the long form in the file would fix the
comparison and cost the corpus the thing it is for. So a file may declare `numbers` as `f32`, and both sides are
narrowed before they are compared. Declared per file rather than assumed everywhere, because narrowing is a loss:
under `f32`, two numbers differing past the seventh digit become one.

**A red suite becomes a scavenger hunt.** A failing row is worse to debug than a failing named test unless the
driver is built for it. Failure output names the file, the case, the `why` - included precisely because that is
the moment it is needed - then input, expected, actual, and the first difference. Bytes render readably: `ESC [ A`,
not `[27, 91, 65]`. The driver's own output is tested, like any other thing whose failure mode is silence.

And the hazard all three share: a corpus no driver reads is the silently-skipped suite in a new costume. So the
gate checks that every corpus file is claimed by a driver, that every driver reports how many cases it ran, and
that the count is never zero.

**Snapshots are oracles too**, so they live beside the cases in `corpus/snapshots/` rather than under one
language's tests. Some behavior is one matrix with one reason rather than N behaviors with N justifications - what
nineteen common keystrokes encode to, what a recorded frame stream paints - and a rendered file is the honest shape
for that. Both implementations read the same bytes, which is what makes "the port did not have to re-record them"
worth anything: a snapshot that gets regenerated to make a rewrite pass was never an oracle.

**What a snapshot renders is data too**, in a `survey` section beside the cases. The rendering belongs to the driver,
but the inputs to it are the corpus's to state: a keystroke list left in one language's tests is nineteen entries the
next language re-types, and then a snapshot both languages agree on says nothing about a list only one of them has. A
survey argues its reason once - that is the whole argument for it not being cases - so the per-case `why` rule does not
apply to it, and the gate holds it to a `why` of its own instead.

**What stays native.** Not everything should be data, and forcing it produces an unreadable pseudo-language. The
line falls where behavior stops being expressible in Muster's vocabulary: driving an `NSView` with a synthesized
`NSEvent`, or proving that two processes appending to one log file never tear a line. Translation *into* the
vocabulary is the hybrid case - a macOS driver maps `NSEvent` to a `KeyEvent` and asserts against a portable
expectation, and a GTK shell would write its own table producing the same `KeyEvent`.
