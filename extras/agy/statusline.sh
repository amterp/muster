#!/bin/sh

# Command substitution would otherwise strip the original JSON's final newlines.
input=$(cat; printf x)
input=${input%x}

if [ -n "$MUSTER_DAEMON" ]; then
    printf '%s' "$input" | python3 "$(dirname "$0")/report.py" status --background >/dev/null 2>&1
fi

if [ "$#" -gt 0 ]; then
    printf '%s' "$input" | /bin/sh -c "$1"
fi
