#!/bin/sh
set -eu

if [ "${1-}" = report ]; then
    printf '%s\n' "$*" > "$AGY_TEST_WORK/last-report"
    if [ -f "$AGY_TEST_WORK/hold-report" ]; then
        mv "$AGY_TEST_WORK/hold-report" "$AGY_TEST_WORK/held-report"
        exec sleep 10
    fi
    exec "$AGY_TEST_DAEMON" "$@"
fi

work=$(pwd -P)
plugin=$(cat "$work/plugin")
export TMPDIR="$work"
export AGY_TEST_WORK="$work" AGY_TEST_DAEMON="${AGY_TEST_DAEMON:-$MUSTER_DAEMON}"
export MUSTER_DAEMON="$0"

run_hook() {
    export ANTIGRAVITY_CONVERSATION_ID="$1"
    (cd "$plugin" && python3 -c 'import os, sys; os.setpgrp(); os.execl("/bin/sh", "sh", "-c", sys.argv[1])' "$2") < "$work/input" > "$work/hook-output"
}

if [ "${1-}" = nested-hook ]; then
    run_hook "$2" "$3"
    exit
fi

printf '%s\n' "$@" > "$work/argv"
rule='────────────────────────────────────────────────────────────────────────────────'
frame=idle
draw() {
    printf '\033[2J\033[H'
    case "$frame" in
        working) printf '⠋ Thinking\nesc to cancel\n' ;;
        dialog) printf 'Requesting permission for: printf sentinel\nDo you want to proceed?\n' ;;
    esac
    draft=
    if [ "$frame" = draft ]; then draft='unsent draft'; fi
    printf '%s\n> %s\n%s\n? for shortcuts\n' "$rule" "$draft" "$rule"
}

draw
printf 'AGY-READY\n'

while read -r action sequence conversation command; do
    case "$action" in
        hook) run_hook "$conversation" "$command" ;;
        nested)
            python3 -c 'import os, sys; os.setpgrp(); os.execv(sys.argv[1], [sys.argv[1], "nested-hook", *sys.argv[2:]])' "$0" "$conversation" "$command"
            ;;
        old-hook)
            (export MUSTER_DAEMON_SOCKET="$(cat "$work/old-socket")"; run_hook "$conversation" "$command")
            ;;
        status)
            result=0
            "$plugin/statusline.sh" 'cat; exit 7' < "$work/input" > "$work/drawn" || result=$?
            printf '%d\n' "$result" > "$work/draw-exit"
            ;;
        status-sample)
            python3 "$plugin/report.py" status < "$work/input"
            ;;
        frame) frame=$conversation ;;
        '[muster]')
            printf '%s\t%s %s %s %s\n' "$frame" "$action" "$sequence" "$conversation" "$command" >> "$work/wakes"
            draw
            continue
            ;;
        quit) exit ;;
    esac
    draw
    printf 'AGY-DONE %s\n' "$sequence"
done
