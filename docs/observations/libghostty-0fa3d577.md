# libghostty at 0fa3d577 (vt-1.0.0)

What moved when the pin went from `9f9b8d1d` to upstream's `vt-1.0.0` tag, re-measured
rather than copied. Built and read 2026-11-10 against ghostty `0fa3d577a`, macOS 26.4 /
arm64, Zig 0.17.1. `docs/observations/libghostty-9f9b8d1d.md` stays the record of the old
pin; a section of it that is not repeated here was not re-measured, and its citations name
the old tree's lines.

Section numbers match the old file where a section carries over.

## 1. Building it

Zig 0.17.1. `build.zig.zon` asks for at least 0.17.0, and `src/build/zig.zig` accepts only
that major and minor, so 0.16 cannot build the tag and 0.17 cannot build the old pin.
Homebrew has no versioned 0.17 formula, so `deps/zig.pin` names the build and `tools/zig`
fetches it into `~/.cache/muster-zig/<version>/` (AGENTS.md, "Building").

Release builds stopped stripping libghostty-vt by default: only ReleaseSmall strips now,
where the old pin stripped ReleaseFast. `./dev` passes `-Dstrip=true` on both lib-vt
builds to keep the old result. `ghostty-vt.xcframework` is no longer built by default, and
Muster never read it.

| Output | At 9f9b8d1d | At 0fa3d577 |
|---|---|---|
| `macos/GhosttyKit.xcframework` | 129 MB | 144 MB |
| `zig-out/lib/libghostty-vt.a` (macOS) | 8.7 MB | 3.5 MB, stripped |
| `zig-out/lib/libghostty-vt.<version>.dylib` | 1.6 MB | 1.9 MB, stripped |
| `zig-out/x86_64-linux-musl/lib/libghostty-vt.a` | not recorded | 5.4 MB, stripped |

A full `./dev -d` with all four patches - GhosttyKit, the macOS lib-vt and both Linux
targets - took 306 s on a machine whose fast cores were already committed (load 16), with
`~/.cache/zig` partly warm from a lib-vt test build. The checkout with its caches is 3.3 GB;
`~/.cache/zig` is 722 MB.

## 3. The embedding runtime is still six callbacks

`ghostty_runtime_config_s` holds the same six: wakeup, action, read clipboard, confirm
read clipboard, write clipboard, close surface. `ghostty_action_tag_e` has 71 tags, up
from 68; `SET_WINDOW_TITLE` is inserted mid-enum and renumbers the tags after it, which
does not matter because GhosttyKit links statically and Swift compiles against the same
header. Muster declines all three new tags.

What changed is the signatures. `include/ghostty.h` now wraps itself in
`#pragma clang assume_nonnull`, so pointer parameters and fields not marked
`GHOSTTY_NULLABLE` arrive in Swift as non-optional: the action callback's app, an
OPEN_URL's and MOUSE_OVER_LINK's `url`, a diagnostic's `message`. The read clipboard
callback returns `ghostty_clipboard_read_result_e`, whose zero is
`GHOSTTY_CLIPBOARD_READ_STARTED`: a callback that still returned `false` would tell
libghostty a read had started, and the request would stay open. Muster returns
`GHOSTTY_CLIPBOARD_READ_UNSUPPORTED` by name.

## 8. Muster links libghostty-vt statically, and the two archives still collide

The dylib's install name is now `@rpath/libghostty-vt.1.dylib` (`otool -D`), where it was
`@rpath/libghostty-vt.dylib`. That stopped mattering once Muster linked
`zig-out/lib/libghostty-vt.a` into everything that uses it, on macOS as on Linux. Nothing
Muster ships loads the dylib, so the bundle carries none and a remote Mac's install is the
daemon, the CLI and the data directory, the same files as a Linux machine's.

The two archives cannot share an image. `nm -gUm`, with weak definitions dropped, finds 815
strong external symbols defined both in GhosttyKit's `libghostty-internal.a` and in
`libghostty-vt.a`:
- 409 are compiler-rt's;
- most of the rest are Wuffs, simdutf and Highway;
- six are libghostty's own C functions: `ghostty_simd_*` and `ghostty_hwy_detect_targets`.

The old pin's 35 came from a link attempt, not from `nm`, so the two numbers are not
comparable. The link itself was not retried here.

Muster never puts both in one image. GhosttyKit links into the app, and libghostty-vt into
`libmuster.dylib`, a cdylib whose only exports are `include/muster.h`'s three functions
(`nm -gU`: `_muster_dispatch`, `_muster_free`, `_muster_set_event_callback`). libmuster and
the daemon each depend on nothing but libSystem and libiconv.

