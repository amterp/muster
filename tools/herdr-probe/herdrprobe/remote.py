"""The same scenarios, run against the daemon in the devenv container over SSH.

Muster's whole premise is that a local daemon and one on an SSH devenv are rendered
and driven identically, so the probe should be able to say whether they behave
identically. It answers that by running one scenario set against both and diffing the
corpora.

The control socket is reached through an SSH Unix-domain forward, so the client code
is the same code the local runs use. Anything that shells out goes through
`herdr_argv`, which prefixes ssh instead of running the local binary.
"""

from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

from .client import Client
from .daemon import DaemonError, _SUN_PATH_MAX

DEFAULT_REMOTE_ROOT = Path("/tmp/muster-devenv")
REMOTE_SOCKET = "/home/dev/.config/herdr/herdr.sock"
REMOTE_SESSION = "/home/dev/.config/herdr/session.json"


class RemoteDaemon:
    """The devenv container's herdr, reached over SSH."""

    def __init__(self, key: Path, port: int = 2222, user: str = "dev", host: str = "localhost",
                 root: Path = DEFAULT_REMOTE_ROOT):
        self.root = Path(root).resolve()
        self.socket_path = self.root / "herdr.sock"
        if len(str(self.socket_path)) > _SUN_PATH_MAX:
            raise DaemonError(f"forwarded socket path {self.socket_path} is too long for sockaddr_un")
        self.target = f"{user}@{host}"
        self.ssh_opts = [
            "-i", str(key), "-p", str(port),
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
        ]
        self._forward: subprocess.Popen | None = None

    @property
    def env(self) -> dict[str, str]:
        return dict(os.environ)

    @property
    def herdr_bin(self) -> str:
        return "herdr"

    def herdr_argv(self, *args: str) -> list[str]:
        return ["ssh", *self.ssh_opts, self.target, "herdr", *args]

    @property
    def screen_agent(self) -> str:
        return "/usr/local/bin/claude"

    def ssh(self, command: str, check: bool = True) -> subprocess.CompletedProcess:
        return subprocess.run(["ssh", *self.ssh_opts, self.target, command],
                              capture_output=True, text=True, check=check)

    def prepare(self, manifest_source=None) -> None:
        """The image already carries the daemon config, manifests, and fake agents.

        What is left is giving this scenario a session with no leftovers, which the local
        runs get from a fresh scratch root. Here rather than in `start`, and the difference
        is not cosmetic: the durability scenario restarts the daemon mid-run to see what
        survives, and a start that wiped the session would answer "nothing" every time and
        record it as a platform difference.
        """
        self.root.mkdir(parents=True, exist_ok=True)
        self.socket_path.unlink(missing_ok=True)
        self.ssh("herdr server stop >/dev/null 2>&1; sleep 0.3; "
                 f"rm -f {REMOTE_SESSION}", check=False)

    def start(self, timeout: float = 30.0) -> None:
        """Starts the container's daemon, then forwards its socket to this machine.

        Whatever session file is there is left alone, so that a stop followed by a start is
        the restart a scenario means by it. Emptying the session belongs to `prepare`.
        """
        self.ssh("setsid nohup herdr server >>/home/dev/herdr-server.out 2>&1 < /dev/null & "
                 "sleep 0.2", check=False)

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.ssh(f"test -S {REMOTE_SOCKET}", check=False).returncode == 0:
                break
            time.sleep(0.2)
        else:
            raise DaemonError(
                f"the devenv daemon did not create {REMOTE_SOCKET} within {timeout}s.\n"
                f"  Impact: no remote scenario can run.\n"
                f"  Check: ./devenv/devenv status, and "
                f"docker exec muster-devenv cat /home/dev/herdr-server.out"
            )

        self.socket_path.unlink(missing_ok=True)
        self._forward = subprocess.Popen(
            ["ssh", *self.ssh_opts, "-N", "-L", f"{self.socket_path}:{REMOTE_SOCKET}", self.target],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.socket_path.exists():
                try:
                    Client(self.socket_path, self.root, timeout=2.0).request("ping")
                    return
                except (OSError, ConnectionError):
                    pass
            if self._forward.poll() is not None:
                err = self._forward.stderr.read().decode(errors="replace")
                raise DaemonError(
                    f"the SSH socket forward exited: {err.strip()}\n"
                    f"  Impact: the probe cannot reach the devenv daemon's control socket.\n"
                    f"  Check that the container is up and the key in devenv/.ssh/ still matches it."
                )
            time.sleep(0.1)
        raise DaemonError(f"forwarded socket {self.socket_path} never answered a ping within {timeout}s")

    def client(self, timeout: float = 10.0) -> Client:
        return Client(self.socket_path, self.root, timeout=timeout)

    def cli(self, *args: str, check: bool = True, **kwargs) -> subprocess.CompletedProcess:
        return subprocess.run(self.herdr_argv(*args), capture_output=True, text=True, check=check, **kwargs)

    def stop(self) -> None:
        """Stops the daemon over there, and the forward that reached it.

        Both, because a scenario that stops a daemon means the daemon: leaving it running
        and only dropping the tunnel would make the restart it is about invisible, and the
        facts it records would describe nothing that happened.
        """
        self.ssh("herdr server stop >/dev/null 2>&1", check=False)
        if self._forward is not None:
            self._forward.terminate()
            try:
                self._forward.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._forward.kill()
            self._forward = None
        self.socket_path.unlink(missing_ok=True)

    def leave_running(self, timeout: float = 30.0) -> None:
        """Puts the container back the way its image intends to find it.

        The entrypoint starts a daemon and every other thing that talks to this container -
        `devenv status`, the remote tier's tests - assumes one is there. A probe run ends
        with the daemon stopped, because the last scenario's teardown stops it, so the run
        has to put it back rather than leaving the fixture broken for whatever comes next.
        """
        self.ssh("setsid nohup herdr server >>/home/dev/herdr-server.out 2>&1 < /dev/null & "
                 "sleep 0.2", check=False)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.ssh(f"test -S {REMOTE_SOCKET}", check=False).returncode == 0:
                return
            time.sleep(0.2)
        raise DaemonError(
            f"the devenv daemon did not come back within {timeout}s after the probe run.\n"
            f"  Impact: the container is left with no daemon, so anything else that talks to\n"
            f"  it will fail with a connection that opens and then refuses.\n"
            f"  Fix: ./devenv/devenv rebuild"
        )

    def __enter__(self) -> RemoteDaemon:
        return self

    def __exit__(self, *_exc) -> None:
        self.stop()
