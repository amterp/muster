#!/usr/bin/env python3
"""Run dev's timing recorder twice in one directory without building the project."""

import re
import subprocess
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def wait_for(path: Path, process: subprocess.Popen) -> None:
    deadline = time.monotonic() + 10
    while not path.exists():
        if process.poll() is not None or time.monotonic() >= deadline:
            raise AssertionError(f"timing process did not reach {path.name}")
        time.sleep(0.01)


def main() -> None:
    source = (REPO / "dev").read_text()
    constants = "\n".join(
        line for line in source.splitlines() if re.match(r"(?:GATE_STEPS|RUN_STARTED) =", line)
    )
    functions = source[source.index("fn announce(") : source.index("// What the machine is doing,")]
    start = source.index('\nquiet $`mkdir -p target/gate`', source.index("report_machine(cores, fast_cores"))
    initialize = source[start : source.index("\nif doctor:", start)]
    with tempfile.TemporaryDirectory(prefix="muster-dev-timing-") as directory:
        root = Path(directory)
        processes = []
        try:
            for name, status in [("first", 7), ("second", 0)]:
                script = root / f"{name}.rad"
                script.write_text(
                    constants + "\n" + functions + initialize
                    + f'\nannounce("{name}")\n'
                    + f'write_file("{name}.ready", "")\n'
                    + f'while not get_path("{name}.release").exists:\n    sleep(0.01)\n'
                    + f'mark("{name}: finished")\nreport_steps()\nexit({status})\n'
                )
                process = subprocess.Popen(
                    ["rad", str(script)], cwd=root, text=True,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                )
                processes.append((name, status, process))
                wait_for(root / f"{name}.ready", process)
            paths = []
            for name, status, process in processes:
                (root / f"{name}.release").touch()
                stdout, stderr = process.communicate(timeout=10)
                assert process.returncode == status, (stdout, stderr)
                other = "second" if name == "first" else "first"
                assert other not in stdout, stdout
                assert stdout.count("▸ time") == 1, stdout
                assert "finished" in stdout and "total" in stdout, stdout
                reported = [
                    line.strip()[7:] for line in stdout.splitlines()
                    if line.strip().startswith("marks: ")
                ]
                assert len(reported) == 1, f"footer must identify its timing file: {stdout}"
                path = root / reported[0]
                contents = path.read_text()
                assert f"\t{name}\n" in contents and f"\t{name}: finished\n" in contents, contents
                assert other not in contents, contents
                paths.append(path)
            assert paths[0] != paths[1], paths
        finally:
            for _, _, process in processes:
                if process.poll() is None:
                    process.kill()
                process.communicate()
    print("dev timing: concurrent success and failure footers stay separate")


if __name__ == "__main__":
    main()
