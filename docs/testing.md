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
- **Fake only the seams, and fake them honestly.** The outside world enters at the backend connection and the
  renderer seam, plus the clock. Tests run the real core against fakes only there, and the fakes keep the contract's
  awkward parts - frame batching, disconnects mid-stream, out-of-order events, resize races. A polite fake is false
  confidence.
- **Audit the fakes against reality.** We do not own the backend, and its API is explicitly unstable; a drifted fake
  daemon is Muster's top false-green risk. One contract corpus replays against both the fake and a real herdr -
  local and the devenv container in CI. When herdr changes behavior the corpus fails loudly; that is the upgrade
  gate, not a production surprise.
- **Record reality, replay it as data.** Oracles come from capture, not belief: ANSI streams from real agent
  sessions, key encodings from real terminals, daemon event logs. Cases are text files a reviewer can read, in the
  style of [go-snap](https://github.com/amterp/go-snap); adding coverage means adding data, not test code.
- **Assert what the user sees and what the daemon receives.** The user-facing oracle is the terminal grid, computed
  in the harness by libghostty-vt - the production engine. The daemon-facing oracle is the exact intent messages on
  the wire. Never pixels (GPU-flaky), never internal structures (false confidence in both directions).
- **Deterministic or it does not merge.** Injected clock, event-driven waits (`events.subscribe`,
  `pane.output_matched`), no sleeps, suite passes offline. Async byte streams are replayed, never raced.
- **Tiered for speed.** Slow tests get run less, so the default suite runs against the fake: milliseconds, offline,
  authoritative for iteration. The contract tier runs against a real, version-pinned herdr - spawned headless with
  an isolated config dir, one daemon shared per run, a throwaway session per test, panes running scripted fake
  agents, synchronized by event waits - and stays small enough to finish in seconds. It runs on herdr upgrades, on
  adapter or fake changes, and in CI, not on every save. The SSH tier (devenv container) is smaller still. Trust in
  the fast tier is earned by the slow tiers; that is what keeps iteration fast without going blind.
- **The suite proves itself.** A bug fix lands as a failing test first, then the fix - two commits, so CI shows red
  then green (cmux's discipline). Guard against silently skipped tests. Performance is measured against cardinality
  budgets separately; a functional green is never a performance claim.

The seams these tests inject at, and the oracles they read, are defined in `architecture.md` (seams and test hooks).
