#!/usr/bin/env python3
"""Reads a `cargo test` log and prints a command per failing test binary that re-runs only its
failed tests: `cargo test -p muster-seam --test seam -- --exact windows::a windows::b`.

A binary that failed with no test named - it crashed, or aborted - gets the command for the
whole binary. Prints nothing when the log shows no failure.

Usage: tools/failed-tests.py LOG
"""

import re
import shlex
import sys

FAILED = re.compile(r"^---- (.+) stdout ----$")
RERUN = re.compile(r"to rerun pass `([^`]+)`")


def reruns(lines):
    commands = []
    names = []
    for line in lines:
        line = line.rstrip("\n")
        failed = FAILED.match(line)
        if failed:
            names.append(failed.group(1))
            continue
        rerun = RERUN.search(line)
        if rerun:
            command = f"cargo test {rerun.group(1)}"
            if names:
                command += " -- --exact " + " ".join(shlex.quote(name) for name in names)
            commands.append(command)
            names = []
    return commands


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    with open(sys.argv[1], errors="replace") as log:
        for command in reruns(log):
            print(command)


if __name__ == "__main__":
    main()
