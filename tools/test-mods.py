#!/usr/bin/env python3
"""Is every Rust file compiled that a crate holds as code?

cargo builds only what a crate root reaches through `mod` lines. A file whose `mod` line is
missing compiles to nothing: its code and its tests never run, the gate stays green, and nothing
else notices. That is most likely for a test file, `tests.rs` beside the code it tests or a file
in an integration test binary, since nothing calls into one. This fails for every such file,
naming the line to add.

The roots are each crate's `src/lib.rs` and `src/main.rs`, each file or `main.rs` under
`src/bin/`, and each file or `<name>/main.rs` directly under `tests/` and `examples/`. A module's
own files are in its directory: `mod.rs`'s, a `main.rs` root's, or `foo/` beside `foo.rs`; and a
directory with a `mod.rs` directly under `tests/` or `examples/`, as `tests/support/`, is shared
by that directory's single-file roots, one of which must declare it. A directory that is none of
those is data, not code, and is left alone. Nothing here uses `#[path]`, and the `include!`s
only reach files cargo generates.

Runs in the default gate: it reads files and nothing else.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MOD = re.compile(r"^\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.MULTILINE)


def missing(
    directory: Path, declarers: list[Path], skip: tuple[str, ...] = ()
) -> list[tuple[list[Path], str]]:
    """The modules in `directory` that none of `declarers` declares, and those under them."""
    declared = {name for declarer in declarers for name in MOD.findall(declarer.read_text())}
    found = []
    for path in sorted(directory.iterdir()):
        if path in declarers or path.name in skip:
            continue
        if path.is_file() and path.suffix == ".rs":
            name = path.stem
            if path.with_suffix("").is_dir():
                found += missing(path.with_suffix(""), [path])
        elif path.is_dir() and (path / "mod.rs").is_file():
            name = path.name
            found += missing(path, [path / "mod.rs"])
        else:
            continue
        if name not in declared:
            found.append((declarers, name))
    return found


def roots() -> list[tuple[Path, list[Path], tuple[str, ...]]]:
    """Each directory a crate root declares modules in, with the roots that declare them."""
    found = []
    for src in sorted(REPO.glob("crates/*/src")):
        crate = [root for root in (src / "lib.rs", src / "main.rs") if root.is_file()]
        if crate:
            found.append((src, crate, ("bin",)))
        for binary in sorted((src / "bin").glob("*.rs")):
            if binary.with_suffix("").is_dir():
                found.append((binary.with_suffix(""), [binary], ()))
        for main in sorted((src / "bin").glob("*/main.rs")):
            found.append((main.parent, [main], ()))
    for targets in sorted([*REPO.glob("crates/*/tests"), *REPO.glob("crates/*/examples")]):
        for main in sorted(targets.glob("*/main.rs")):
            found.append((main.parent, [main], ()))
        single = sorted(targets.glob("*.rs"))
        if single:
            binaries = tuple(main.parent.name for main in targets.glob("*/main.rs"))
            found.append((targets, single, binaries))
    return found


def main() -> int:
    checked = roots()
    if not checked:
        print("test-mods: no crate roots found, so nothing was checked", file=sys.stderr)
        return 1
    problems = [
        problem
        for directory, declarers, skip in checked
        for problem in missing(directory, declarers, skip)
    ]
    for declarers, name in problems:
        relative = declarers[0].relative_to(REPO)
        # Several declarers are the single-file roots of one tests/ or examples/ directory.
        missing_line = (
            f"no file directly in {relative.parent} declares"
            if len(declarers) > 1
            else f"{relative} does not declare"
        )
        # Only src/ holds tests beside code; a test or example target is compiled as itself.
        hint = " (under #[cfg(test)] if it holds tests)" if relative.parts[2] == "src" else ""
        print(
            f"{missing_line} `mod {name};`, so {name} is never compiled and its tests never "
            f"run. Add the line{hint}, or delete the file.",
            file=sys.stderr,
        )
    if problems:
        return 1
    print(f"test-mods: every Rust module file is declared, under {len(checked)} roots")
    return 0


if __name__ == "__main__":
    sys.exit(main())
