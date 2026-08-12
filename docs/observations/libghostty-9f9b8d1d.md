# libghostty at 9f9b8d1d

What the renderer dependency actually offers, as opposed to what `architecture.md` assumed
while never having built it.

Read and built 2026-08-12 against ghostty `9f9b8d1d` (version string `1.3.2-dev`, `main`
at the time of pinning), macOS 26.4 / arm64, zig 0.16.0. The pin lives in
`deps/ghostty.pin` and `./dev -d` reproduces everything below. Header citations are
against that tree; herdr citations are against the `v0.8.0` tag at `~/src/herdr`.

`main` rather than the v1.3.1 release, because the release's libghostty-vt exposes only
key, OSC, SGR and paste. Everything in section 6 - the oracle the whole testing strategy
rests on - arrived after it.

## 1. Building it costs a toolchain pin and 5 minutes

zig 0.16.0 exactly. v1.3.1 wanted 0.15.2, so the requirement moves with the pin, and a
mismatch surfaces as a wall of compiler errors inside ghostty rather than as a version
complaint. `./dev -d` checks it first and says so.

Two builds, because `-Demit-lib-vt` turns the xcframework off:

| Build | Command | Wall | Output |
|---|---|---|---|
| surface API | `zig build -Doptimize=ReleaseFast -Demit-xcframework=true -Dxcframework-target=native` | ~5 min cold | `macos/GhosttyKit.xcframework`, 129 MB |
| VT | `zig build -Doptimize=ReleaseFast -Demit-lib-vt=true` | 32 s on that cache | `zig-out/lib/libghostty-vt.a`, 8.7 MB, plus headers and its own xcframework |

The checkout is 135 MB and the zig caches come to ~1.6 GB (1.2 GB in-tree, 407 MB in
`~/.cache/zig`). Re-running `./dev -d` against a warm cache is 12 s; against a matching
stamp, instant.

## 2. A surface cannot be fed bytes - confirmed

`ghostty_surface_config_s` (`include/ghostty.h:471`) carries `command`,
`working_directory`, `env_vars`, `initial_input`, `wait_after_command`, and nothing that
takes a stream. The only ways into a surface are `ghostty_surface_key` and
`ghostty_surface_text` (`:1149`, `:1153`) - key events and committed text, not bytes.

So the pane channel reaches a surface through the command it spawns, and the bridge
subprocess `architecture.md` describes is forced rather than chosen. `initial_input` is
a one-shot at creation and does not change this.

Re-check on every pin bump: this is the single constraint the data plane's shape rests
on, and one function would dissolve it.

## 3. The embedding runtime is six callbacks

`ghostty_runtime_config_s` (`:1042`) wants `userdata`, a `supports_selection_clipboard`
flag, and six function pointers: wakeup, action, read clipboard, confirm read clipboard,
write clipboard, close surface.

The breadth hides behind `action_cb`, one dispatcher over the 68 tags of
`ghostty_action_tag_e` (`:905-973`) - open a tab, set the title, ring the bell. A host
that answers `false` to a tag declines it, so a spike can implement almost none.

This was carried on the board as the spike's unknown cost, on the assumption that
ghostty's own macOS app was the measure of what embedding demands. It is not: that app is
large because it is a terminal, not because the API is.

## 4. Two things about embedding that the header does not say

Both cost an afternoon, and neither is discoverable by reading `ghostty.h`. Recorded
because the next person to touch `MusterRenderer` will hit them again.

**`ghostty_init` wants the process's real argv.** It parses `+action` CLI verbs there.
Handed `argc: 0` and an empty array, the process exits 6 before any error can be
returned or logged - the failure looks like the embedding API rejecting the host, and is
just a bad argv. Pass `CommandLine.argc` and `CommandLine.unsafeArgv`.

**Runtime callbacks must be genuinely nonisolated.** libghostty invokes them from its own
threads - renderer, IO. Written as closures inside a `@MainActor` type, Swift 6 infers
main-actor isolation for them, compiles in an executor check, and aborts the process the
first time the renderer thread calls `wakeup_cb`:

