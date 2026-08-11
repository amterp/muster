# The remote half of Muster's workflow, and the fixture the remote path is tested
# against. One artifact serves both, so the environment developed against and the one
# CI asserts on cannot drift.
#
# Build context is the repo root, because the fake agents come from
# tools/herdr-probe/fake-agent - the same scripts the local probe drives.

FROM debian:bookworm-slim

ARG TARGETARCH
ARG HERDR_VERSION=0.8.0
ARG HERDR_SHA256_arm64=f647ac66468d9efbc642fe534fb284468f0aea60641606fc008dfc0d82a3ca87
ARG HERDR_SHA256_amd64=b872ea7e40fa2cb17e857ac9b62b1bf26db7b403c622f5d2f3f5b35f6e9acd28
ARG DEV_PUBKEY

# ForceIPv4 because deb.debian.org resolves to an IPv6 address that Docker's default
# bridge has no route to, and apt then hangs for minutes instead of failing. Costs
# nothing when IPv6 works.
RUN apt-get -o Acquire::ForceIPv4=true update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        openssh-server ca-certificates curl procps git less sudo \
    && rm -rf /var/lib/apt/lists/*

# Pinned by digest, not by tag: a fixture that silently upgrades its daemon is a
# fixture that silently changes what the tests mean.
RUN set -eux; \
    case "${TARGETARCH}" in \
        arm64) asset=herdr-linux-aarch64; sha="${HERDR_SHA256_arm64}" ;; \
        amd64) asset=herdr-linux-x86_64;  sha="${HERDR_SHA256_amd64}" ;; \
        *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/herdr \
        "https://github.com/herdrdev/herdr/releases/download/v${HERDR_VERSION}/${asset}"; \
    echo "${sha}  /usr/local/bin/herdr" | sha256sum -c -; \
    chmod 0755 /usr/local/bin/herdr

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
