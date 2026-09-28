# Claude Code 2.1.283

What Claude Code does with a message that reaches a session's inbox socket from a process that is
not one of that session's children, which is what `muster-daemon` is to every Claude session it
wakes (MIP-4, section 6).

Measured 2026-09-28 on macOS 26.4.1 / arm64, with Claude Code 2.1.283 logged in to a Claude Max
account and running Haiku 4.5, by
`crates/muster-daemon/tests/daemon/claude_code_inbox.rs`
(`MUSTER_RECORD_CLAUDE_INBOX=1 ./dev --claude-code`). Each session runs in a pane of a daemon the
test started, with `MUSTER_PANE` removed from its environment. The sender is the test process
itself, which is neither the session nor any child of it. The transcripts are
`corpus/claude-code-2.1.283/inbox-*.txt`, summarized in `inbox.json`, and every claim below reads
off one of them. Raised by MIP-4's "What is not yet verified" and kan `a_2Y3OZzZl4`.

Without `MUSTER_RECORD_CLAUDE_INBOX`, the same test holds the installed Claude Code to the newest
`inbox.json`, so `./dev --claude-code` fails when an update changes any outcome below.

## 1. A session that prompts for permissions delivers it; one that bypasses them holds it

| the session runs in | the sender sends | outcome | transcript |
|---|---|---|---|
| default mode | the message line only | delivered | `inbox-default-none.txt` |
| default mode | an auth line with the session's token, then the message | delivered | `inbox-default-token.txt` |
| `bypassPermissions` | the message line only | **held** | `inbox-bypass-none.txt` |
| `bypassPermissions` | an auth line with the session's token, then the message | **held** | `inbox-bypass-token.txt` |
| `bypassPermissions`, `crossSessionInbound: "accept"` passed with `--settings` | the message line only | delivered | `inbox-bypass-none-accept.txt` |

This is Claude Code's documented default for a sender that states no permission mode, and the
held dialog says so: "The sender did not attest its permission mode, and this session bypasses
permission prompts." Claude Code names the sender by process id ("from an unidentified session
[verified pid 28166]", the test process), so it did look at who connected.

**The token does not verify a sender that is not the session's child.** The documentation leaves
this open: it says the token verifies a child on macOS once the child has exited, and says nothing
of a process that was never a child. Presenting the token changed nothing in either mode.

A delivered message starts a turn at once in an idle session, headed "Another Claude session sent
a message:" and followed by Claude Code's instruction to treat it as a teammate's request. A held
one shows a notice, "Held peer message", and a dialog offering "Deny" first and "Deliver this
message to Claude" second.

## 2. The socket says nothing back

In every case the connection returned no bytes within three seconds of the message line. Nothing
on the socket distinguishes delivered from held, so a sender cannot report which happened.

## 3. A connection that sends nothing shows nothing

Each transcript opens with the screen three seconds after the test connected and closed without
sending a line. None shows any trace of it, so connecting and closing is a liveness probe the
session does not see.

## What this decides for Muster

The inbox adapter stays the default, and sends no auth line. It wakes every Claude session that
prompts for permissions, which includes `auto` mode, with nothing configured. A session started
with `--dangerously-skip-permissions` or `--permission-mode bypassPermissions` is woken only if it
was started with `--settings '{"crossSessionInbound":"accept"}'`. Setting that in the session's
own flags keeps it off every other session, where user settings would apply it to all of them.
The hooks adapter (MIP-4, stage 4) is the way to wake a bypass session with nothing set, because
its hook is the session's own child.

Since the token verifies nothing here, the daemon neither sends nor keeps it: a participant's
inbox is its socket path alone.

## Not run

The four Linux cases, default and bypass, with and without the token. Neither Linux environment
this repository can reach has a Claude Code that can run: the devenv container (`./dev --ssh`)
and the test container (`./dev --linux`) have no Claude Code installed, and no credentials to run
one with - this machine's login lives in the macOS keychain, and no `ANTHROPIC_API_KEY` is set.
The documentation says Linux verifies a child by process evidence even after it exits, which
concerns children only, so nothing here predicts the Linux answer for the daemon.

## Traps when re-running

- A held dialog quotes "Another Claude session sent a message", the line a delivered message
  opens with. The test asks about held first for that reason.
- User settings are not loaded (`--setting-sources project`), so a `crossSessionInbound` in them
  cannot leak in; the only settings are what `--settings` passes, which include
  `skipDangerousModePermissionPrompt` so a bypass session starts without its consent dialog.
- Each case gets its own session. A second message to a session with a dialog open lands behind
  the dialog and would be classified by what the first one showed.
