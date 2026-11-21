#!/bin/sh
set -eu

work=$1
plugin=$2
export TMPDIR="$work"
rule='────────────────────────────────────────────────────────────────────────────────'
printf '\033[2J\033[H%s\n>\n%s\n? for shortcuts\n' "$rule" "$rule"
printf 'AGY-READY\n'

while read -r action sequence conversation command; do
    case "$action" in
        hook)
            export ANTIGRAVITY_CONVERSATION_ID="$conversation"
            (cd "$plugin" && python3 -c 'import os, sys; os.setpgrp(); os.execl("/bin/sh", "sh", "-c", sys.argv[1])' "$command") < "$work/input" > "$work/hook-output"
            ;;
        status)
            result=0
            "$plugin/statusline.sh" 'cat; exit 7' < "$work/input" > "$work/drawn" || result=$?
            printf '%d\n' "$result" > "$work/draw-exit"
            ;;
        status-sample)
            python3 "$plugin/report.py" status < "$work/input"
            ;;
    esac
    printf 'AGY-DONE %s\n' "$sequence"
done