The cost is a copy per binary. In a release build, `muster-daemon` grows from 10.1 MB to
11.6 MB and libmuster by 26 KB, since the linker keeps only what the core calls. The
1.9 MB dylib leaves the bundle.

## 11. What the archives bring in

Measured by `tools/ghostty-embeds.py` against the linked app binary: the same ten
libraries link into `Contents/MacOS/muster` as at the old pin, with symbol counts within a
few percent. The package manifest moved from `build.zig.zon.json` to `deps/packages.json`.

libghostty-vt is not free of third-party code, as the old file and NOTICE said. Upstream's
`deps/libghostty-vt-third-party.json` lists Highway 1.2.0, simdutf 9.0.0, uucode 0.2.0,
Wuffs 0.4.0-alpha.10 and translate-c 2.0.0 as linked into it. Highway and simdutf were
already in the old pin's build. `licenses/GHOSTTY-EMBEDDED.md` now carries all five.

## 14. The formatter, and what Muster still carries for it

Upstream absorbed most of what patch 0001 added to the formatter: hyperlinks in VT output
(#14711), blanks written as cursor moves rather than spaces and rows painted only with a
background kept (#14725), and the blank rows below the last row with text kept for VT
output with trimming off (#14766). Patch 0001 no longer touches `formatter.zig`.

Three needs remain, and `src/terminal/c/muster.zig` meets them from the formatter's public
Zig types:

- formatting a screen that is not the active one, since the C formatter formats only the
  active screen;
- formatting a screen's history alone, which is how the catch-up test's oracle reads it;
- formatting terminal state with no content and the cursor placed for origin mode and the
  right margin. `TerminalFormatter` sets `cursor_origin` and `wrap_column` on the screen
  formatter it builds only when it also writes the modes, and Muster writes modes itself.

A new formatter defaults its extras to palette, style and hyperlink, so every Muster
formatter sets its extras explicitly; a replay that restated the palette would pin the
surface to the daemon's colors. Upstream's cursor extra now keeps a pending wrap over a
blank cell (#14670, #14918), which Muster's own cursor restore lost.

## 15. Freeing a surface can still wait on the main thread forever

Upstream did not fix the deadlock. The app mailbox is still `BlockingQueue(Message, 64)`
(`App.zig:599`), drained only by a tick; the reader still falls back to a `.forever` push
(`termio/stream_handler.zig:147`), the IO thread pushes `child_exited` `.forever`
(`termio/Exec.zig:293`), and `Surface.deinit` still joins the IO thread on the caller.

What changed is a rule about which thread may free a surface. #14691 routes every
CVDisplayLink start and stop through the main thread, and `renderer.deinit` stops the link
(`renderer/DisplayLink.zig`: `close` "Must be called on the main thread"). Stopping it
elsewhere can collide with CoreVideo's own stop during display sleep, wake or
reconfiguration and freeze the app, and can free the link while its main-queue sync
callback runs. The old patch 0002 ran all of `Surface.deinit` on its spawned thread.

So 0002 now splits `Surface.deinit` into `stopThreads` and `deinitStopped`. The spawned
thread runs only `stopThreads`, the joins that can wait on the mailbox, and then
dispatches to the main queue, where `deinitStopped`, the surface's memory and the
completion run. `deinit` calls both halves in order, so a plain free behaves as before.
Because completions are now main-queue jobs, the renderer's own teardown cannot wait for
them; it hands the app to the object counting frees, which ticks it and frees it after the
last one (`Renderer.swift`, `SurfaceFrees`).

The two other waits the old section listed are still there: `queueIo`'s `.forever`
fallback (`termio/mailbox.zig:95`) and the renderer's `.forever` push to the app mailbox
(`renderer/generic.zig:2059`).

## 17. Defaults and behavior that moved without a compile error

- Key encoding under modifyOtherKeys 2: F13-F25, Help and alt+Escape now send xterm's
  encodings where they sent nothing or `ESC ESC` (#14145, #14146, #14508). Ctrl keys and the
  keypad encode as before (`corpus/conformance/key-encoder.json`).
- Mode 5522, kitty's paste events, is a new mode, so the replay states it.
- A program's RIS no longer clears the kitty image limits and allowed mediums (#13951);
  `tests/vt/effects.rs` checks that a file-path image stays refused after `ESC c`.
- `copy-on-select` became a compatibility key in `Config.zig`. Muster's clipboard write
  callback does nothing, so this has no effect.

Not measured here: throughput (`perf/baseline.json` was not re-recorded), and anything
that needs a display - parked display links (#14035), GPU memory released by hidden
surfaces (#14017), sleep and wake.
