#!/usr/bin/env python3
"""Is every test file compiled?

Each crate's integration tests are one binary, `crates/<crate>/tests/<name>/main.rs`, and every
other file in that directory is a module of it. A file whose `mod` line is missing compiles to
nothing: its tests never run, the gate stays green, and nothing else notices, since cargo builds
only what `main.rs` reaches. This fails for every such file, naming the line to add.

A directory under a binary holding a `mod.rs` is a module too, and the same holds inside it.
A directory with no `mod.rs` is data, not code, and is left alone.

Runs in the default gate: it reads files and nothing else.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MOD = re.compile(r"^\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.MULTILINE)


def missing(directory: Path, root: Path) -> list[tuple[Path, str]]:
    """The modules under `directory` that `root` (its main.rs or mod.rs) does not declare."""
    declared = set(MOD.findall(root.read_text()))
    found = []
    for path in sorted(directory.iterdir()):
        if path == root:
            continue
        if path.is_file() and path.suffix == ".rs":
            name = path.stem
        elif path.is_dir() and (path / "mod.rs").is_file():
            name = path.name
            found += missing(path, path / "mod.rs")
        else:
            continue
        if name not in declared:
            found.append((root, name))
    return found


def main() -> int:
    binaries = sorted(REPO.glob("crates/*/tests/*/main.rs"))
    if not binaries:
        print("test-mods: no crates/*/tests/*/main.rs found, so nothing was checked", file=sys.stderr)
        return 1
    problems = [problem for main in binaries for problem in missing(main.parent, main)]
    for root, name in problems:
        print(
            f"{root.relative_to(REPO)} does not declare `mod {name};`, so the tests in "
            f"{name} are never compiled or run. Add the line, or delete the file.",
            file=sys.stderr,
        )
    if problems:
        return 1
    print(f"test-mods: every test file is declared in one of {len(binaries)} test binaries")
    return 0


if __name__ == "__main__":
    sys.exit(main())
