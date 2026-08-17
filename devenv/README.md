# devenv

The Linux machine Muster's remote half talks to: a container running sshd and nothing
else, reachable at `ssh -p 2222 dev@localhost`.

```
./devenv/devenv build           build the image, and nothing else
./devenv/devenv up              build and start it
./devenv/devenv status          is it up, and is there a daemon in it
./devenv/devenv ssh             shell in as dev
./devenv/devenv install-daemon  put a herdr binary in it, for the corpus probe
./devenv/devenv down            stop and remove it
./devenv/devenv rebuild         rebuild from scratch
```

`up` builds every time rather than only when the image is missing. Docker's layer cache
makes that about a second, and the alternative was worse: an edited Dockerfile did nothing
until somebody thought to say `rebuild`.

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

## No daemon is installed here

That absence is the fixture. Muster puts its own herdr on a machine it attaches to, so a
container that arrived with one would exercise the adopt path and never the install path -
and a person setting up a real devenv installs nothing either.

Two things put a daemon in, and neither is the image. Muster does it on attach, under
`~/.muster/herdr/<version>/herdr`, having downloaded it on the machine running Muster and
copied it over the ssh master. And `./dev --ssh` does it with `install-daemon` afterwards,
for the corpus probe, which starts a herdr of its own rather than going through Muster.
Both take the version and the checksum from `deps/herdr.pin`, so re-pinning is one file
rather than three - the Dockerfile used to keep its own copy of the checksums by hand.

Update checks are off in `devenv/config.toml` and in the file Muster writes, so a container
that has been up for a week behaves like one started this morning.

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
