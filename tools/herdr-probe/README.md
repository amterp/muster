# herdr-probe

Watches a real herdr daemon and records what it does, so `docs/architecture.md` can
rest on observed behavior instead of a careful reading of herdr's source.

```
./probe                     # every scenario
./probe frames input-path   # a subset
./probe --list
```

Transcripts land in `corpus/herdr-<version>/<scenario>/`. Findings read off them live
in `docs/observations/herdr-<version>.md`.

## Re-run it on every herdr upgrade

That is the point of keeping it. herdr's API is explicitly unstable and ships weekly;
a drifted understanding of it is Muster's top false-green risk (`docs/testing.md`).
Bump the pinned version, run `./probe`, and diff the new corpus against the old: what
changed in the diff is what changed in herdr. This is the seed of the contract tier,
not a finished version of it.

## It cannot touch your sessions

Every run spawns its own daemon under a scratch `XDG_CONFIG_HOME` and refuses to open
any socket outside that root - a whitelist, checked before the first request. The
scenarios split panes, send input, and close things, so nothing less would be safe to
run on a machine with real agents on it.

The daemon is pinned to a non-login `/bin/sh` with update checks off, so the corpus
records herdr's behavior rather than the developer's dotfiles.

## Why Python

The probe reads base64 frame payloads, scans raw ANSI for specific byte sequences, and
speaks a Unix-socket line protocol with a subscription that streams. Rad has no bytes
type and no socket client, so it would end up shelling out for every one of those.
Nothing else in the repo depends on this; it is a dev tool, not a component.

## Layout

| File | What |
|---|---|
| `probe` | entrypoint |
| `herdrprobe/daemon.py` | the isolated daemon and its lifecycle |
| `herdrprobe/client.py` | control socket: requests and subscriptions |
| `herdrprobe/panestream.py` | the pane data plane (`terminal session control`) |
| `herdrprobe/scenarios.py` | the scenarios |
| `herdrprobe/recorder.py` | corpus writer |
| `fake-agent/probe-agent` | fake agent that reports its state through the API |
| `fake-agent/screen-agent` | fake agent that herdr detects by reading its screen |
| `fake-agent/claude.toml` | detection manifest the screen agent is matched against |
