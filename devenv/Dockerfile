# The remote half of Muster's workflow, and the fixture the remote path is tested
# against. One artifact serves both, so the environment developed against and the one
# CI asserts on cannot drift.
#
# Build context is the repo root, because the fake agents come from
# tools/herdr-probe/fake-agent - the same scripts the local probe drives.
#
# **No daemon is installed here, and that absence is the fixture.** Muster puts its own
# herdr on a machine it attaches to, so a devenv that arrived with one would test the
# adopt path and never the install path - and a person setting up a real devenv installs
# nothing either. `./dev --ssh` runs the remote tests against exactly what is built here,
# and only afterwards drops a pinned daemon in for the corpus probe, which starts a herdr
# of its own rather than going through Muster.

FROM debian:bookworm-slim

ARG DEV_PUBKEY

# ForceIPv4 because deb.debian.org resolves to an IPv6 address that Docker's default
# bridge has no route to, and apt then hangs for minutes instead of failing. Costs
# nothing when IPv6 works.
#
# python3 is for the corpus probe. Its sending-text scenario puts a program in a pane that
# says exactly which bytes reached it, and the pane is in here - so the program has to run
# in here too, and it is the same Python the local probe runs on a Mac.
RUN apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        openssh-server ca-certificates curl procps git less sudo python3 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash dev \
    && mkdir -p /var/run/sshd /home/dev/.ssh \
    && chmod 0700 /home/dev/.ssh \
    && chown -R dev:dev /home/dev/.ssh

# Key-only login. The container listens on localhost and holds nothing precious, but
# a password-authenticating sshd in a repo is a habit worth not forming.
RUN printf '%s\n' \
        'PasswordAuthentication no' \
        'PermitRootLogin no' \
        'AllowUsers dev' \
        'AcceptEnv HERDR_* MUSTER_*' \
    > /etc/ssh/sshd_config.d/devenv.conf

RUN test -n "${DEV_PUBKEY}" \
    && echo "${DEV_PUBKEY}" > /home/dev/.ssh/authorized_keys \
    && chmod 0600 /home/dev/.ssh/authorized_keys \
    && chown dev:dev /home/dev/.ssh/authorized_keys

# Root for exactly one thing: pointing /bin/sh at bash and back. Muster runs its remote scripts
# through `sh`, and whether a backgrounded list forks depends on which shell that is - dash
# here, bash on a RHEL-family box. crates/muster-herdr/tests/devenv_forking_shell.rs swaps it
# for the length of one test (kan a_2I5yedXos). Scoped to the two commands rather than
# NOPASSWD: ALL, for the same reason sshd refuses passwords above.
RUN printf '%s\n' \
        'dev ALL=(root) NOPASSWD: /usr/bin/ln -sf bash /bin/sh, /usr/bin/ln -sf dash /bin/sh' \
    > /etc/sudoers.d/devenv-sh \
    && chmod 0440 /etc/sudoers.d/devenv-sh \
    && visudo -cf /etc/sudoers.d/devenv-sh

# The same fake agents the local probe uses. screen-agent installs as `claude`
# because herdr identifies an agent by its foreground process name, and only names it
# already knows can carry an override manifest.
COPY tools/herdr-probe/fake-agent/probe-agent  /usr/local/bin/probe-agent
COPY tools/herdr-probe/fake-agent/screen-agent /usr/local/bin/claude
COPY tools/herdr-probe/fake-agent/claude.toml  /home/dev/.config/herdr/agent-detection/claude.toml
COPY devenv/config.toml                        /home/dev/.config/herdr/config.toml
COPY devenv/entrypoint                         /usr/local/bin/devenv-entrypoint

RUN chmod 0755 /usr/local/bin/probe-agent /usr/local/bin/claude /usr/local/bin/devenv-entrypoint \
    && chown -R dev:dev /home/dev/.config

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/devenv-entrypoint"]
