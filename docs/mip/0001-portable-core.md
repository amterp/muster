---
mip: 1
title: A portable core, and the seam that reaches it
status: Accepted
kind: Architecture
created: 2026-08-13
decided: 2026-08-13
supersedes:
superseded-by:
related:
---

# MIP-1: A portable core, and the seam that reaches it

## Summary

Muster's headless core moves from Swift to Rust. The shell stays native and per-OS -
Swift and AppKit on macOS, something else on Linux and Windows. They meet at one C ABI
symbol carrying protobuf-encoded messages in Muster's own vocabulary, and that schema is
also the CLI and agent-facing API.

The data plane is unaffected: pane frames go from adapter to surface without passing
through the core, so this seam carries events, never bytes.

## Context / Motivation

`origin.md` leaned Swift-first with portability enforced by contract tests, and left the
real choice to be made "after stage 1 teaches us the real shape" (kan `a_26BJG0H5B`).
Stage 1 is done: a libghostty surface renders a daemon-owned pane, keystrokes reach it
correctly, and the core is 1269 lines in which only four files import anything at all.

Two facts make now the moment rather than later.

**The core is at its smallest and about to triple.** The next planned work is the session
mirror, then layout math, then the dispatcher and attention routing - every one of them
core. A port today is a couple of days. The same port after Stage 2 is a rewrite of the
thing everything else hangs off.

**A Swift core does not avoid the FFI boundary, it relocates it.** Swift on Linux is real
for servers; its GUI bindings are not something to bet a product on, and Windows is
thinner still. So a Linux Muster's shell is Rust or C++ whatever we do here:

| | macOS | Linux / Windows |
|---|---|---|
| Swift core | native | Rust shell calls a Swift library |
| Rust core | Swift shell calls a Rust library | native, no boundary at all |

Swift-called-from-Rust is the worse of the two directions: `@_cdecl` exports written by
hand, ownership managed manually across the boundary, and the Swift runtime shipped as a
dependency of a Rust binary. It also lands on the platform where we would have the least
tooling to debug it. "Cross-platform stays open" is a desideratum, not an aspiration, and
this is the choice that decides whether it stays open cheaply or expensively.

## Decision

**The core is Rust.** The mirror, keymap and input pipeline, layout math, the action
dispatcher, attention routing, configuration, and the herdr adapter. No OS types, no
windowing, no rendering.

**The shell is native per OS.** On macOS: Swift, AppKit, and the libghostty surface
embedding, which is `NSView`-shaped and could not move regardless. It wires OS events in
and renders what the core says.

**They meet at one symbol.**

```
muster_dispatch(request_bytes, len) -> response_bytes
```

Every intent in and every view state out is one protobuf schema, regenerated on both
sides when types change, so the hand-written binding surface stays near zero. The `.proto`
becomes the executable definition of Muster's vocabulary, which `architecture.md` already
wants and currently has only as prose.

**The push direction is a registered callback.** The core must wake the shell without
being asked - an agent changed state, a daemon event arrived, a notification is due. The
shell registers a function pointer at startup; the core calls it with an encoded event.
This is the part protobuf does not solve and the part most likely to be got wrong, so it
is named here rather than discovered.

**The data plane does not cross this seam.** Pane frames run adapter to surface, as they
already do. The boundary carries events at human and daemon rates - keystrokes are around
ten per second - which is why serialization cost is not a consideration and ergonomics
win.

**One schema serves IPC, CLI, and the agent API.** "Every action runs through one shared
path exposed to GUI, CLI, and API alike - parity by construction, not by discipline"
stops being a discipline and becomes codegen.

## Rationale

The FFI boundary has to exist somewhere the moment a second platform appears. Putting it
between a native shell and a portable core puts it where it is cheapest on every platform
and where the tooling is best on the one we develop on.

Rust specifically, over the other portable options: it exports a C ABI without a runtime
to ship, it is what herdr is written in (so its wire types are readable to us and its
`src/ghostty/mod.rs` is a working reference for driving libghostty-vt's key encoder from
Rust, which is the fiddliest thing in the port), and it is the language a Linux or Windows
shell would most plausibly be written in - collapsing the boundary to nothing there.

Protobuf over a hand-rolled struct ABI, because the types crossing this seam are a pane
tree, a set of agent states and a keymap resolution, not scalars. Hand-maintaining those
in two languages is where drift lives, and drift at this seam is invisible until it is a
crash.

