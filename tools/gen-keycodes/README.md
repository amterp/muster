# gen-keycodes

Generates the three tables the input path needs to turn a macOS key press into a
libghostty-vt key event, from the pinned ghostty checkout in `deps/ghostty`.

```
./gen-keycodes
```

Reads:

| Source | What |
|---|---|
| `deps/ghostty/src/input/keycodes.zig` | macOS virtual keycode <-> W3C DOM code, per key |
| `deps/ghostty/zig-out/include/ghostty/vt/key/event.h` | the `GhosttyKey` C enum |

Writes:

| Output | What |
|---|---|
| `crates/muster-core/src/input/key.rs` | `Key`, Muster's own physical-key enum, and the macOS keycode it comes from |
| `crates/muster-vt/src/key_mapping.rs` | `Key` onto `GhosttyKey`, the encoder seam's mapping |
| `Sources/MusterMac/KeyNames.generated.swift` | the W3C name a macOS virtual keycode carries, for the shell |

## Re-run it on every ghostty pin bump

That is the point of keeping it. A pin bump can rename or renumber `GhosttyKey`
members, and the three files above are the only place that mapping lives. Bump
`deps/ghostty.pin`, rebuild the ghostty checkout, run `./gen-keycodes`, and let it fail
loudly (see below) rather than silently mis-map a key.

**`./dev` refuses to build if you forget.** Each generated file records the pin it came
from, and the build compares that against `deps/ghostty.pin` before compiling anything.
What it is protecting against is the half that fails silently: a *renamed* key already
takes the build down, because `muster-vt` generates its bindings to the C enum fresh
every time, but an *added* key just has no row here - it reaches no pane, nothing
errors, and the suite stays green while that one key does nothing.

The check refuses rather than regenerating, which is the opposite of what the build does
for the seam's protobuf types beside it. Regenerating rewrites three committed files, and
a pin bump is the moment to read what moved: it can change the naming rule this derives
names by, and the two JIS keys deliberately left out below want re-checking each time in
case upstream has since named them.

## Why it doesn't just read ghostty's own `code_to_key` map

keycodes.zig builds its own DOM-code-to-key table from a hand-curated
`StaticStringMap` that has gaps: it has no entry for `AudioVolumeMute`,
`IntlBackslash`, `IntlRo`, `IntlYen`, or `NumpadComma`, even though all five have real
macOS keycodes and real `GhosttyKey` members. Trusting that map would carry the gap
into Muster as keys that silently do nothing. Instead, this script derives each
`GhosttyKey` name straight from its DOM code and validates the result against the
enum parsed from `event.h`, so a name it gets wrong fails the run instead of shipping.

The one case where a real macOS keycode genuinely has no `GhosttyKey` to map to is
`Lang1`/`Lang2` - kVK_JIS_Kana and kVK_JIS_Eisu, keys that exist only on JIS
keyboards, which ghostty's enum never modeled. That's a named, commented exception in
the script (`KNOWN_UNMAPPABLE_MAC_CODES`), not a silent skip; everything else that
fails to resolve is a hard error.

## Why Python

No third-party dependencies, and nothing else in the repo needs this table shape; it
is a dev tool, not a component, in the same spirit as `tools/herdr-probe`.
