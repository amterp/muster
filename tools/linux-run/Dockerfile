# A Linux machine for the daemon's own suites to run on (./dev --linux). The binaries are static,
# so the image holds only what a test starts: the three shells whose integration the daemon
# injects, infocmp to read the terminfo it carries, ps, jq for the Claude Code statusline
# in extras/, Node.js for OpenCode's plugin, and Python for Antigravity's.
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash zsh fish jq nodejs ncurses-bin procps python3 \
    && rm -rf /var/lib/apt/lists/*
