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
RUN apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        openssh-server ca-certificates curl procps git less sudo \
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
