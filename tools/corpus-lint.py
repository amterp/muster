#!/usr/bin/env python3
"""Is the conformance corpus actually being used, and does it meet its own contract?

A driver can check the files it loads. It cannot check the file nobody loads - and a
corpus that quietly stopped being read is the silently-skipped suite in a new costume,
which is the failure `docs/testing.md` treats as a standing hazard.

So this is the language-neutral authority on the corpus itself:

  - every case carries a non-empty `why`, because the reasoning in those cases is the
    documentation, and a row that does not say what it protects gets deleted by whoever
    it inconveniences;
  - every file declares `source` as recorded, ported or authored, so a reader knows how
    far to trust it, and a `recorded` file carries the command that re-derives it;
  - every file is claimed by at least one driver.

Runs in the default gate: it reads files and nothing else.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CORPUS = REPO / "corpus/conformance"
DRIVER_ROOTS = (REPO / "Tests", REPO / "crates")
SOURCES = {"recorded", "ported", "authored"}
# How every driver, in any language, is expected to name the file it runs. Swift spells the
# call `Conformance.load`, Rust `Conformance::load`, and a regex that knew only one of them
# would report a whole language's drivers as absent - which reads as a corpus nobody runs,
# exactly the alarm this file exists to raise.
CLAIM = re.compile(r"""Conformance(?:\.|::)load\(\s*["']([^"']+)["']""")


def problems_in(path: Path) -> list[str]:
    found = []
    name = path.name
    try:
        document = json.loads(path.read_text())
    except OSError as exc:
        return [f"{name}: unreadable ({exc})"]
    except ValueError as exc:
        # The corpus is full of terminal bytes, so this is nearly always one escape
        # sequence written as itself. JSON's own message names a column and not a cure.
        raw = path.read_bytes()
        stray = sorted({b for b in raw if b < 0x20 and b not in (0x09, 0x0A, 0x0D)})
        if stray:
            spelled = ", ".join(f"0x{b:02x} (write it as \\u{b:04x})" for b in stray)
            return [
                f"{name}: not JSON ({exc}). It carries raw control byte(s): {spelled}. "
                "A control character written as itself is not legal in a JSON string, and "
                "escape sequences are most of what this corpus is about."
            ]
        return [f"{name}: not JSON ({exc})"]

    if not document.get("concept"):
        found.append(f"{name}: no `concept`")
    if not document.get("why"):
        found.append(f"{name}: no file-level `why` saying what this corpus is for")

    source = document.get("source")
    if source not in SOURCES:
        found.append(
            f"{name}: `source` is {source!r}, must be one of {', '.join(sorted(SOURCES))}. "
            "It says how far these expectations can be trusted, and a file without one "
            "cannot be judged."
        )
    elif source == "recorded" and not document.get("regenerate"):
        found.append(
            f"{name}: declares `recorded` but carries no `regenerate` command, so its "
            "provenance is a claim rather than a fact."
        )
    elif source == "authored" and not document.get("citation"):
        found.append(
            f"{name}: declares `authored` but cites nothing. Authored cases have no oracle "
            "beyond their citation, so without one nothing distinguishes policy from guess."
        )

    cases = document.get("cases")
    if not isinstance(cases, list) or not cases:
        found.append(f"{name}: no cases. An empty corpus passes every driver.")
        return found

    seen = set()
    for index, case in enumerate(cases):
        label = case.get("name") or f"case {index}"
        if not case.get("name"):
            found.append(f"{name}: case {index} has no `name`")
        elif case["name"] in seen:
            found.append(f"{name}: two cases named {case['name']!r}")
        else:
            seen.add(case["name"])
        if not case.get("why"):
            found.append(f"{name}: case {label!r} has no `why`")
        if "given" not in case or "expect" not in case:
            found.append(f"{name}: case {label!r} has no `given`/`expect`")
    return found


def claimed_files() -> dict[str, set[str]]:
    """Which corpus files each language's drivers load."""
    claims: dict[str, set[str]] = {}
    for root in DRIVER_ROOTS:
        if not root.exists():
            continue
        for path in root.rglob("*"):
            if path.is_file() and path.suffix in {".swift", ".rs", ".py"}:
                for name in CLAIM.findall(path.read_text(errors="replace")):
                    claims.setdefault(name, set()).add(path.suffix.lstrip("."))
    return claims


def main() -> int:
    if not CORPUS.exists():
        print(f"corpus-lint: no {CORPUS.relative_to(REPO)} directory", file=sys.stderr)
        return 1

    files = sorted(CORPUS.glob("*.json"))
    if not files:
        print(f"corpus-lint: no cases under {CORPUS.relative_to(REPO)}", file=sys.stderr)
        return 1

    found = []
    total = 0
    for path in files:
        found.extend(problems_in(path))
        try:
            total += len(json.loads(path.read_text()).get("cases", []))
        except (ValueError, OSError):
            pass

    claims = claimed_files()
    for path in files:
        if path.name not in claims:
            found.append(
                f"{path.name}: no driver loads it. Its cases are checked in and never run, "
                "which reads as coverage and is not. Either write the driver or delete the "
                "file."
            )
    for claim in sorted(claims):
        if not (CORPUS / claim).exists():
            found.append(f"a driver loads {claim}, which does not exist")

    if found:
        print("corpus-lint: the conformance corpus does not meet its own contract.\n")
        for problem in found:
            print(f"  {problem}")
        print(f"\n{len(found)} problem(s). See docs/testing.md, 'The conformance corpus'.")
        return 1

    print(f"corpus-lint: {len(files)} file(s), {total} case(s), all claimed by a driver.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