## Alternatives Considered

**Swift everywhere, port later if a Linux version happens.** The status quo, and the
cheapest thing today. Rejected because the cost curve only rises: the core is smaller now
than it will ever be again, and the plan on the table triples it. It also fails to avoid
the boundary - see the table above - so it buys a delay rather than a simplification. If
cross-platform were dropped as a desideratum this would win outright, and that is the one
change that would reopen this decision.

**Go core.** Easier to write than Rust and a fine language for this shape of logic.
Rejected on embedding: a GC and a scheduler inside a GUI process is awkward, cgo's
boundary is slow enough to matter at event rates, and Go's C-export ergonomics are worse
than Rust's in exactly the direction we need.

**Zig core.** Matches ghostty, which is the dependency we embed. Rejected because the
language is pre-1.0 and the project already spends real effort pinning a Zig version for
libghostty alone; making the core hostage to that same churn buys alignment with one
dependency at the cost of stability everywhere else.

**Hand-rolled C ABI with plain structs, no protobuf.** Fastest, and no codegen step.
Rejected because the win is on an axis that does not bind - the seam carries events, not
bytes - and the cost lands on the axis that does: every vocabulary change becomes a
two-language edit done by hand.

**Core as a separate process, IPC over a local socket.** Cleaner isolation, and a crash in
one does not take the other. Rejected for now as a bigger change than the problem needs:
it adds process lifetime, startup ordering and a second failure mode, and Muster already
has enough processes. The protobuf schema means this stays available later - a socket
transport is a different carrier for the same messages.

**FlatBuffers or Cap'n Proto.** Zero-copy reads, which would matter if the render path
crossed this seam. It does not.

## Consequences & Trade-offs

**Two toolchains.** `./dev` grows a Rust build, and a contributor needs both. The one door
stays one door.

**Everything that exists gets ported**, including 81 passing tests. This is the real cost,
and it is mitigated by extracting a language-neutral conformance corpus from the Swift
suite *while it is green*, before any porting starts - so the corpus is proven by a working
implementation and becomes the port's acceptance criterion rather than something written
afterward to match. See `testing.md`.

**A stretch where nothing visibly improves.** The app will do exactly what it does today
with more machinery behind it. That is the price of doing this at the cheapest moment
rather than the convenient one.

**libghostty is reached from two languages.** The surface API from Swift, the VT and key
encoder from Rust. Both are C; herdr already does the second.

**The renderer seam narrows usefully.** Anything the shell can do that the core cannot
describe in the schema becomes visible as a missing message, rather than hiding as a Swift
call.

## Open Questions

- ~~Whether `muster-bridge` stays a Swift executable or moves to Rust.~~ **Answered: Rust,
  and first.** Its only decidable part - frame decoding - belongs to the core, and being a
  separate process it needs no seam at all. That made it the one place Rust could run in
  production at zero FFI risk, so it crossed before the boundary existed rather than after.
- How the callback direction handles a shell that is slow to drain. Backpressure at this
  seam has no design yet. The starting answer worth writing down: because view = f(daemon
  state), a queued state update can be *coalesced* rather than dropped or blocked on, which
  is what lets this seam afford a bounded queue. Nothing is queued today - the callback
  runs on the thread that noticed and the shell copies and hops - so this stays open until
  there is state worth coalescing.
- Whether the herdr adapter can share types with herdr itself rather than restating them.
  Tempting, and in tension with "nothing herdr-shaped escapes the adapter".

## References

- `docs/architecture.md` - the three layers and two seams this refines.
- `docs/testing.md` - the conformance corpus that makes the port verifiable.
- `docs/origin.md`, "The decision" - the Swift-first lean this supersedes.
- kan `a_26BJG0H5B` - the card that deferred this decision to stage 1's end.

---

## History
- 2026-08-13 Draft
- 2026-08-13 Accepted
- 2026-08-13 Core, herdr adapter, VT and bridge ported; the bridge question answered in
  Rust's favor. The seam itself is still ahead.
- 2026-08-13 The seam exists and carries the shell's log records. The libghostty
  co-linking worry did not materialize: the core is a dylib exporting three symbols, so
  its libghostty-vt dependency is its own and never meets the surface xcframework's.
