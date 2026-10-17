#!/usr/bin/env python3
"""Run the workspace's Rust tests with their binaries side by side: what `cargo test --workspace
--no-fail-fast` runs, in a fraction of the time.

cargo runs one test binary at a time, so a gate spent most of its Rust suite waiting on the
longest binaries in turn, with cores idle. And a second `cargo test` beside the first waits on
the build directory's lock, so the binaries are run here directly, the way cargo runs them: from
their package's directory, with the variables cargo gives them.

A binary named with --split runs each of its tests in a process of its own. That is for the
seam's tests, which share one session per process and so take turns behind a lock
(`muster::testing::fresh_session`); one test to a process, nothing waits for it. And for the
longest binary, whose tests then fill whatever the others leave rather than running last on
threads of their own.

What a red run needs stays as cargo has it. Every job runs whatever else fails. A failing
binary's output comes out in one piece, followed by the line cargo would print, `error: test
failed, to rerun pass `...``, which is what tools/failed-tests.py reads. The exit status is 101,
cargo's, when anything failed.

How long each job took is kept in target/gate/test-times.json, and the next run starts the
longest first, so the slowest binary is never the one left running alone at the end.

Usage: tools/run-tests.py [--jobs N] [--split PACKAGE:TARGET]...
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

TIMES = Path("target/gate/test-times.json")


class Binary:
    """One test executable, as `cargo test --no-run` built it."""

    def __init__(self, artifact):
        self.executable = artifact["executable"]
        self.target = artifact["target"]["name"]
        self.kind = artifact["target"]["kind"]
        self.manifest = Path(artifact["manifest_path"])
        with open(self.manifest, "rb") as manifest:
            self.package = tomllib.load(manifest)["package"]["name"]

    def key(self):
        return f"{self.package}:{self.target}"

    def rerun(self):
        """The arguments that make cargo run this binary alone."""
        if "test" in self.kind:
            which = f"--test {self.target}"
        elif "bin" in self.kind:
            which = f"--bin {self.target}"
        else:
            which = "--lib"
        return f"-p {self.package} {which}"

    def environment(self):
        # What cargo sets for a test it runs, so a test reads the same either way.
        return {
            **os.environ,
            "CARGO_MANIFEST_DIR": str(self.manifest.parent),
            "CARGO_PKG_NAME": self.package,
            "CARGO_CRATE_NAME": self.target.replace("-", "_"),
        }

    def tests(self, ignored):
        flags = ["--list", "--format", "terse"] + (["--ignored"] if ignored else [])
        listed = subprocess.run(
            [self.executable, *flags],
            cwd=self.manifest.parent,
            env=self.environment(),
            capture_output=True,
            text=True,
            check=True,
        )
        lines = listed.stdout.splitlines()
        return [line[: -len(": test")] for line in lines if line.endswith(": test")]


class Job:
    """A binary run whole, one of its tests, or the doc tests."""

    def __init__(self, key, argv, cwd, env, binary):
        self.key = key
        self.argv = argv
        self.cwd = cwd
        self.env = env
        self.binary = binary
        self.output = ""
        self.passed = False
        self.started = 0.0
        self.ended = 0.0

    def group(self):
        """The binary this job is part of, which is what a line of the report is about."""
        return self.binary.key() if self.binary else self.key


def built_binaries():
    built = subprocess.run(
        ["cargo", "test", "--workspace", "--no-run", "--message-format=json"],
        stdout=subprocess.PIPE,
        check=True,
        text=True,
    )
    binaries = []
    for line in built.stdout.splitlines():
        message = json.loads(line)
        if (
            message.get("reason") == "compiler-artifact"
            and message.get("executable")
            and message["profile"]["test"]
        ):
            binaries.append(Binary(message))
    return binaries


def jobs_for(binaries, split):
    jobs = []
    for binary in binaries:
        if binary.key() in split:
            ignored = set(binary.tests(ignored=True))
            for test in binary.tests(ignored=False):
                if test in ignored:
                    continue
                argv = [binary.executable, "--exact", test]
                jobs.append(Job(f"{binary.key()}::{test}", argv, binary.manifest.parent,
                                binary.environment(), binary))
        else:
            jobs.append(Job(binary.key(), [binary.executable], binary.manifest.parent,
                            binary.environment(), binary))
    # Doc tests only cargo can run, and nothing else here takes its lock.
    jobs.append(Job("doc", ["cargo", "test", "--workspace", "--doc", "--no-fail-fast"],
                    Path.cwd(), dict(os.environ), None))
    return jobs


def by_longest_known(jobs):
    try:
        known = json.loads(TIMES.read_text())
    except (OSError, ValueError):
        known = {}
    # A job never timed goes first among the whole binaries, since an unknown binary is the
    # likelier long one; one of many split tests is short either way.
    def expected(job):
        if job.key in known:
            return known[job.key]
        return 1.0 if "::" in job.key else 1e9
    return sorted(jobs, key=expected, reverse=True)


RUNNING = set()
RUNNING_LOCK = threading.Lock()


def run(job):
    job.started = time.monotonic()
    # A file rather than a pipe, and the process waited for rather than the pipe's end: a test
    # that leaves a child behind leaves it holding whatever its output was, and a pipe would
    # keep this job open until that child ended. cargo waits for the binary alone, too.
    with tempfile.TemporaryFile() as output:
        # A process group of its own, so ending the job ends the daemons its tests started too:
        # a test killed part way never stops them itself.
        process = subprocess.Popen(job.argv, cwd=job.cwd, env=job.env, stdout=output,
                                   stderr=subprocess.STDOUT, process_group=0)
        with RUNNING_LOCK:
            RUNNING.add(process)
        process.wait()
        with RUNNING_LOCK:
            RUNNING.discard(process)
        output.seek(0)
        job.output = output.read().decode(errors="replace")
    job.passed = process.returncode == 0
    job.ended = time.monotonic()
    return job


def stop_everything(signum, _frame):
    """Ends what is running and leaves at once: an orderly exit would wait for the pool, which
    starts every job still queued."""
    with RUNNING_LOCK:
        for process in RUNNING:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    os._exit(128 + signum)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    # Past the cores, because these tests mostly wait: sixteen at once on a ten-core Mac kept
    # two cores busy on average, and twenty-four took the load to 17 and failed tests that
    # time themselves.
    parser.add_argument("--jobs", type=int, default=(os.cpu_count() or 4) * 3 // 2,
                        help="how many jobs run at once (default: one and a half per core)")
    parser.add_argument("--split", action="append", default=[], metavar="PACKAGE:TARGET",
                        help="run each test of this binary in a process of its own")
    arguments = parser.parse_args()
    # Each job's group is its own, so a ^C at the terminal reaches only this process.
    for stopping in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(stopping, stop_everything)

    began = time.monotonic()
    binaries = built_binaries()
    jobs = by_longest_known(jobs_for(binaries, set(arguments.split)))
    print(f"{len(jobs)} jobs from {len(binaries)} test binaries and the doc tests, "
          f"{arguments.jobs} at a time", flush=True)

    # Per binary: how many of its jobs are still to finish, and the ones that have.
    remaining = {}
    for job in jobs:
        remaining[job.group()] = remaining.get(job.group(), 0) + 1
    finished = {group: [] for group in remaining}
    failed_binaries = []
    times = {}
    with ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        for future in as_completed([pool.submit(run, job) for job in jobs]):
            job = future.result()
            times[job.key] = round(job.ended - job.started, 2)
            finished[job.group()].append(job)
            remaining[job.group()] -= 1
            if remaining[job.group()] == 0:
                report(job.group(), finished[job.group()], failed_binaries)

    TIMES.parent.mkdir(parents=True, exist_ok=True)
    TIMES.write_text(json.dumps(times, indent=1, sort_keys=True))
    wall = time.monotonic() - began
    if failed_binaries:
        print(f"\n{len(failed_binaries)} of {len(finished)} test binaries failed, in {wall:.1f}s: "
              + ", ".join(failed_binaries), flush=True)
        sys.exit(101)
    print(f"\nevery test binary passed, in {wall:.1f}s", flush=True)


def report(key, jobs, failed_binaries):
    """Says how a binary did once its last job ends: one line, and all of a failure's output."""
    # From its first job's start to its last one's end, which for a split binary is not the
    # sum of its tests.
    seconds = max(job.ended for job in jobs) - min(job.started for job in jobs)
    count = f"{len(jobs)} tests, " if len(jobs) > 1 else ""
    failures = [job for job in jobs if not job.passed]
    if not failures:
        print(f"  ok      {key}  ({count}{seconds:.1f}s)", flush=True)
        return
    failed_binaries.append(key)
    print(f"  FAILED  {key}  ({count}{len(failures)} failed, {seconds:.1f}s)\n", flush=True)
    for job in failures:
        print(job.output, flush=True)
    binary = jobs[0].binary
    if binary is not None:
        # The line tools/failed-tests.py reads, after the failures it names.
        print(f"error: test failed, to rerun pass `{binary.rerun()}`\n", flush=True)


if __name__ == "__main__":
    main()
