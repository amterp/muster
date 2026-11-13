# Codex 0.167.2

Measured 2026-11-13 on macOS arm64 with Codex 0.167.2, using isolated homes and a local Responses
stub. No model or user account was contacted by these probes. Live-tier runs used the real login
and `gpt-6.1-terra` at low effort, without changing the user's configuration or trust state.

## 1. Plugin overrides need an inline table

`corpus/codex-0.167.2/plugin-overrides.txt` records an installed local `muster-codex` plugin whose
SessionStart hook writes a marker. The user configuration enables it. A project hook also records
its events, and the local provider finishes one short turn.

With no override, the plugin writes its marker. The dotted override
`-c 'plugins."muster-codex@muster".enabled=false'` still lets the plugin run on this version.
The inline-table override `-c 'plugins={"muster-codex@muster"={enabled=false}}'` suppresses the
plugin while the project's hooks continue to run. Neither command changes the stored setting.

The live tier therefore disables the installed Muster plugin with the inline table for each
pane whose project hooks it selects. Its hookless pane reads state only from the screen, while
its hooked pane runs this checkout's project hooks. It keeps the user's HOME for the login.

The baseline live tier passed eight checks and failed the hookless comparison because the
installed plugin reported that pane's working and idle states. The idle-message round trip that
failed on 0.166.0 passed on 0.167.2 before any code change here.
