# devenv

The Linux machine Muster's remote half talks to: a container running sshd and nothing
else, reachable at `ssh -p $(./devenv/devenv port) dev@localhost`, or `./devenv/devenv ssh`.

```
./devenv/devenv build           build the image, and nothing else
./devenv/devenv up              build and start it
./devenv/devenv status          is it up, and is there a daemon in it
./devenv/devenv ssh             shell in as dev
./devenv/devenv down            stop and remove it
./devenv/devenv rebuild         rebuild from scratch
./devenv/devenv port            the port this checkout's container listens on
```

`up` builds every time rather than only when the image is missing. Docker's layer cache
makes that about a second, and the alternative was worse: an edited Dockerfile did nothing
until somebody thought to say `rebuild`.

The first `up` generates a keypair into `devenv/.ssh/`, which is gitignored. Nothing
in the image is a secret, and the port is published on loopback only, so nothing outside
this machine can reach it.

**Each checkout has its own container, image, port and key.** The container and image are
`muster-devenv-<checkout>`, where `<checkout>` is the first eight hex digits of the SHA-256 of
the checkout's path - the same name `./dev --contract` gives its directory - and the port is
22000 plus that number modulo 1000. So two worktrees run `./dev --ssh` at once, neither
recreates the other's container, and a Dockerfile edit in one reaches no other. The port is
derived rather than left to docker so that it survives a recreate: anything pointed at it
stays pointed at it.

The key is baked into the image, so a container started before `devenv/.ssh/` was regenerated
refuses the new one. `up` checks for that and recreates the container, and `status` reports it
rather than probing over ssh.

`./dev --doctor` lists every checkout's running container, including one whose worktree has
since been deleted, which nothing else would stop.

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
