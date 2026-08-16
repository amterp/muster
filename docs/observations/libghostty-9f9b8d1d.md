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
encodes". That half is now overturned outright - the daemon does not encode on the
channel Muster uses (`herdr-0.8.0.md` section 5) - but the correction this section asked
for still holds: the report is itself a kitty-protocol encoding, the core produces it
with libghostty-vt, and the flag set matches herdr's TUI rather than being maximal.

## 8. Using both libraries at once costs two things

Recorded 2026-08-13, wiring libghostty-vt into the package. Neither is visible in a
header, and both were found by building.

**The two static archives cannot go in one binary.** `GhosttyKit.xcframework` and
`ghostty-vt.xcframework` are separate builds of the same commit, and each embeds its own
copy of Zig's runtime. Linking both fails on 35 duplicate symbols - `___ubsan_handle_*`,
defined by each Zig compilation unit:

```
duplicate symbol '___ubsan_handle_sub_overflow_abort' in:
    .build/.../libghostty-vt.a[arm64][6](libghostty-vt-static_zcu.o)
    .build/.../libghostty-internal.a[276](libghostty_zcu.o)
ld: 35 duplicate symbols
```

The shared C++ dependencies (simdutf, highway) sit in separate archive members and would
have resolved against GhosttyKit's copies; it is the Zig units that collide, and both are
pulled the moment anything calls into either API.

`libghostty-vt.dylib` composes where the archive cannot: it exports only the 192 public
`ghostty_*` functions and keeps its runtime private, so `Package.swift` links the surface
API statically and the VT dynamically. Its install name is `@rpath/libghostty-vt.dylib`,
so two `@loader_path` rpaths - one for executables, one for a test bundle's deeper
`Contents/MacOS` - make a plain checkout run without installing anything. Revisit if
upstream ever emits one library carrying both APIs; until then this is a fact about the
build, not a preference.

**A stock headless terminal does not render what a herdr pane renders.** herdr's
vendored copy patches DEC mode 2027 on by default (section 5); ours has it off, and the
difference is visible on the first emoji:

```
herdr:      emoji: 👍 and 👨‍👩‍👧
libghostty: emoji: 👍 and 👨‍" 👧
```

A ZWJ cluster lands in one cell there and several here, so a grid read from an unmodified
terminal describes a screen the user never saw. `MusterVT.Terminal` writes `CSI ? 2027 h`
at creation to match; there is no option on `ghostty_terminal_set` that reaches DEC modes.

Worth noting how this surfaced, because it is the argument for the check that found it:
the patch was already written down in section 5 and its consequence was still missed. It
came back as a failing cross-oracle test - herdr's own `pane.read` of a screen against
libghostty-vt's replay of the frames describing that same screen
(`corpus/herdr-0.8.0/frame-fidelity/`). A snapshot alone would have recorded the wrong
screen as the expectation and passed forever.

## 9. Configuring it without a config file

Measured 2026-08-16, cutting the loan that let a Ghostty config decide what a Muster pane
looks like. The question was whether an embedder can hand libghostty its own appearance
values at all, since everything else about the seam depends on the answer.

**There is no setter.** No `ghostty_config_set`, no string or memory loader. The only ways
into a `ghostty_config_t` are the four `load_*` functions (`include/ghostty.h:1097-1100`).
`Config.loadIter` in Zig takes any iterator of `--key=value` strings and is the natural
hook for one, but it is not exported to C in this revision.

**Two of the four work for an embedder, and only one of them twice.**

`ghostty_config_load_cli_args` takes no arguments - it reads `global.args()`, which is
whatever was handed to `ghostty_init` (`src/global.zig:74`). Nothing requires that be the
process's real `argv`, so synthesizing `--key=value` strings works and reaches the parser.
It is one-shot: `ghostty_init` assigns `var state: ?GlobalState` unconditionally, so
changing the values later means re-initializing libghostty's global runtime underneath a
live app.

`ghostty_config_load_file` takes an absolute path and reads the same syntax without the
dashes - `key = value`, one per line, `#` comments. It composes: a second file read into a
second handle gives a second set of values, which is what `ghostty_app_update_config`
wants. **This is the one Muster uses**, for both the first launch and every reload after
it, because two mechanisms for one job is a way for launch and reload to disagree. The path
being absolute is asserted rather than refused, so in a ReleaseFast build a relative one is
undefined rather than an error.

Worth being explicit that today's call is a *latent* bug as well as a design one: Muster
passes its own `CommandLine.unsafeArgv` to `ghostty_init`, so `muster --pane w1:p1` is
already being offered to libghostty as configuration. Nothing has come of it because
nothing calls `load_cli_args`, but the fix is the same either way - hand it a program name
and nothing else.

**Nothing about a bad config file is fatal, and both kinds are reported.** A key
libghostty does not know and a value it cannot parse each append a diagnostic and leave the
rest of the file applied:

```
config:2:not-a-ghostty-key: unknown field
config:1:cursor-style: invalid value "wobble", valid values are: bar, block, underline, block_hollow
```

