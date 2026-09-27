# devenv

The Linux machine Muster's remote half talks to: a container running sshd and nothing
else, reachable at `ssh -p 2222 dev@localhost`.

```
./devenv/devenv build           build the image, and nothing else
./devenv/devenv up              build and start it
./devenv/devenv status          is it up, and is there a daemon in it
./devenv/devenv ssh             shell in as dev
./devenv/devenv down            stop and remove it
./devenv/devenv rebuild         rebuild from scratch
```

`up` builds every time rather than only when the image is missing. Docker's layer cache
makes that about a second, and the alternative was worse: an edited Dockerfile did nothing
until somebody thought to say `rebuild`.

The first `up` generates a keypair into `devenv/.ssh/`, which is gitignored. Nothing
in the image is a secret and nothing outside localhost can reach it.

That keypair is per worktree, and the container is one per machine. So `up` checks that a
running container lets this worktree's key in, and recreates it when it does not - taking
whatever was running in it, and locking out the worktree that started it until that one runs
`up` again. `status` reports the mismatch rather than probing over ssh. Two worktrees still
cannot use the tier at once; whether each gets its own container or all share one key is
open (kan a_2Ky2ptlug).

## One artifact, two jobs

It stands in for the work devenv during development, and it is the fixture `./dev --ssh`
tests the remote path against. Keeping those the same container is the point: the
environment that gets developed against and the one the tests assert on cannot drift apart
if there is only one of them.

## No daemon is installed here

That absence is the fixture. Muster puts its own daemon on a machine it attaches to, so a
container that arrived with one would exercise the adopt path and never the install path -
and a person setting up a real devenv installs nothing either.

Muster puts one in on attach, under `~/.muster/daemon/<version>/`, copying the Linux build
the app carries over the ssh master, and `./dev --ssh`'s remote tests go through that same
install. The image itself never does.
