#!/bin/sh
# A Claude Code statusline command that tells Muster how full this session's context is,
# which model it is running and what it has cost, then draws your statusline.
#
#   "statusLine": { "type": "command",
#                   "command": "/path/to/statusline.sh ~/.claude/your-statusline.sh" }
#
# A command after it gets Claude Code's JSON on stdin, as it would have, and draws the line.
# With none, this draws the model and how full the context is.
#
# Outside a Muster pane it only draws: $MUSTER_DAEMON is how a pane reaches the daemon that owns
# it. Needs jq. The report runs in the background, so a slow daemon never holds the line up.

# The x keeps the trailing newline, which $( ) would strip: the command after this reads what
# Claude Code wrote, byte for byte.
input=$(cat; printf x)
input=${input%x}

report() {
    eval "set -- $1"
    "$MUSTER_DAEMON" report "$@"
}

if [ -n "$MUSTER_DAEMON" ] && command -v jq >/dev/null 2>&1; then
    facts=$(printf '%s' "$input" | jq -r '[
        (.context_window.used_percentage // empty | "--context-used", tostring),
        (.model.display_name // empty | "--model", .),
        (.cost.total_cost_usd // empty | "--cost-usd", tostring)
    ] | @sh' 2>/dev/null)
    # Claude Code reads the line until every writer has closed it, so the background job's own
    # output goes nowhere. Redirecting only the call inside it would leave the job holding the
    # line open until the report finished.
    { [ -n "$facts" ] && report "$facts"; } >/dev/null 2>&1 &
fi

if [ $# -gt 0 ]; then
    printf '%s' "$input" | "$@"
else
    printf '%s' "$input" | jq -r \
        '"\(.model.display_name // "")  \(.context_window.used_percentage // 0 | floor)% context"' \
        2>/dev/null
fi
