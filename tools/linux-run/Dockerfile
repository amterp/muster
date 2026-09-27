# A Linux machine for the daemon's own suites to run on (./dev --linux). The binaries are static,
# so the image holds only what a test starts: the three shells whose integration the daemon
# injects, infocmp to read the terminfo it carries, ps, and jq for the Claude Code statusline
# in extras/.
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash zsh fish jq ncurses-bin procps \
    && rm -rf /var/lib/apt/lists/*
