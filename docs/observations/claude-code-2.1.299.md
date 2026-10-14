# Claude Code 2.1.299

How Claude Code 2.1.299 draws a finished Bash call, which detection had been reading as the
spinner of a turn still at work, and the screens it shows before a session starts.

Measured 2026-10-13 on macOS 26.4.1 / arm64, with Claude Code 2.1.299 in 100x30
pseudo-terminals driven by `tools/detection-capture.py`. The transcripts are
`corpus/claude-code-2.1.299/*.txt`. The screens are the "claude 2.1.299" cases in
`corpus/conformance/agent-detection-recorded.json`; the startup screens were recorded with a
scratch `CLAUDE_CONFIG_DIR`, no credentials, and an API key that is not one aimed at a port
nothing listens at, so none reached an account.

## 1. A finished Bash call is one row that looks like a spinner

Once a Bash call finishes, Claude Code draws it as one row, `· $ <command>`, and a command wider
than the pane is cut off with an ellipsis: `· $ echo the quick brown fox ... the lazy do…`. The
spinner at the start of a turn, before it has a timer, is the same glyph and the same ellipsis
around a word: `· Brewing…`. Later spinner lines carry a timer, `✻ Thinking… (3s · ...)`, and the
finished call's row has none (`bash-row.txt`).

## 2. Up to five screens come before the first prompt

A fresh install asks for a theme ("Choose the text style that looks best with your terminal"),
asks whether to use an API key it finds in its environment, shows "Security notes: ... Press
Enter to continue…", asks to trust the folder, and, started with `--dangerously-skip-permissions`,
asks to have bypass mode's risks accepted. With no API key it asks for a login method instead of
the key. The API key question, the trust dialog and the bypass warning end in "Enter to confirm ·
Esc to cancel"; the theme picker, the security notes and the login picker have no footer. None
sets a window title (`startup-dialogs.txt`).

An installed Claude Code in a folder it has not seen opens on the trust dialog alone, with
"No, exit" selected above "Yes, I trust this folder". The dialog was whole 0.5 s after `claude`
started and unchanged at 8 s (`startup-dialogs.txt`).

`/login` typed at the prompt opens the login picker's words and options under a "Login" title, a
top border of `▔` and an "Esc to cancel" footer. None of the three screens without a footer draws
a row of `─`; a session draws one as the top of its prompt box (`startup-dialogs.txt`).

## What this decides for Muster

`claude.toml`'s `live_turn_working` reads a spinner line without a timer only when the text
before the ellipsis is words, as in `· Brewing…`. It had accepted any text there, so a turn that
ended within twelve lines of a long Bash call stayed `working`: the Stop hook's idle report went
unconfirmed and expired, and the doorbell, which waits for an agent to stop working, held every
message to that agent until something redrew its screen. On 2026-10-13 that held two agents'
messages for about forty minutes.

`claude.toml` reads the theme picker, the security notes and the login picker as blocked, by
their words, since no footer or prompt box is there to read (section 2). The other three
startup screens already read blocked by `live_blocked_form`. Those words read blocked only with
no row of `─` among them, so a session quoting them keeps its state, and `/login` opened from the
prompt is a menu someone opened, which leaves the state as it was.

So when `muster pane wait --until idle,blocked` answered `idle` at the trust dialog, the rules had
not misread it. The detector publishes a new agent before its screen counts, three seconds after
it starts, and had published it `idle`; it now publishes it `unknown` until then.