```
thread #8, name = 'renderer', stop reason = EXC_BREAKPOINT
  libdispatch`_dispatch_assert_queue_fail
  libswift_Concurrency`_swift_task_checkIsolatedSwift
  muster`closure #1 in Renderer.init
```

That backtrace reads like a libghostty threading bug and is a host bug. Declare the six
callbacks as file-scope functions, which `@convention(c)` wants anyway.

Worth knowing alongside them: libghostty runs its own display link and paints the layer
it attaches to the host's `NSView`. A host that also calls `ghostty_surface_draw` from
`draw(_:)` is a second painter racing the first - ghostty's own macOS app never calls it.

## 5. The input path is the same encoder on both ends

libghostty-vt encodes key events to escape sequences with no surface and no terminal
involved: `ghostty_key_encoder_new`, `_setopt`, `_encode`
(`include/ghostty/vt/key/encoder.h`). A key event carries action, key, mods,
consumed mods, `composing`, the utf8 text, and the unshifted codepoint
(`vt/key/event.h`) - which is the full-fidelity report `architecture.md` asks the shell
to produce, `composing` included for IME.

`GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS` selects the kitty keyboard protocol, and
`GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT` settles the macOS option key.

The part that matters more: **herdr encodes with this same library.** It vendors
libghostty-vt under `vendor/libghostty-vt`, and `src/ghostty/mod.rs:2547` and `:2552`
call `ghostty_key_encoder_setopt_from_terminal` and `ghostty_key_encoder_encode` to
re-encode input for a pane's real modes. So Muster's input path is ghostty-encode,
herdr's crossterm parse (`src/raw_input.rs`), ghostty-encode again - one foreign step
between two runs of the same engine, rather than two independent implementations that
have to agree.

The two copies are near but not identical, and the difference is worth knowing before
trusting the symmetry. herdr v0.8.0 vendors `c5a21edf` (`libghostty-vt-1.3.2-HEAD`)
against our `9f9b8d1d`, and carries one local patch: `0001-default-grapheme-cluster-mode`
makes DEC mode 2027 the default so multi-codepoint clusters land in one cell.
That patch touches `src/terminal/c/terminal.zig` and nothing in key encoding, so it does
not weaken the symmetry above - but `vendor/libghostty-vt.patches.md` is the file to
re-read when herdr upgrades, because a patch that did touch encoding would.

**Do not encode at maximal fidelity.** herdr's TUI enables exactly three kitty flags -
disambiguate, report event types, report alternate keys
(`src/input/model.rs:219`) - and the function is named
`ime_compatible_keyboard_enhancement_flags` because reporting *all* keys as escape codes
breaks IME. `src/input/model.rs:427` asserts `REPORT_ALL_KEYS_AS_ESCAPE_CODES` stays out.
Muster should emit the same three: it is what herdr's parser is tested against, and the
two flags left off are left off on purpose.

## 6. The grid oracle exists here

`ghostty_terminal_vt_write` (`vt/terminal.h:1721`) feeds bytes to a headless terminal;
`ghostty_terminal_grid_ref` (`:1968`), `ghostty_row_get` and `ghostty_cell_get`
(`vt/screen.h:363`, `:316`) read the result back. `snapshot.h`, `render.h`,
`selection.h` and `modes.h` are all present.

That is `testing.md`'s user-facing oracle, in C, at this pin: feed a recorded pane stream
through the production engine and snapshot the grid. The card carried it as blocked on
upstream. It is not blocked.

`mouse.h` and `paste.h` are here too, so SGR mouse encoding and bracketed paste come from
the same place as key encoding rather than being hand-rolled.

## 7. What this changes

Two things `architecture.md` says are now underspecified rather than wrong.

The renderer seam says the bridge exists because "the embedding header has no byte-feed
API". True, and section 2 is the evidence - but it reads as a temporary embarrassment,
and it is a stable constraint worth stating as one.

The control plane says the shell reports keys with full fidelity and "the daemon
encodes". Also true, and incomplete: the report is itself a kitty-protocol encoding, the
core produces it with libghostty-vt, and the flag set is chosen to match herdr's TUI
rather than to be maximal. Both encoding steps deserve naming, because "report" sounds
like a structure on the wire and it is bytes.
