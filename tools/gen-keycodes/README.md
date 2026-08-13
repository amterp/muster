# gen-keycodes

Generates the two tables the input path needs to turn a macOS key press into a
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
| `Sources/MusterCore/Input/Key.generated.swift` | `Key`, Muster's own physical-key enum, and `Key(macOSKeycode:)` |
| `Sources/MusterVT/KeyMapping.generated.swift` | `Key.ghosttyKey`, the encoder-seam mapping onto `GhosttyKey` |

## Re-run it on every ghostty pin bump

That is the point of keeping it. A pin bump can rename or renumber `GhosttyKey`
members, and the two files above are the only place that mapping lives - hand-editing
them after a bump is exactly the kind of drift `docs/testing.md` calls Muster's top
false-green risk. Bump `deps/ghostty.pin`, rebuild the ghostty checkout, run
`./gen-keycodes`, and let it fail loudly (see below) rather than silently mis-map a key.

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
