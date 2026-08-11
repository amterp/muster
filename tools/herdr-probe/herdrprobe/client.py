"""Client for a herdr daemon's control socket.

The wire is newline-delimited JSON. Two things about it shape this client, both
observed rather than documented:

- **One request per connection.** The server answers a request and closes. There is
  no long-lived request/response channel; every call pays a connect().
- **Subscriptions hold their connection open.** `events.subscribe` answers
  `subscription_started` and then streams `{event, data}` frames until either side
  hangs up. Subscribing also replays the current session as synthetic events.
"""

from __future__ import annotations

import json
import socket
import threading
import uuid
from pathlib import Path
from typing import Any, Callable


class ProtectedSocketError(RuntimeError):
    """Raised when a client is pointed at a daemon the probe does not own."""


class RequestError(RuntimeError):
    def __init__(self, method: str, body: Any):
        self.method = method
        self.body = body
        super().__init__(f"{method} failed: {json.dumps(body)}")


class Client:
    def __init__(self, socket_path: Path, owned_root: Path, timeout: float = 10.0):
        _assert_owned(socket_path, owned_root)
        self.socket_path = Path(socket_path)
        self.owned_root = Path(owned_root)
        self.timeout = timeout

    def request(self, method: str, params: dict | None = None) -> Any:
        req = {"id": f"probe-{uuid.uuid4().hex[:12]}", "method": method, "params": params or {}}
        sock = self._connect(self.timeout)
        try:
            sock.sendall((json.dumps(req) + "\n").encode())
            msg = _read_line(sock, bytearray())[0]
        finally:
            sock.close()
        if "error" in msg:
            raise RequestError(method, msg["error"])
        return msg.get("result")

    def subscribe(self, subscriptions: list[dict]) -> EventStream:
        return EventStream(self, subscriptions)

    def _connect(self, timeout: float | None) -> socket.socket:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(timeout)
        sock.connect(str(self.socket_path))
        return sock


class EventStream:
    """A connection held open on `events.subscribe`, drained by a background thread."""

    def __init__(self, client: Client, subscriptions: list[dict]):
        self._sock = client._connect(None)
        self.events: list[dict] = []
        self._cv = threading.Condition()
        self._closed = False
        req = {"id": "subscribe", "method": "events.subscribe", "params": {"subscriptions": subscriptions}}
        self._sock.sendall((json.dumps(req) + "\n").encode())
        buf = bytearray()
        ack, buf = _read_line(self._sock, buf)
        if "error" in ack:
            raise RequestError("events.subscribe", ack["error"])
        self.ack = ack.get("result")
        self._thread = threading.Thread(target=self._drain, args=(buf,), daemon=True)
        self._thread.start()

    def _drain(self, buf: bytearray) -> None:
        while True:
            try:
                msg, buf = _read_line(self._sock, buf)
            except (EOFError, OSError, ValueError):
                with self._cv:
                    self._closed = True
                    self._cv.notify_all()
                return
            with self._cv:
                self.events.append(msg)
                self._cv.notify_all()

    def wait_for(self, predicate: Callable[[dict], bool], timeout: float = 10.0) -> dict | None:
        """Block until an event satisfies `predicate`, including ones already seen."""
        with self._cv:
            seen = 0
            while True:
                for event in self.events[seen:]:
                    if predicate(event):
                        return event
                seen = len(self.events)
                if self._closed or not self._cv.wait(timeout):
                    return None

    def snapshot(self) -> list[dict]:
        with self._cv:
            return list(self.events)

    def close(self) -> None:
        try:
            self._sock.close()
        except OSError:
            pass

    def __enter__(self) -> EventStream:
        return self

    def __exit__(self, *_exc) -> None:
        self.close()


def _read_line(sock: socket.socket, buf: bytearray) -> tuple[dict, bytearray]:
    while b"\n" not in buf:
        chunk = sock.recv(65536)
        if not chunk:
            raise EOFError("herdr closed the control socket")
        buf.extend(chunk)
    idx = buf.index(b"\n")
    line = bytes(buf[:idx])
    return json.loads(line), bytearray(buf[idx + 1 :])


def _assert_owned(socket_path: Path, owned_root: Path) -> None:
    """Refuse any socket the probe did not create.

    A whitelist, not a blacklist: the probe splits panes, sends input, and closes
    things, so a run that reached a daemon holding real work would damage it. The only
    sockets it may touch are ones under a root it made itself.
    """
    resolved = Path(socket_path).resolve()
    root = Path(owned_root).resolve()
    if not resolved.is_relative_to(root):
        raise ProtectedSocketError(
            f"refusing to connect: {resolved} is outside the probe root {root}.\n"
            f"  Impact: the probe sends mutating requests (split, close, input), so a daemon "
            f"it does not own would have its live sessions damaged.\n"
            f"  Check: was --root pointed at an existing herdr config dir, or did "
            f"HERDR_SOCKET_PATH leak in from the surrounding shell?"
        )
