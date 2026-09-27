# The remote half of Muster's workflow, and the fixture the remote path is tested
# against. One artifact serves both, so the environment developed against and the one
# the tests assert on cannot drift.
#
# Build context is the repo root, for the entrypoint.
#
# **No daemon is installed here, and that absence is the fixture.** Muster puts its own
# muster-daemon on a machine it attaches to, so a devenv that arrived with one would test
# the adopt path and never the install path - and a person setting up a real devenv installs
# nothing either. `./dev --ssh` runs the remote tests against exactly what is built here.

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
        'AcceptEnv MUSTER_*' \
        'AcceptEnv COLORTERM TERM_PROGRAM TERM_PROGRAM_VERSION' \
    > /etc/ssh/sshd_config.d/devenv.conf

RUN test -n "${DEV_PUBKEY}" \
    && echo "${DEV_PUBKEY}" > /home/dev/.ssh/authorized_keys \
    && chmod 0600 /home/dev/.ssh/authorized_keys \
    && chown dev:dev /home/dev/.ssh/authorized_keys

# Root for exactly one thing: infocmp as root keeping the caller's TERMINFO, which is what
# Ghostty's sudo wrapper asks for. crates/muster-daemon/tests/daemon/devenv_terminfo.rs runs it
# through the wrapper. Scoped to the one command rather than NOPASSWD: ALL, for the same reason
# sshd refuses passwords above. sshd's second AcceptEnv line is for the names Ghostty's ssh
# wrapper forwards.
RUN printf '%s\n' \
        'dev ALL=(root) NOPASSWD:SETENV: /usr/bin/infocmp' \
    > /etc/sudoers.d/devenv-terminfo \
    && chmod 0440 /etc/sudoers.d/devenv-terminfo \
    && visudo -cf /etc/sudoers.d/devenv-terminfo

COPY devenv/entrypoint /usr/local/bin/devenv-entrypoint
RUN chmod 0755 /usr/local/bin/devenv-entrypoint

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/devenv-entrypoint"]
