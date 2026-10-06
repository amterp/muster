# Claude Code 2.1.290

What Claude Code's dialogs do with keys and with a paste, which is how `muster pane send --key`
answers one, and what it does with `/clear` written into its prompt box several ways.

Measured 2026-10-05 on macOS 26.4.1 / arm64, with Claude Code 2.1.290 started as
`claude --model haiku --permission-mode default` in a fresh folder, first in a pseudo-terminal
driven by a scratch script built on `tools/detection-capture.py`, then in a pane of a
muster-daemon built from the commit that added `--key`, driven by its `muster` CLI with no window
answering. The transcripts are `corpus/claude-code-2.1.290/dialog-keys.txt` and `clear.txt`.

## 1. The folder-trust dialog moves on arrow keys, and ignores a paste and a digit

A fresh folder opens on "Accessing workspace", with "No, exit" selected above "Yes, I trust this
folder". A down arrow written alone selects "Yes", an up arrow selects "No" again, and Return
takes the selected option. The same down arrow inside a bracketed paste only redraws the dialog,
and so does a `1`: its options carry no numbers, and a digit picks nothing. A down arrow and a
Return in one write were both taken. From the last option, down selects the first: the list
wraps (`dialog-keys.txt`).

Through muster-daemon, `--key down --key down --enter` from "Yes" went to "No", back to "Yes",
and trusted the folder, each key pressed on its own 50 ms after the last.

## 2. `/clear` runs as a command at the prompt, however it is written

Written at the idle prompt as one paste with its Return, as a paste with the Return two seconds
later, as six keys and a Return, and as a paste with its Return while a turn was running a
command, `/clear` ran as the command every time: the session's log records it under
`<command-name>`, and no case sent it to the model as a message (`clear.txt`). A pasted `/clear`
opens the slash-command menu under the box with `/clear` first, and a Return later runs it.

## What this decides for Muster

`muster pane send --key` presses keys rather than pasting them, one write each with a gap, which
is what section 1 needs; the text of a send stays a paste. A send of keys alone is confirmed by
the pane changing, which the trust dialog does on every key it takes and not on one it ignores.

Section 2 is why a slash command needs nothing special. A `/clear` that reached a worker as
chat, seen once in September 2026 through muster-daemon with no window (kan a_2ZNnTCJ4q), was
not reproduced by any way of writing it here. The window and a CLI with no window put the same
text, keys and Return in the same daemon message, so they cannot be the difference.
