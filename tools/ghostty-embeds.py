#!/usr/bin/env python3
"""Write licenses/GHOSTTY-EMBEDDED.md: what libghostty brings in with it.

Muster's NOTICE has always said it links libghostty, which is MIT. That was
true and incomplete. libghostty is built from a Zig package set of 38
dependencies, and `libghostty-internal.a` - statically linked into
Contents/MacOS/muster - carries the object code of ten of them, including one
under the LGPL. None of that is visible from the bundle, from Cargo.lock, or
from Package.resolved.

Which ten is measured rather than read off the manifest, because a static
archive only contributes the members the linker actually pulls. The probe below
asks the *linked* binary for symbols that only one library defines. On macOS
that answer is narrower than the manifest suggests: harfbuzz, fontconfig and
libxml2 are all in the package set and none of them links, because text shaping
here goes through CoreText.

Run this when deps/ghostty.pin moves, and commit what it writes - a re-pin is
exactly when this answer changes. It needs a built binary, so run a build first.
Pass --print to see the probe's answer without writing the file.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGES = ROOT / "deps" / "ghostty" / "zig-pkg"
MANIFEST = ROOT / "deps" / "ghostty" / "build.zig.zon.json"
PIN = ROOT / "deps" / "ghostty.pin"
OUTPUT = ROOT / "licenses" / "GHOSTTY-EMBEDDED.md"

# The binary the archive is linked into. Present after `./dev -b`, and the same
# link the bundle copies - so this can be answered without assembling one.
BINARY = ROOT / ".build" / "arm64-apple-macosx" / "debug" / "muster"

# name -> (symbol pattern that only this library defines, SPDX, license files to
# reproduce). A pattern matching nothing is reported rather than skipped: a
# library that stopped linking and a probe that stopped working look identical
# from here, and only one of them is good news.
PROBES = [
    ("gettext", r"_libintl_(gettext|dgettext|bindtextdomain)$", "LGPL-2.1-or-later",
     ["gettext-runtime/intl/COPYING.LIB", "gettext-runtime/COPYING"]),
    ("breakpad", r"google_breakpad", "BSD-3-Clause", ["LICENSE"]),
    ("sentry", r"_sentry_(init|options_new)$", "MIT", ["LICENSE"]),
    ("freetype", r"_FT_(Init_FreeType|Load_Glyph)$", "FTL OR GPL-2.0-or-later",
     ["LICENSE.TXT"]),
    ("libpng", r"_png_(create_read_struct|read_info)", "libpng-2.0", ["LICENSE"]),
    ("zlib", r"_(inflateInit|deflateInit)", "Zlib", ["LICENSE"]),
    ("oniguruma", r"_onig_(new|search)$", "BSD-2-Clause", ["COPYING"]),
    ("wuffs", r"wuffs_(base|png)__", "Apache-2.0 OR MIT", ["LICENSE"]),
    ("spirv_cross", r"spirv_cross|SPIRV", "Apache-2.0", ["LICENSE"]),
    ("glslang", r"glslang", "BSD-3-Clause AND Apache-2.0", ["LICENSE.txt"]),
]


def package_dirs() -> dict[str, Path]:
    manifest = json.loads(MANIFEST.read_text())
    return {
        (v.get("name") or key): PACKAGES / key for key, v in manifest.items()
    }


def symbols(binary: Path) -> str:
    out = subprocess.run(["nm", "-a", str(binary)], capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout:
        sys.exit(
            f"nm could not read {binary}.\n"
            "Impact: what libghostty statically embeds cannot be measured, so the\n"
            "attribution beside it cannot be written or trusted. The binary is\n"
            "produced by a build - try: ./dev -b"
        )
    return out.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--print", action="store_true", dest="show",
                        help="report the probe's answer without writing the file")
    args = parser.parse_args()

    if not BINARY.exists():
        sys.exit(f"{BINARY.relative_to(ROOT)} does not exist. Try: ./dev -b")

    table = symbols(BINARY)
    dirs = package_dirs()
    pin = PIN.read_text().strip()

    linked, absent = [], []
    for name, pattern, spdx, files in PROBES:
        # MULTILINE because nm prints one symbol per line and several of these
        # patterns anchor with $ - without it they only ever match the last line,
        # which reads exactly like a library that stopped linking.
        hits = len(re.findall(pattern, table, re.MULTILINE))
        (linked if hits else absent).append((name, spdx, files, hits))

    if args.show:
        for name, spdx, _, hits in linked:
            print(f"{name:12} {hits:>6} symbols  {spdx}")
        for name, spdx, _, _ in absent:
            print(f"{name:12} {'-':>6}           {spdx}  (no longer linking)")
        return 0

    if absent:
        print(
            "These libraries no longer contribute any symbol: "
            f"{', '.join(n for n, _, _, _ in absent)}.\n"
            "Either the pin dropped them, which is good news and means their entry\n"
            "here can go, or the symbol this probes for was renamed, which means the\n"
            "probe is now silently measuring nothing. Check before removing them from\n"
            "PROBES in this file.",
            file=sys.stderr,
        )

    lines: list[str] = []
    w = lines.append
    w("# Software libghostty carries into Muster")
    w("")
    w("Generated by `tools/ghostty-embeds.py`; edit that rather than this file.")
    w("")
    w("Muster links libghostty, which is MIT. libghostty is itself built from a Zig")
    w("package set, and the static archive it produces carries the object code of the")
    w("libraries below into `Contents/MacOS/muster`. `NOTICE` says libghostty is here;")
    w("this file is what that turns out to mean.")
    w("")
    w(f"Measured against the build of `deps/ghostty.pin` = `{pin}`, by asking the linked")
    w("binary for symbols only one library defines. Re-run it when the pin moves.")
    w("")
    w("`Contents/Frameworks/libghostty-vt.dylib` is the other build of the same source")
    w("and carries none of these - it is the terminal state machine and nothing else.")
    w("")
    w("| Library | License | Symbols in the linked binary |")
    w("| --- | --- | --- |")
    for name, spdx, _, hits in linked:
        w(f"| {name} | {spdx} | {hits} |")
    w("")
    w("**gettext is the one that asks for something beyond attribution.** What links in")
    w("is `gettext-runtime/intl`, which is LGPL-2.1-or-later - not the GPL half of the")
    w("package, which is the tools and does not ship. Static linking under the LGPL asks")
    w("that whoever has the binary be able to relink it against a modified libintl, and")
    w("Muster is in an unusually good position to offer that rather than a written")
    w("promise of it: the whole source is public and Apache-2.0, `deps/ghostty.pin` names")
    w("the commit that produced libghostty, and that build resolves gettext at a pinned")
    w("hash - so `./dev` rebuilds the entire chain from source on any machine, with")
    w("whatever libintl you put in it.")
    w("")
    w("## License texts")
    w("")

    for name, spdx, files, _ in linked:
        base = dirs.get(name)
        if base is None or not base.exists():
            sys.exit(
                f"No package directory for {name} under {PACKAGES.relative_to(ROOT)}.\n"
                "Impact: its license text cannot be reproduced, so the bundle would\n"
                "ship the library and not its terms. Try: ./dev -d"
            )
        for relative in files:
            path = base / relative
            if not path.exists():
                sys.exit(
                    f"{name} no longer publishes {relative}.\n"
                    "Impact: the text this file is supposed to carry is missing, and a\n"
                    "silently shorter file is the failure this exists to prevent. Look\n"
                    f"in {base.relative_to(ROOT)} for where it moved to and update PROBES."
                )
            w(f"### {name} - {relative} ({spdx})")
            w("")
            w("```")
            w(path.read_text(encoding="utf-8", errors="replace").strip())
            w("```")
            w("")

    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text("\n".join(lines).rstrip() + "\n")
    print(
        f"wrote {OUTPUT.relative_to(ROOT)}: "
        f"{len(linked)} libraries linked into the binary"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
