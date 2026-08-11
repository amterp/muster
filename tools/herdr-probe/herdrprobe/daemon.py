"""An isolated herdr daemon the probe owns end to end.

herdr resolves its config directory from XDG_CONFIG_HOME and keeps the control
socket inside it, so pointing the XDG variables at a scratch root yields a daemon
with its own socket, its own session state, and no view of the developer's real one.
Agent-detection manifests are copied in rather than fetched, so runs work offline.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import time
from pathlib import Path

from .client import Client


class DaemonError(RuntimeError):
    pass


# A Unix socket path must fit sockaddr_un.sun_path: 104 bytes on macOS, 108 on Linux.
# The socket lives inside the config dir, so the root has to stay short - the usual
# per-session scratch directories are already too long on their own.
DEFAULT_ROOT = Path("/tmp/muster-probe")
_SUN_PATH_MAX = 100


class IsolatedDaemon:
    def __init__(self, root: Path = DEFAULT_ROOT, herdr_bin: str = "herdr"):
        self.root = Path(root).resolve()
        self.herdr_bin = herdr_bin
        self.config_dir = self.root / "config" / "herdr"
        self.state_dir = self.root / "state"
        self.socket_path = self.config_dir / "herdr.sock"
        if len(str(self.socket_path)) > _SUN_PATH_MAX:
            raise DaemonError(
                f"probe root {self.root} is too long: it yields a {len(str(self.socket_path))}-byte "
                f"socket path, over the {_SUN_PATH_MAX}-byte limit.\n"
                f"  Impact: herdr server exits immediately with an InvalidInput socket error.\n"
                f"  Fix: pass a shorter --root, e.g. {DEFAULT_ROOT}."
            )
        self._process: subprocess.Popen | None = None

    @property
    def env(self) -> dict[str, str]:
        env = dict(os.environ)
        env.pop("HERDR_SOCKET_PATH", None)
        env.pop("HERDR_CLIENT_SOCKET_PATH", None)
        env.pop("HERDR_SESSION", None)
        env.update(
            HOME=str(self.root / "home"),
            XDG_CONFIG_HOME=str(self.root / "config"),
            XDG_STATE_HOME=str(self.state_dir),
            XDG_DATA_HOME=str(self.root / "data"),
            XDG_CACHE_HOME=str(self.root / "cache"),
            TERM="xterm-256color",
        )
        return env

    @property
    def screen_agent(self) -> Path:
        """The fake screen-detected agent, installed under a name herdr recognizes.

        herdr identifies an agent from the pane's foreground process name, and only
        names in its known-agent enum can carry an override manifest, so the fixture
        ships as `claude`.
        """
        return self.root / "bin" / "claude"

    def prepare(self, manifest_source: Path | None = None) -> None:
        for path in (self.config_dir, self.state_dir, self.root / "home",
                     self.root / "data", self.root / "cache", self.root / "bin"):
            path.mkdir(parents=True, exist_ok=True)
        (self.config_dir / "config.toml").write_text(_CONFIG_TOML)

        # Copy the real manifests in rather than letting the daemon fetch them, so a
        # run works offline and records the same rules every time.
        detection = self.state_dir / "herdr" / "agent-detection"
        detection.mkdir(parents=True, exist_ok=True)
        source = manifest_source or (Path.home() / ".local/state/herdr/agent-detection")
        if source.is_dir():
            shutil.copytree(source, detection, dirs_exist_ok=True)

        fixtures = Path(__file__).resolve().parents[1] / "fake-agent"
        override_dir = self.config_dir / "agent-detection"
        override_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy(fixtures / "claude.toml", override_dir / "claude.toml")
        shutil.copy(fixtures / "screen-agent", self.screen_agent)
        self.screen_agent.chmod(0o755)

    def start(self, timeout: float = 20.0) -> None:
        log = open(self.root / "server.log", "ab")
        self._process = subprocess.Popen(
            [self.herdr_bin, "server"], env=self.env, stdout=log, stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.socket_path.exists():
                try:
                    Client(self.socket_path, self.root, timeout=2.0).request("ping")
                    return
                except (OSError, ConnectionError):
                    pass
            if self._process.poll() is not None:
                raise DaemonError(
                    f"herdr server exited with code {self._process.returncode} before accepting a "
                    f"connection.\n  Impact: no scenario can run; the probe has no daemon.\n"
                    f"  Check {self.root / 'server.log'} - a port/socket collision, an unreadable "
                    f"config.toml, or an incompatible herdr build are the usual causes."
                )
            time.sleep(0.1)
        raise DaemonError(
            f"herdr server did not answer on {self.socket_path} within {timeout}s.\n"
            f"  Impact: no scenario can run.\n  Check {self.root / 'server.log'} for startup errors."
        )

    def cli(self, *args: str, check: bool = True, **kwargs) -> subprocess.CompletedProcess:
        """Run the herdr CLI against this daemon."""
        return subprocess.run(
            [self.herdr_bin, *args], env=self.env, capture_output=True, text=True, check=check, **kwargs
        )

    def client(self, timeout: float = 10.0) -> Client:
        return Client(self.socket_path, self.root, timeout=timeout)

    def stop(self) -> None:
        if self._process is None:
            return
        try:
            self.cli("server", "stop", check=False)
            self._process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self._process.terminate()
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
        finally:
            self._process = None

    def __enter__(self) -> IsolatedDaemon:
        return self

    def __exit__(self, *_exc) -> None:
        self.stop()


# Deterministic and offline. The shell is pinned to a non-login /bin/sh so the
# developer's dotfiles play no part in what the corpus records - a login zsh under a
# scratch HOME exits nonzero, which closes the pane, the workspace, and then the
# whole headless server.
_CONFIG_TOML = """\
[terminal]
default_shell = "/bin/sh"
shell_mode = "non_login"
new_cwd = "current"

[update]
version_check = false
manifest_check = false
"""
