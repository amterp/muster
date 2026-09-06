#!/usr/bin/env python3
"""What every wait in a test run actually cost, per site.

Reads the lines `herdr_harness::until` writes when `MUSTER_WAIT_LOG` is set, and prints one row
per wait site. Anything else on the stream is ignored, so a whole `cargo test` transcript can be
piped in without filtering it first:

    MUSTER_WAIT_LOG=1 cargo test -p muster-seam --test respawn -- --nocapture 2>&1 \
      | python3 tools/wait-margins.py

The column that answers the question is `margin`, the gap between the slowest observed wait and
the allowance it had. A site whose slowest run is milliseconds is not short of room however
often it fails, and `docs/testing.md` says what that shape means: no deadline would have made a
difference, so the thing to fix is whatever wedged.

`n` is worth reading beside it. One run says nothing here - the whole point of the tool is that
a distribution over many runs is the evidence, and a max drawn from three samples is an anecdote
with a table around it.
"""

import sys
from collections import defaultdict


def percentile(sorted_values, fraction):
    """The value at `fraction` through the sample, nearest-rank.

    Nearest-rank rather than interpolating: these are milliseconds from a handful of runs, and
    an interpolated p95 invents a number no run produced.
    """
    if not sorted_values:
        return 0
    rank = max(1, round(fraction * len(sorted_values)))
    return sorted_values[min(rank, len(sorted_values)) - 1]


def main():
    waits = defaultdict(list)
    allowances = {}
    for line in sys.stdin:
        fields = line.rstrip("\n").split("\t")
        if len(fields) != 4 or fields[0] != "wait":
            continue
        try:
            waited, allowance = int(fields[1]), int(fields[2])
        except ValueError:
            continue
        what = fields[3]
        waits[what].append(waited)
        # Last one wins, and they should all agree: an allowance is a property of the call site.
        # A site that disagrees with itself is worth seeing rather than averaging away, and the
        # row's margin will look wrong in a way that says so.
        allowances[what] = allowance

    if not waits:
        print("no waits recorded - was MUSTER_WAIT_LOG set, and did the run reach a wait?")
        return 1

    rows = []
    for what, observed in waits.items():
        observed.sort()
        allowance = allowances[what]
        rows.append(
            {
                "n": len(observed),
                "p50": percentile(observed, 0.50),
                "p95": percentile(observed, 0.95),
                "max": observed[-1],
                "allowance": allowance,
                "margin": allowance - observed[-1],
                "what": what,
            }
        )
    # Tightest margin first: the row most likely to fail next is the one to read.
    rows.sort(key=lambda row: row["margin"])

    print(f"{'n':>5} {'p50':>8} {'p95':>8} {'max':>8} {'allow':>8} {'margin':>8}  what")
    for row in rows:
        print(
            f"{row['n']:>5} {row['p50']:>8} {row['p95']:>8} {row['max']:>8} "
            f"{row['allowance']:>8} {row['margin']:>8}  {row['what']}"
        )
    print("\nmilliseconds. margin = allowance - slowest observed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
