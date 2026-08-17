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
  wrong: a herdr daemon costs 25 ms to spawn and answer, a session snapshot 0.7 ms, and a full mirror bootstrap -
  snapshot, subscribe, first event - 0.7 ms. Thirty repeated runs across serial and parallel execution produced no
  failures. The seconds-per-test cost worth fearing belongs to agent *detection*, which screen-scrapes on a
  hardcoded two-second timer; tests that report agent state through the API never touch it.

  So the backend seam is not faked at all. Tests that need a daemon spawn a real one under a scratch config
  directory, through `crates/herdr-harness` - one daemon per test, killed on drop including on a panic, isolated by
  pointing XDG at a scratch root because that is where herdr resolves its config and keeps its socket. What this
  buys is the removal of a whole category: there is no hand-written herdr in this repo, so there is nothing to
  drift, and "a drifted fake daemon is Muster's top false-green risk" stops being a risk we manage and becomes one
  we do not have.

  It also catches what a stand-in cannot. Building the subscription against a real daemon turned up two facts no
  invented one would have contradicted: a subscription is requested by a dotted name and answered with a snake one,
  and half-closing the write side - which is how every other herdr call signals it is finished - ends a subscription
  on the spot. Both fail as silence rather than as an error, which is the shape of bug a fake is worst at.

  The daemon is pinned rather than found. `deps/herdr.pin` carries a version and a checksum per platform, `./dev`
  fetches that binary into `deps/herdr/` once and verifies it, and the path is handed down to the tests through
  the environment. Nothing consults PATH, for two reasons: a contributor's own herdr should be free to be any
  version, and a test that resolved its own daemon could quietly run against one nobody verified. The probe that
  records the corpus uses the same pinned binary, so the oracle and the code being judged always come from one
  daemon.
- **Detect wire drift mechanically, not by waiting for a test to fail.** herdr generates a canonical JSON Schema of
  its whole API from its own request types, fails its own build when the two disagree, and embeds it in the binary
  (`herdr api schema --json`). A copy sits in `corpus/herdr-<version>/api-schema.json`, and `./dev` diffs the two
  before running anything. A daemon that changed its wire is named as such, with the diff, instead of surfacing as
  a puzzling failure three layers up.
- **Inject at the seams the code already has, not by impersonating a daemon.** Three different things get called
  fault injection, and only one needs machinery. *Daemon state* - a blocked agent, fifteen panes, a pane whose
  program died - is driven through herdr's own API, which can produce all of it on request. *Daemon-internal
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
  time by construction: `split_sides.rs` waits past herdr's own second publish, measured at 100.4 ms, so that a
  mirror which merely got there first and then walked backwards fails rather than passing on timing. There is no
  event for "nothing further arrives". Both are legitimate; both need a measured number and a comment saying which
  measurement, because a wait sized by guesswork is the flake this rule exists to prevent.

  **There is one `until`, in `herdr-harness`, and it has one deadline.** There were twenty-four, one per test file,
  because the way a test gets written is by copying the nearest one - and they had drifted to deadlines of two, ten,
  fifteen, twenty and thirty seconds, with not one of the outliers saying why. A single number is the honest answer
  because a deadline here bounds a failure rather than tuning anything: a genuine wedge shows up as runs that either
  finish well inside a second or sit at exactly the deadline, so no value would have made the difference and only
  the shape of that distribution says what is wrong. A wait that truly needs longer takes `until_within` and states
  its reason at the call site, which is the one place a reader can check it. Every wait also carries a slot for what
  was true instead, because a timeout saying only that a condition never came true sends whoever hit it back to add
  exactly that and run again.
- **Tiered by what a tier can reach, not by what it fakes.** Most of the core is pure - a keymap, a fold over
  events, a byte-stream parser - and needs no daemon in any tier, so those stay microseconds. Tests that need a
  daemon spawn one and stay in the default gate, because 25 ms is not a tier boundary. What remains genuinely out
  of the gate is what needs something a developer's machine cannot be assumed to have: `--contract` needs a
  logged-in GUI session to launch the app, `--latency` and `--perf` measure timing and would be flaky as
  assertions, and the SSH tier needs the devenv container. That is the real line, and it is narrower than the one
  drawn when the backend was going to be faked.
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

**What stays native.** Not everything should be data, and forcing it produces an unreadable pseudo-language. The
line falls where behavior stops being expressible in Muster's vocabulary: driving an `NSView` with a synthesized
`NSEvent`, or proving that two processes appending to one log file never tear a line. Translation *into* the
vocabulary is the hybrid case - a macOS driver maps `NSEvent` to a `KeyEvent` and asserts against a portable
expectation, and a GTK shell would write its own table producing the same `KeyEvent`.
