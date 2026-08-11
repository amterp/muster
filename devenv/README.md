# devenv

The Linux machine Muster's remote half talks to: a container running sshd and a herdr
daemon, reachable at `ssh -p 2222 dev@localhost`.

```
./devenv/devenv up       build if needed, then start it
./devenv/devenv status   is it up, and does its daemon answer
./devenv/devenv ssh      shell in as dev
./devenv/devenv down     stop and remove it
./devenv/devenv rebuild  rebuild from scratch
```

The first `up` generates a keypair into `devenv/.ssh/`, which is gitignored. Nothing
in the image is a secret and nothing outside localhost can reach it.

## One artifact, two jobs

It stands in for the work devenv during development, and it is the fixture the remote
path is tested against - locally and in CI. Keeping those the same container is the
point: the environment that gets developed against and the one CI asserts on cannot
drift apart if there is only one of them.

`tools/herdr-probe/probe --remote` runs the same scenarios here that it runs against a
local daemon, recording into `corpus/herdr-<version>-linux/`. Diffing that against the
macOS corpus is how "local and remote render identically" stops being an aspiration.

## What is pinned, and why

The herdr binary is downloaded by version and verified against the sha256 published in
`herdr.dev/latest.json`, rather than built from source - a 222k-line Rust build in the
image would make a rebuild an event rather than a habit. Update checks and manifest
fetches are off, so a container that has been up for a week behaves like one started
this morning, and the suite runs offline.

A fixture whose daemon silently upgrades is a fixture whose test results silently
change meaning. When Muster moves to a newer herdr, bump `HERDR_VERSION` and the two
`HERDR_SHA256_*` args together, rebuild, and re-record the corpus.

## The fake agents

Copied from `tools/herdr-probe/fake-agent/`, so local and remote fixtures are the same
scripts rather than two that drift:

- `probe-agent` reports its lifecycle through herdr's API. State is driven, so a test
  asserts immediately.
- `claude` (installed from `screen-agent`) says nothing and paints marker lines that
  the bundled override manifest matches. It exercises the path a real coding agent
  takes. It installs under the name `claude` because herdr identifies an agent from the
  pane's foreground process name, and only names it already knows can carry an override
  manifest.

Both take `working`, `blocked`, `idle`, and `quit` on stdin.