Drain them with `ghostty_config_diagnostics_count` and `ghostty_config_get_diagnostic`. For
Muster this is not a user-facing error path: the person's own file was already parsed and
refused by the core, so a diagnostic here means Muster's own translation emitted something
libghostty does not accept, and the log says so.

**`ghostty_config_get` reads back less than you can write.** It answers for
`?[:0]const u8`, `bool`, `u8`/`u32`, `i16`, `f32`/`f64`, any enum (as its tag name), and
structs carrying a `cval()` - Color and Palette among them (`src/config/c_get.zig`). It
returns false for everything else, which turns out to be most of what Muster sets. Of the
twelve keys the appearance vocabulary emits, six read back - `font-size`, `background`,
`foreground`, `cursor-style`, `cursor-style-blink`, `palette` - and six are write-only:
`font-family` is a `RepeatableString`; `cursor-color`, `cursor-text`,
`selection-background` and `selection-foreground` are `?TerminalColor`, a union with no
`cval`; `window-padding-x`/`-y` are a plain struct with none either. All of them apply.

So the oracle for a translation test is two-sided, and both sides are needed: every
C-readable key reads back the value Muster meant, and the diagnostics count is zero -
which is what covers the keys that cannot be read, since a wrong key name or an
unparseable value would show up there.

One trap in the readback worth naming, because it fails quietly. The out-pointer's type is
decided by the key and nothing checks it: reading `font-size`, an `f32`, into a `Double`
returns true and yields `-1.0000002441229299` rather than the 17 that was set.

## 10. Search is here, and it searches a buffer Muster's panes do not have

Read 2026-08-16 while building find-in-pane. libghostty already implements terminal
search, and the useful finding is what it searches rather than that it exists.

**All five binding actions reach an embedder by name.** `ghostty_surface_binding_action`
parses whatever string a config file could hold and performs it
(`src/apprt/embedded.zig:1981-1996`), so `search:<text>`, `search_selection`,
`navigate_search:next|previous`, `start_search` and `end_search`
(`src/input/Binding.zig:409-433`) need no new C entry point. Muster already drives font
sizing this way. Empty text on `search:` cancels the search and leaves any UI up;
`end_search` is what tears it down.

**Four actions come back through the app-wide callback**, not a search-specific one:
`GHOSTTY_ACTION_START_SEARCH`, `_END_SEARCH`, `_SEARCH_TOTAL`, `_SEARCH_SELECTED`
(`include/ghostty.h:966-969`), delivered to the `action_cb` in
`ghostty_runtime_config_s`. Both counters are `ssize_t` with **-1 standing for null**
(`src/apprt/action.zig:1006-1049`), and `selected` is 0-based despite a doc comment
saying otherwise. Index 0 is the *bottom-most* match and the index counts upward, which
is why Ghostty's own bar draws "next" as a chevron pointing up.

**The total is a running count, and nothing says when it is final.** Search runs on its
own OS thread with a 24 ms refresh timer (`src/terminal/search/Thread.zig:41-45`),
emitting `total_matches` each time the count changes. The thread does raise a `complete`
event, and `Surface.searchCallback_` drops it - `// Unhandled, so far.`
(`src/Surface.zig:1544-1545`) - so an embedder watches the number tick upward with no
way to know it has stopped.

**It covers the whole scrollback, and that is exactly why it is the wrong engine here.**
`ScreenSearch` pairs an `ActiveSearch` over the mutable active area with a
`PageListSearch` walking the entire history in reverse (`src/terminal/search/screen.zig`),
and keeps a separate search per screen so switching to the alt screen and back does not
restart it. That is a better find than the one Muster is building - over a buffer Muster's
panes do not have. A pane's surface is repainted from herdr's frame diffs, so its own
scrollback is empty and libghostty would be searching one screen. The scrollback is the
daemon's, and the daemon cannot search it (`herdr-0.8.0.md` section 17).

**The matcher is `std.ascii.indexOfIgnoreCase`** (`src/terminal/search/sliding_window.zig`)
- plain substring, ASCII case folding, no regex, no word boundaries. Anything Muster
counts itself has to match that exactly, or the number in the find bar disagrees with what
is highlighted underneath it.

**Highlighting is libghostty's and costs the embedder nothing.** A third searcher over just
the viewport produces the rectangles, which go to the renderer thread; the app draws
nothing. Colors are config keys - `search-background` `#FFE082`, `search-selected-background`
`#F2A57E` and their foregrounds (`src/config/Config.zig:1097-1124`) - so they arrive through
the same derived config file as the rest of the appearance vocabulary.

**One build fact that decides which library this works in.** The search thread is compiled
out of the `.lib` artifact and present in the `.ghostty` one:

```zig
pub const Thread = switch (options.artifact) {
    .ghostty => @import("search/Thread.zig"),
    .lib => void,
};                                      // src/terminal/search.zig:11-15
```

`.lib` is libghostty-vt, which Muster links for key encoding and the grid oracle; `.ghostty`
is `GhosttyKit.xcframework`, which is where panes come from. So search is available exactly
where Muster needs it, and would not have been had the surfaces come from the other library.
The two headers are byte-identical either way, so this is not something a build failure
would report.
