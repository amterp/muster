---
mip: 4
title: Agents messaging each other, and groups convened around a policy
status: Draft
kind: Architecture
created: 2026-09-26
decided:
supersedes:
superseded-by:
related: 1, 3
---

# MIP-4: Agents messaging each other, and groups convened around a policy

## Summary

Agents send each other messages through a service inside `muster-daemon`. An agent posts with
`muster msg post`; the daemon appends the message to a log, works out which participants it
should wake, and wakes each one through whatever that agent can be woken by: Claude Code's
inbox socket, the agent's own hooks, or, as a last resort, one short line typed into its pane.
The woken agent fetches the text with `muster msg read`. No agent waits inside a blocking call,
which is what stranded the agents of council v1 (`amterp/council`, the tool this replaces), and
no message body travels through a keyboard, so a message of any length arrives intact.

The model has five concepts: participant, message, group, delivery and adapter. A convened
council is a group whose policy is data: which members an unaddressed post wakes (its ring set),
who may address whom, and who may add or remove members. The daemon enforces that policy and one
guard, that a post is refused while its author has unread messages in that group. Everything
about how a council should behave, including who directs it and when to stop talking, lives in
skills.

Across machines, each group has one home daemon, which numbers and stores its log; other daemons
forward posts to it over the ssh socket forwarding Muster already runs. Presence is the agent
state the daemon already detects. For messages, `muster msg post` replaces `muster pane send`,
which stays as the way to type into a pane.

The messaging code is a crate with no knowledge of panes, so an agent in a plain terminal, never
in a pane, joins the same way a pane agent does, and nothing in Muster's core depends on it.

## Decisions for amterp

These four decisions are the owner's; argument alone does not settle them. Each says whether it
is one-way, meaning hard to reverse once things are built on it. Everything else below is
proposed as decided.

**1. How the human reads and writes messages.**

- (a) A transcript in an ordinary pane (`muster msg log --follow`), posting with `muster msg
  post`, and a notification through attention routing for every message that wakes the human.
- (b) (a), plus a groups section in the sidebar with an unread count per group and a badge for
  messages addressed to the human.
- (c) A web view, as council v1 had.

Recommendation: (a) first, and (b) once (a) has been used for a round. v1's human spoke through
agents' terminals rather than through its web view. They also got lost in the volume of messages,
which argues for notifying only on messages meant for the human. Not one-way: each option reads
the same events.

**2. Whether private messages exist in v2.**

- (a) No. Addressing decides who is woken, never who may read. Every message is in its group's
  log, readable by any participant and by the human.
- (b) Yes: a message with addressees is readable only by them and the human.

Recommendation: (a). In the roughly twenty real v1 sessions nobody sent or asked for a private
message; the cost the human paid was noise, which is a waking problem, not a reading one. (a) is
reversible, since private messages can be added later; (b) is not, since removing them breaks
agents that rely on them.

**3. The verb namespace, and what runs the verbs on Linux.**

- (a) `muster msg <verb>` on both platforms, with the `muster` CLI built for Linux and installed
  to `~/.muster/bin/muster` by the install that places the daemon. `muster-cli` is already a pure
  client (clap, prost and generated types, no libghostty), so a musl build is small.
- (b) Top-level verbs: `muster post`, `muster read`, `muster who`.
- (c) Subcommands of the daemon binary, `muster-daemon msg <verb>`, following the `report`
  subcommand an agent already uses to report facts about itself (MIP-3 section 2,
  `crates/muster-daemon/src/report.rs`).
- (d) A separate binary.

Recommendation: (a). An agent learns one command name on every machine, and `msg` keeps `read` from
colliding with `pane read`. A Linux `muster` is also one of the two pieces kan `a_29e9be7aQ` needs;
the other is forwarding the window's socket. If (a) is taken, `report`, a subcommand of the daemon
binary today, would move beside these for the same reason. Close to one-way: skills, hook
configurations and agents' habits will spell the verbs, and renaming them means a transition period.

**Decided: (a)**, by amterp on 2026-09-28.

**4. Who holds the link between two machines' daemons.**

- (a) The app. The ssh master it already opens for each `[[daemon]]` forwards the far daemon's
  socket; the app tells the local daemon that path, and the local daemon dials it.
  Cross-machine messaging stops while no window is running.
- (b) The daemon, with ssh masters of its own, so a laptop agent and a devenv agent keep talking
  with the app closed.

Recommendation: (a) for v2. It reuses supervision that already works against the work devenv.
(b) has open questions nobody has measured: whether a daemon started through Launch Services can
reach the user's ssh agent, and what happens when ssh needs an interactive login. Not one-way: the
link is a socket path either way, and the daemon-to-daemon protocol is the same.

## Context / Motivation

### What `muster pane send` cannot do

`muster pane send` types into a PTY, which is right for a keystroke and wrong for a message. A
Return that arrives mid-paste submits the fragment so far. Claude Code folds a long paste into a
`[Pasted text #N]` placeholder that waits for a person's Return (measured at 1583 bytes,
`docs/cli/limits.md`). A pane showing a dialog answers its first option to any Return. Exit 0
means the daemon took the text, not that the program received it. A devenv pane cannot send at
all (`a_29e9be7aQ`).

### What council v1 got wrong in use

Council v1 (Go, last commit 2026-01-04) had the group semantics: a shared transcript, turn order,
a moderator view, and an optimistic lock that refused a post unless its author had seen the latest
event, which this MIP calls the stale-context guard. Its transport was a JSONL file per session,
polled every 2 s by an agent blocked inside a Bash tool call with a timeout. About twenty of the
621 session directories in `~/.council/sessions/` are real use. They, the repository's board and
its commits record what went wrong; each row below is a failure, its cause, and the part of this
design that answers it.

| Observed in v1 | Cause | Answered by |
|---|---|---|
| Agents stuck waiting when others left | Waiting was a poll on a name being designated next; a designated agent that had stopped looping stranded everyone | Push wakes (sections 5, 6); no turn order (Rationale) |
| The last agent waiting forever on "Moderator" | `await` never released on the human | The human is a participant with a wake (section 10) |
| A post to a departed agent released every waiter at once | The fix for stranding was "release everyone" | Wakes go only to addressees or the ring set, coalesced (section 5) |
| Agents unreachable once they started working | Delivery needed the receiver blocked in `await` | Wakes reach a working agent between tool calls (section 6) |
| Leave cascades within 60 s of the first "signing off", in almost every session; one agent left on the turn it was told not to | Staying meant sitting in a blocking loop; the skill primed leaving; prompting failed | Staying costs nothing; policy can refuse leaving (section 8) |
| Joined agents asleep, with no way to tell ("Hello", "Test", no reply) | Membership was not presence | Presence from agent state (section 7) |
| The human could not keep up and typed "wait" | No pause, and nothing separated messages for the human from chatter | Notifications only for messages that wake the human; `pause` (sections 8, 10) |
| The human spoke through agents' terminals | The web view was one more window | The transcript is a pane in the window the human is already in (section 10) |
| Multi-page pasted reviews | The web compose box took them; a PTY would not | Bodies never travel through a PTY (section 4) |
| Hand-counted `--after N` on every post | Stateless CLI | A read cursor per participant (section 4) |
| Turns spent only on passing the turn: "Standing by", "Passing to Moderator" | A turn was the only way to pass | An agent ends its turn and is woken when addressed (section 5) |
| An "orchestrated mode" that existed only as a sentence in the skill | Nothing enforced it | Policy as data, enforced (section 8) |
| 482 test sessions in the real `~/.council` | No injectable store root | Tests use the test's scratch directory (section 15) |

What worked and is kept: one append-only log that agents and the human read alike, which let late
joiners catch up; the guard; text framed for a model, with a start and end marker per message;
explicit addressing; the human as a light participant whose few posts steered whole sessions; a
skill carried in `--help`, and errors that name the next command. Every real participant ran in a
plain terminal, never in a Muster pane, so an agent outside a pane must join as easily as one
inside. v1's code is not carried.

### What MIP-3 changed

Kan `a_2Rtd0Ed0l` assumed the message service would be Muster's first daemon, beside herdr. MIP-3
replaces herdr with `muster-daemon`, one per machine per install, speaking protobuf over a Unix
socket, copied from the app bundle onto a devenv, and running inside `MusterSessions.app` on the
Mac. Agent detection lives in it, and so do the facts an agent reports about itself (MIP-3 section
2), with the daemon's executable and socket named in every pane's environment as `MUSTER_DAEMON` and
`MUSTER_DAEMON_SOCKET`. MIP-3 section 11 already leaves room: requests are namespaced by service,
and a message service is "one more crate, depending on nothing about panes, hosted by
`muster-daemon` and never depended on by `muster-core`".

## Decision

### 1. The service lives in `muster-daemon`, as a crate that knows nothing of panes

The message service is a crate, `muster-msg`, hosted by `muster-daemon` and reached on the
daemon's socket under the `msg.*` namespace. It holds participants, groups, policy, logs, cursors
and delivery, and names no pane, tab or window. Everything machine- or harness-shaped reaches it
through four traits it defines: wake, presence, store, and peer link. `muster-daemon` implements
them over its panes, its detection and its disk; the crate's own tests implement them in memory.

`muster-daemon` depends on `muster-msg`, and nothing depends the other way. `muster-core` depends
on neither, and sees messaging only as `msg.*` events that the seam's daemon client turns into
attention events and view state. A process that is not in a pane, and is not Muster, joins over
the same socket: the CLI finds it from `MUSTER_DAEMON_SOCKET` when set, and otherwise at
`~/.muster/daemon/<install>.sock`, with the install name compiled in (MIP-3 section 1).

If no daemon is running, any `msg` verb starts one the way the app does: through Launch Services
on the Mac, with `setsid` on Linux. A daemon that starts restores its persisted tabs with a shell
in each pane, so a `msg` verb can bring back the panes of a machine whose daemon had stopped, as
shells. That is the daemon's existing restart behavior, stated here because messaging can now
trigger it.

### 2. The model

**Participant.** A name, a home daemon (the one it joined on), and the wake addresses the daemon
learned from it. When its agent exits it is gone, not deleted, and its name and cursors wait for
it to come back.

**Message.** An author, a group, optional addressees, a UTF-8 body of at most 1 MiB (well under the
16 MiB a daemon message may be), and a sequence number assigned by the group's home daemon.

**Group.** A named set of participants with a policy and one log. Every message belongs to
exactly one group. A post names its group with `--group`. Without one, it goes to the group its
author and addressees share, if exactly one; if they share none, to the group of exactly them,
created on first use with the default policy; if they share several, or the post has no
addressees and its author is in more than one group, it is refused and asks for `--group`. So two
agents can message each other without convening anything, members of a council messaging each
other stay under the council's policy, and there is one kind of log.

**Delivery.** From each new message the daemon computes who to wake, coalesces, and hands each
wake to an adapter. A read cursor per participant per group makes `read` mean "what I have not
read", and lets `post` refuse while its author has unread messages.

**Adapter.** How to wake a participant, how to tell whether it is alive and busy, where logs are
stored, and how to reach another daemon: the four traits of section 1. The word is used as
`docs/glossary.md` uses it for the backend, one module per outside thing, translating.

Nothing else is a concept. The daemon knows no director, moderator or council: a director is a
participant that a group's policy points at, and a council is a group convened with a policy from
a skill.

### 3. Joining, and who a caller is

`muster msg join --name critic --group review` registers the caller as participant `critic` and
adds it to `review`, creating the group if it does not exist. The caller hands over every wake
address in its own environment: Claude Code's `CLAUDE_CODE_MESSAGING_SOCKET` and `MUSTER_PANE`;
`--pull` says its hooks fetch messages (section 6). Not `CLAUDE_CODE_MESSAGING_TOKEN`: it
verifies nothing when the daemon sends it (`docs/observations/claude-code-2.1.283.md`), so the
daemon never holds the secret. The verb runs inside the agent's session and inherits the
addresses, so the daemon never has to map panes to processes. Any other verb from a caller that
has not joined registers it the same way, under a default name.

**The default name is the last part of the caller's working directory**, with `-2`, `-3` added
while a live participant holds it, and a gone holder's name taken over as `join` would. Parallel
agents already sit in distinct worktrees (`muster-1` to `muster-5`), so that is the name a person
would use for each, and it never refuses. Claude's session name is not in a Bash call's
environment, and requiring `--name` first would make an agent's first `read` fail. From stage 2 a
pane's label takes precedence, since a person chose it.

No caller may become a participant that is alive in another session, whether it asks with
`join --name` or with `--as`: its addresses would replace the live one's, which would never be woken
again. A caller carrying no address, such as a script, moves nothing, so it may act as anyone.

Every verb identifies its caller by the first of: `--as NAME`; a session address a wake adapter
recognizes, such as Claude's socket path, which is the same in every Bash call, whereas a variable
exported in one Bash call is gone in the next; or `MUSTER_PANE`, when detection has found an agent
in that pane. A caller carrying none of these is the human (section 10): a person's own shell, in
a pane or not, has no agent identity. Every verb also refreshes the caller's wake addresses, so a
participant known only by its pane gains Claude's socket the first time it runs any verb from
inside Claude.

Names are not authentication. The daemon trusts every process of the user it runs as, the same
boundary Claude Code draws around its inbox socket and Muster around `MUSTER_SOCKET`.

**Coming back.** A resumed agent has new addresses. Joining with the name of a participant that is
gone takes it over, cursors included. Joining with the name of one that is alive is refused, and
the refusal says where that participant is.

**A pane that never joined** can be addressed by its pane name; the daemon creates the participant
on first address, named after the pane, with the pane as its wake address. As built, any open
pane can be, not only one where an agent has been found: `pane new --run claude` and the post
that follows it come a moment apart, before detection has found the agent, and the ring waits
for one anyway. A pane identifies the caller running a command in it only once an agent is found
there, since a person's shell in a pane is the human. A pane's label does not name the
participant: labels hold emoji and spaces that names cannot, and the integrator already has the
pane name in hand.
That is what lets `msg post` replace `pane send` for an agent started by `muster pane new --run
claude`, which has joined nothing (section 14).

### 4. Posting, reading, and the stale-context guard

```
muster msg post --group review "The parser change is in; rebase before touching lexer.rs."
muster msg post --group review --to builder --file findings.md
muster msg read
```

`post` answers with the message's number and whom it woke:

```
posted #42 to review
woke: builder (working), director (idle)
not woken: scout (gone)
```

A post that wakes no live participant is still appended, and exits 6, so an agent does not end
its turn expecting an answer nobody will send. Live means woken, already woken, to be rung once
its pane allows (`rung once idle: critic (working)`), or the human. An agent in a pane is not
live when its pane has no agent and has been open for over 30 seconds (`no agent in its pane`),
or when its agent's prompt cannot be read, so the doorbell never rings it (`its prompt cannot be
read`, section 6). The human always counts as
live, since messages to the human wait for them. v1's last agent waiting forever on the human
becomes something the agent can see.

`read` prints every unread message across the caller's groups, or one group's with `--group`, in
v1's framing, and moves the cursor past them:

```
--- review #41 | director -> builder ---
Take the lexer; leave the parser to critic.
--- end review #41 | director ---
```

Joins, leaves and policy changes appear in `read` as one-line notices and do not count as unread.
v1's lock counted them, so every join made every pending post stale.

**The guard.** `post` to a group is refused while its author has unread messages there, and the
refusal says how many and to run `muster msg read --group review`. The post carries the author's
cursor, and the home daemon compares it with the log's head as it appends, so the check holds
across machines without the agent counting anything. There is no override: an agent that meant to
post anyway reads first, which costs one call.

The human is exempt. The guard keeps a model from acting on context that has gone stale, and the
daemon cannot see a person's screen: the human reads the transcript with `log --follow`, which
moves no cursor, so under the guard a busy group would refuse nearly every post the human made
and ask them to reprint what they had just read. Agents keep it.

`muster msg log --group review [--since N] [--follow]` prints the transcript and moves no cursor.
The human reads it, and so does an agent catching up on history from before it joined.

### 5. Delivery: who is woken, and how often

A message wakes its addressees if it has any, and otherwise the ring set its group's policy gives
for its author (section 8). It never wakes its author or a participant that is gone. While a group
is paused, a post wakes only the human, and only if it would have woken the human anyway.

**One wake per batch, per group.** A participant is woken when its count of unread messages in a
group that would wake it goes from zero to more than zero, and not again for that group until it
reads. Messages that would not wake it, such as a directed council's chatter among others, are
unread but never trigger a wake. If its agent goes idle with waking messages still unread, it is
woken once more, and then not until it reads. That second wake comes on the first transition to
idle after the first wake, never on a timer: a timer would fire into a turn that is busy acting on
the first, and idle is the moment an agent can act on a wake at all. Its text says "still unread",
so Claude Code's filter for identical repeats keeps it. It needs presence, so it lands in stage 2. Ten messages arriving while an agent works cost it
one wake. Every wake an agent acts on is a turn it pays for, so this rule and the ring set are
what bound a group's cost.

**The wake text is a notice, not the message:**

```
[muster] review: 3 new (#40-42), 1 to you, from director, critic. Read: muster msg read --group review
```

It carries the range because Claude Code drops identical repeats arriving within a short window
(its cross-session messaging documentation, "Limitations"), so a second wake with the same text
would vanish. It carries no body because only the agent's own `read` moves the cursor, and the
inbox and doorbell adapters cannot tell whether their text reached the model.

**An urgent post is the exception to one wake per batch.** Added after stage 5: `post --urgent`
wakes its addressees even when they were woken for the group and have not read since, because
once per batch is the rule for what can wait until a turn ends, and an urgent post is one that
cannot. Each urgent post wakes; the doorbell keeps one ring per participant and group waiting, so
a burst before the ring lands costs one ring. Urgency is kept on the message in the log, so it
crosses machines with it and survives a restart. The wake counts it and asks for a read now, since
it lands in the middle of a task:

```
[muster] review: 2 new (#41-42), 1 urgent, 1 to you, from director. Read it now, before you go on: muster msg read --group review
```

### 6. Wake adapters

Three adapters, tried in this order; the first whose address is present and alive delivers. None
of them is Muster assuming a harness: each is one thing Muster supports, in the sense of kan
`a_2AJS0Xz7I`, and the doorbell works for any harness in a pane whose manifest can read its
empty prompt, which as built is Claude Code's alone.

As built in stage 2, an agent in a pane is rung by the doorbell even when it has Claude's inbox
too, so the order is hooks, doorbell, inbox. A session that bypasses permission prompts holds an
inbox message for a person's approval (`docs/observations/claude-code-2.1.283.md`), the daemon
cannot tell which sessions do, and agents in panes are usually started that way. The inbox
remains the adapter for a session outside any pane.

**Hooks (`--pull`).** For a harness whose hooks can run a command and hand its output to the
model. For Claude Code the configuration is two hooks, shipped as a snippet in `extras/`, not as
Muster code:

- `PostToolUse` runs `muster msg read --if-unread` and returns its output as
  `additionalContext`, so messages arrive between tool calls during a turn.
- `Stop` runs `muster msg wait >&2; exit 2` as an `asyncRewake` hook. `wait` blocks until the
  caller has waking messages unread, then prints the notice and exits 0, like every other verb
  that succeeded; the hook moves it to stderr and exits 2, which the hooks documentation says
  wakes the session with that text as a system reminder. A script waiting on a message reads
  stdout and an exit code of 0, and should not have to know Claude's hook convention.

Because the hook runs `read` itself, this is the one adapter that delivers bodies, and the cursor
moves because the text did reach the model. A hook is the session's own child, so Claude Code does
not hold its output for approval, as it may hold an inbox message ("What is not yet verified").
It costs setup the other two do not.

As built in stage 4, the snippet is `extras/claude-code/messaging-hooks.json`, left out of the
plugin's own hooks so that only a session given it fetches its messages this way. It differs from
the design above in three places:

- **`PostToolUse` writes to stderr and exits 2** rather than returning `additionalContext`. Claude
  Code 2.1.283 hands the model either (`crates/muster-daemon/tests/daemon/claude_code_hooks.rs`),
  and JSON would mean escaping a message body from a shell one-liner, which has no JSON encoder.
- **`Stop` runs `muster msg wait --due`**, not plain `wait`. Plain `wait` answers whenever a waking
  message is unread, so an agent that ended a turn with one unread would be woken at the end of
  every turn after. `--due` answers only a wake section 5 would deliver: once per batch, once more
  "still unread", then nothing until the agent reads. Answering marks the agent woken, as a ring
  would.
- **The hook sets `timeout`** to a day. Claude Code ends a hook at its `timeout`, or at a default
  when it has none, and an `asyncRewake` hook is no exception.

`--due`, like `join --pull`, marks the participant as fetching with hooks. Its hooks count as
live while a `wait` of its own is connected, or while it is working and any verb of its own ran
in the last five minutes; a participant with no pane, whose activity nobody sees, is taken at its
last verb. A post to a participant whose hooks are live answers its waiting `wait`, or, when none
waits because a turn is running, marks it woken and sends nothing: the turn's next `PostToolUse`
delivers the message, or its `Stop` hook's `wait --due` returns "still unread". Such a
participant is never rung. A ring queued for it when its hooks come alive goes to its `wait`
instead, or is dropped when none is connected. A participant whose hooks are not live falls back
to the doorbell and the inbox: the hook was ended or never installed, or its turn ended without a
`Stop` hook, in an API error or at Esc, which leaves it idle with no `wait`. Seen idle so, it is
rung "still unread" for what its hooks were counted on to fetch, after two seconds in which a
`Stop` hook that starts late can connect and be told instead. When a verb last ran is kept in memory only, so after a
daemon restart a participant's hooks count as live again once its `wait` reconnects, which the
CLI does by itself, or once one of its verbs runs.

**Claude Code's inbox socket.** When a wake is due the daemon connects to the participant's
`CLAUDE_CODE_MESSAGING_SOCKET`, sends one `{"type":"user","message":...}` line carrying the
notice, and closes. It connects only once the text is ready, because Claude Code closes a
connection that has sent no line within 30 seconds. The socket accepting a connection is also the
participant's liveness, and a connection that sends nothing shows nothing in the session. It works
for a Claude session in a plain terminal, and needs nothing configured unless the session bypasses
permission prompts: Claude Code 2.1.283 holds the daemon's message for approval in such a session,
whether or not it carries the session's token, and delivers it once the session was started with
`--settings '{"crossSessionInbound":"accept"}'` (`docs/observations/claude-code-2.1.283.md`).
Nothing comes back on the socket either way, so the post's answer says the wake was handed over,
not that it was read.

**A one-line doorbell into the pane.** For an agent in a pane, as built, whatever else it has. The
daemon queues the notice on the pane's writer as a paste followed by Return, the path `pane send`
uses. It is one line, well under a canonical-mode line limit and under the length at which Claude
Code folds a paste. The daemon never rings a pane whose agent is blocked, since a Return would
answer the dialog, and waits until no keystroke has arrived from a window for that pane for a few
seconds, so a person's half-typed prompt is not submitted with the notice in it. Cyclops, a
prior-art tool that rings agents in tmux panes, guards its doorbell the same way. Stage 2's
review found those two guards were not enough, and the doorbell as built rings only a prompt it
has read as empty.

As built, the doorbell types into a pane only what it has just read to be its agent's empty
prompt. The daemon's detection finds which manifest rule decides a pane's screen, and a rule may
carry a `prompt` pattern, which says that screen is the agent's prompt and marks where its text
starts. Immediately before each write the doorbell checks, in order:

- the agent is idle or waiting, and nothing has been typed or sent into the pane for three
  seconds, since a keystroke may not have reached the screen yet;
- the agent has drawn nothing for half a second, since a harness that has only just drawn its
  prompt may not read input yet as it will: Codex 0.154 takes a paste the moment its composer
  first appears as typed keys (`docs/observations/codex-0.154.0.md`). An urgent ring at work
  skips this, since a working agent animates, and so does an idle agent whose screen has kept
  moving for five seconds since the doorbell first found it otherwise ready - an animated
  statusline, a clock - which would otherwise never be rung;
- the agent is still the pane's foreground program, not the shell it exited to, whose screen
  still shows the agent's last frame;
- the agent's manifest has a prompt rule;
- that rule decides the screen as it is now, and nothing follows its marker but text drawn faint.

Claude Code 2.1.283 draws a suggestion nobody typed, `Try "..."`, faint in an empty prompt, and
draws what somebody typed at normal weight. So the faint cells are left out, and what remains
is what the prompt holds. A dialog, a menu opened from the prompt, working, and a screen no rule
recognizes are each decided by a rule with no `prompt`, so none of them is rung. Claude Code's
manifest has a prompt rule, and since MIP-5 Codex's does, read in a region of its own; an agent
of any other harness in a pane is never rung. The post's answer says so.

What cannot be rung at once waits in the daemon, and one thread rings it when the pane allows.
That thread wakes when a post arrives and when an agent's state changes, and otherwise sleeps
only until the next deadline: the end of a quiet period, a ring due its Return again, or five
seconds while a prompt holds a draft or a new pane has no agent yet, since nothing announces
either changing. It also delivers the second wake of section 5. A daemon that starts, or takes
the panes over by a handoff, or resumes after a handoff failed, rings again every wake it finds
recorded and unread, since it cannot tell which were rung: at worst a wake too many.

A Claude Code that is starting can draw its empty prompt before it reads its terminal the way
that prompt does. What is typed then fills its prompt and the Return is dropped, so
the ring sits unsent. Once its prompt is up, a Return in the same write as the text is sent with
it: a Return that arrives while a paste is being taken is held until the paste is in, then
pressed. So a ring counts as taken once the agent goes to work, reads what it was rung for, or
shows an empty prompt again. Until then the doorbell presses Return again every five seconds, at
most six times, and only while every check above holds except the last, which becomes: nobody
has typed into the pane since the ring, and the prompt holds the ring's own text and nothing
else. Typing ends the ring even when it was taken back, because the daemon knows what it wrote
into the pane but the screen can lag it: an agent slow to paint, or the daemon's copy of the
screen behind under load, shows the ring alone over words a Return would send. A prompt holding
anything else, or a screen that is no longer the prompt, also ends the ring, and the wake is
forgotten so that the next post rings afresh.

**What the doorbell guarantees.** It types one line, its own wake, and only into a prompt it
read as empty just before the write. It repeats nothing but Return, and only while the prompt
shows that line unsent and nothing has been typed into the pane since, so a repeated Return can
send nothing but the wake. It never rings a
blocked agent, a working one but for an urgent wake (below), a menu or dialog detection
recognizes, a prompt holding a draft, a pane typed into in the last three seconds, an idle agent
that drew something in the last half second, the shell an agent exited to, or an agent whose
manifest has no prompt rule.

**What it does not guarantee.** The check and the write are close but not simultaneous: a dialog
drawn, or a key pressed, between them gets the wake and its Return. A screen that a prompt rule
wrongly decides as the prompt is rung as one, so the guarantee is only as good as the manifest,
and a harness update that draws a new dialog above an unchanged prompt box needs a rule for it.
Text a harness draws faint reads as not typed, so a harness that drew a person's draft faint
would have it read as an empty prompt. And "woke" says the wake was typed, not that the agent
read it.

**An urgent wake may ring an agent at work.** Claude Code keeps its prompt box on screen while it
works; a line typed there with Return is queued, and handed to the model once the tool call it is
in returns, as a reminder to address it before going on
(`docs/observations/claude-code-2.1.288.md`). So for a wake whose messages include an urgent one,
the first check above also lets the agent be working, and the last also takes the prompt box of an
agent at work. Every other check is unchanged, and a blocked agent waits until it is out of its
dialog, which the post's answer says.

Detection's engine 6 reads that prompt. A rule whose state is `working` may carry `prompt`, and
any prompt rule may name a `prompt_region` to read it in. The prompt of an agent at work is read
only when two rules agree: the rule deciding the screen with its title is a working one, and the
rule deciding the screen alone, the title left out, is a working rule with a prompt. The second
look is what keeps a dialog out: Claude's title spinner outranks every screen rule, its Bash
permission dialog included. Claude's `live_turn_working` reads the prompt in its box alone,
because a working screen draws the request that started the turn above the box with the same
caret. The idle prompt's reading is unchanged, so an ordinary ring is held to exactly what it
was.

A ring typed at work is taken once the agent's prompt box is empty again, which is how Claude
Code shows a line it queued; going to work says nothing, since the agent was already working.

An agent at work can stop to ask a person at any moment, which an idle one does not, and the
daemon's copy of the screen shows a new dialog a moment after the agent draws it. A Return at
Claude Code's permission dialog takes its highlighted option, "Yes", while the ring's line
itself, pasted or typed, does nothing there (`docs/observations/claude-code-2.1.288.md`, section
3). So a ring at work is typed without its Return. A second look, a second later, presses Return
only if the prompt box holds the ring and nothing else; a dialog over it keeps the Return back
until the box shows again, however long it stays open, since nothing else would send the ring
sitting in the box. What remains unguarded is a dialog drawn
in the milliseconds between that second look and the Return reaching the agent: the Return would
answer it. Anything else that ends a ring at work - a person typing, a screen the doorbell cannot
read - leaves the agent counted as woken, so it is rung "still unread" as it goes idle.
Until then Return is pressed again as for any ring. The hooks adapter and the inbox already reach
an agent mid-turn, so urgency changes nothing for a participant they serve. Whether the model
stops to read is its own call: in the recording Sonnet read at once, and Haiku 4.5 finished its
task first.

### 7. Presence

A participant is working, blocked, idle, done, alive, or gone:

- **In a pane on its home daemon**: the pane's agent state from detection. Gone when the pane
  closes or detection sees the agent exit. As built, `waiting` is an idle agent that declared it is
  waiting on work of its own, and an agent in a pane is there while the pane has an agent, whatever
  its inbox says.

  For the doorbell a pane is in one of four states. It **rings** when an agent is there and its
  manifest has a prompt rule. It has **no prompt** when an agent is there without one: the post
  says `its prompt cannot be read`, and the agent counts as not live. An **agent is to come**
  while the pane has no agent and was opened under 30 seconds ago, so that `muster pane new --run
  claude` followed at once by a post defers the ring (`rung once an agent is found`) rather than
  losing it. Otherwise it has **no agent**: an older pane whose agent never came or has exited,
  or one that has closed (`no agent in its pane`).
- **Elsewhere, with Claude's socket**: alive while the socket accepts a connection, with no finer
  state.
- **With hooks only**: alive while a `wait` is connected or a hook has run in the last few minutes
  (five, as built).

Each daemon computes presence for its own participants, so `muster msg who` and a post's answer
show every member of a group, whichever machine it is on. As built, a daemon asks the other
machine's for its members when an answer needs them rather than being sent changes: `who` asks,
and a post's answer carries what each machine's daemon did for its own members when it took the
entry. Nothing is replicated that could go stale.

### 8. Groups and policy

A group's policy is five fields. `ring` and `allow` are keyed by author. Every group gets the
permissive default unless its creator passes a policy:

```toml
ring = { "*" = ["*", "@human"] }  # an unaddressed post wakes every member but its author
allow = { "*" = ["*"] }           # anyone may address anyone
membership = ["*"]        # anyone may join, and any member may leave
urgent = ["*"]            # anyone may post urgently
paused = false
```

A council convened around a director might be:

```toml
# extras/skill/council/directed.toml
ring = { director = ["*"], "*" = ["director"] }
allow = { director = ["*"], "@human" = ["*"], "*" = ["director", "@human"] }
membership = ["director", "@human"]
paused = false
```

Under that policy the director's unaddressed post wakes every member but the human, a member's
wakes only the director, a member may address only the director or the human, the director and
the human may address anyone, and only those two may add or remove a member, including the member itself: `muster msg leave` is refused with a message naming
who may dismiss it. `@human` is the reserved name for the human (section 10), so a preset works
for whoever runs it.

`muster msg group new review --policy directed.toml` convenes a group, `muster msg group set`
changes a field, and `muster msg pause review` and `resume` set `paused`. The `membership` list
also decides who may change the policy, and every change is a notice in the log.

As built in stage 4:

- **A ring set's `*` is every member but the human.** Waking the human raises a notification,
  which interrupts a person, so a ring set wakes the human only by naming `@human` (section 10).
  The default names it, so a group nobody gave a policy rings the human as before; a directed
  council's director does not, though the human who convened it is a member.
- **`allow` is checked against a post's addressees**, once `--to` has resolved pane names to
  the participants in them. An addressee outside the author's list is refused with
  `not_allowed`, naming whom the author may address. An unaddressed post is `ring`'s business,
  which enforces nothing, only narrows who is woken.
- **`membership` decides every change to the group but its creation.** It governs joining a
  group that exists, leaving one (a `leave` with no group is refused whole if any of the
  caller's groups refuses it), `group add` and `group remove`, `group set`, `pause` and `resume`. A refusal
  is `not_permitted`, naming who may. Creating a group is open to anyone, and its creator is its
  first member.
- **`group set` replaces the policy** from a file, the same TOML `group new` reads. One
  path for every field is simpler to get right than a grammar per field, and a file of four keys
  is short. A key the file holds that no field reads is refused, so a typo is not silently a
  default. Whether the group is paused is left as it is, and a file saying `paused = true` is
  refused: a pause forgets the wakes it holds and a resume makes them, so `pause` and `resume`
  are the only way to change it, where a file leaving the key out would unpause the group with
  nobody woken for what it held.
- **Changes are entries in the log.** `group add` and `group remove` log a join and a leave
  under the member's name; `group set`, `pause` and `resume` log who changed what.
- **A paused group still reaches the human.** A post to a paused group is appended, the human is
  woken as ever, and the post's answer says each agent is held while the group is paused. It
  counts as heard, since a wake is coming. Pausing forgets which members were woken, so `resume`
  wakes each member with waking messages unread once, including one woken before the pause, and
  its answer lists them as a post's does.
- **`urgent` says who may post urgently**, added after stage 5. An urgent post interrupts an
  agent mid-turn, so a directed council can keep that to its director and the human. A post it
  does not allow is refused as `not_urgent`, naming who may, rather than sent as an ordinary
  post, which would arrive later than its author was told. A policy from a daemon older than
  the key says nothing about it, which is the default.
- **`group delete` takes the group and its log**, added after stage 5. `membership` decides
  it, as every other change, and only the group's home does it. Each member stays a
  participant, losing only its place in the group: its cursor, what it was woken for, and a
  wait kept to the group, which ends as a leave ends it. Unread messages go with the log;
  nothing unread refuses the delete, which is deliberate, and `log` reads a group first. No
  leave entries are written, since the log they would go in is going. What waits for the
  human there is told as nothing, a follow of the log ends as `no_such_group`, and the name is
  free again, from #1. The log file is removed before the saved state is written, so a crash
  between the two leaves a policy with no log, which loading drops, rather than a log whose
  policy fell back to the permissive default.
- **A delete reaches the replicas.** The home tells each machine with a member to forget the
  group, and that machine drops its replica as the home dropped the group. A machine out of
  reach then finds out when the link returns: its refetch is refused as `no_such_group`, and a
  replica whose home says that is forgotten. One case is not handled: a group deleted and made
  again under the same name while the link was down is taken for the old one, whose head is
  past the new log's, so the replica misses the new group's first entries. Telling the two
  apart needs something like a creation stamp on the group.

`allow` binds within its group. Two members who share no other group can still reach each other
by posting to a new group of their own; the policy exists to keep a convened group's rules in
force after a prompt fades, not to confine agents that set out to get around it.

`paused` exists because v1's human typed "wait" into a stream of messages arriving every 15
seconds and had only another message to ask with. While a group is paused its posts are appended
and wake no agent; on resume, each participant with waking messages unread gets one wake.

### 9. What the daemon enforces and what skills carry

The daemon enforces what must still hold on turn 40, after an instruction in a prompt has faded:
`allow`, `ring`, `paused`, `membership`, and the guard.

Kan `a_2Rtd0Ed0l` proposed enforcing only `allow` and `ring`, with the guard as part of delivery.
`paused` is argued in section 8. `membership` is added because v1 shows prompting failing on the
turn it was given: the human wrote "get to work. Don't need to leave though", and the agent
answered "Releasing you both" and left. With push wakes staying costs nothing, so a skill can say
stay, and a directed policy can refuse the leave.

Skills carry everything else: what a council is for, roles, when to post and when to stop,
terseness, relevance, passing the turn, what a director does, and the preset policies. That keeps
a feature shaped like a workflow from imposing one: the daemon knows participants, messages and
policy, and has no opinion about how agents use them. The skill that replaces
`council-participant` ships in `extras/skill/`.

### 10. The human

The human is a participant like any other, reached by the reserved name `@human` and displayed
under a setting that defaults to the login name. It is homed on the machine the app runs on and
registered by the app when it attaches to a daemon. Its wake adapter is attention routing: a
message that wakes the human, because it is addressed to the human or its ring set includes the
human, raises a notification naming its author, and choosing it lands on the transcript. Chatter
that does not wake the human raises nothing. v1's human asked agents to "concisely summarize what
exactly you need input on". Addressing the human, or a ring set that includes the human, is now
the only thing that interrupts.

The human posts with `muster msg post` from the transcript pane or any shell of their own, which
section 3 attributes to the human, including long text with `--file` or stdin. With the app
closed, messages to the human wait unread and notify at the next launch. Anything beyond that is
Decision 1.

As built in stage 3, under Decision 1 (a):

- **Registering is attending.** A window subscribes to the daemon on its own machine as
  attending, and never to one over ssh. While one attends, a message that wakes the human counts
  as waking them; otherwise the post says the human is notified when a window opens.
- **What waits is state.** A wake of the human is not coalesced, since a person reading the
  transcript moves no cursor and a batch would never end. Each one updates what waits for the
  human in its group - the unread messages that would wake them, how many were addressed to
  them, who wrote them - which the daemon sends every window as an event and carries in its
  snapshot. So a window that opens after the post is told, with nothing queued, and the next
  launch notifies.
- **One banner per group.** It names who wrote, is replaced by each new message, and asks
  after a blocked agent and before a program's notification. Several windows raise one between
  them: the one in front most recently, as for a tab nobody holds.
- **The transcript is a pane.** Choosing the banner, or ⌘⇧A, goes to the pane on the human's
  home daemon running `muster msg log --group G --follow`, found by that command, or opens a
  tab running it. Going there is the human reading the group: the window reads it for them, and
  the banner comes down once the daemon says nothing waits. A transcript somebody is reading in
  a focused window reads what arrives there, and raises nothing.
- **The human's home publishes.** For a group homed elsewhere, the home daemon's replica wakes
  the human the way a local post does (section 11), and a window takes what waits for the human
  only from a daemon on its own machine, so only the daemon on the app's machine ever tells a
  window.
- **A far daemon has no human.** The app's daemon is the one that dials a link, so a daemon that
  was dialed records the dialer's machine as the human's home, and keeps it across restarts. From
  then on, unless a window attends it, a person's shell there is that machine's human,
  `@human@laptop`, and an agent's `@human` means the same. The person may post in a group kept
  there that they are in, and change its policy, members or pause. Reading, waiting, joining,
  leaving and making a group need the human's cursors, so the service refuses them there as
  `human_elsewhere`, as it does a post in a group kept anywhere else or in a new pair group. The
  daemon carries each such request over the link to the human's home, as it does a change to a
  group kept there and a request naming a group it holds nothing of. The home does it as the
  human and answers in its own names, which the far machine reads as its own when typed back
  (section 11). Names in the request are written as the far machine knows them, and one it does
  not know as the home's. A carried wait lasts as long as its caller, and ends at the home when
  the caller hangs up. Only with no link up does the person see `human_elsewhere`, saying so. An
  agent there reaches the human as it reaches any member on another machine. A daemon no link
  has dialed yet keeps a human of its own, which no window hears of.

The display name of this section's first paragraph is not built: messages name the human
`@human`.

### 11. Across machines

**A group's home daemon is the one it was created on.** The home assigns every sequence number and
writes the log. With one writer the guard is a single comparison, the same property v1 had from
taking an exclusive lock on its one file for every write.

**Other daemons hold replicas.** A daemon with a local member of a remote group subscribes to that
group's log over a peer link, keeps a copy, and forwards its members' posts to the home. Each
daemon wakes only its own participants, because a wake address is local: a Claude socket on the
devenv can only be dialed on the devenv. Cursors live on the participant's home daemon. The home
builds a post's answer from the presence each daemon reports.

**The link.** Under Decision 4's recommendation, the app tells the local daemon, with a
`msg.peer` request, where each far daemon's socket is forwarded, and the local daemon dials it.
One connection carries traffic both ways, so a devenv member's post reaches a laptop-homed group
though ssh forwards in one direction only. MIP-3 already streams panes over this forwarding, so
this adds a connection, not a tunnel.

**When the link is down**, a post to a remote-homed group fails at once, naming the unreachable
machine. It is not queued: a queued post would land behind messages its author never saw. Groups
homed on this machine keep working, and `read` and `log` on a replica say it may be behind.

A group or participant name is unique on its home daemon, and a verb accepts `review@devenv` when
two attached machines use the same name.

As built, each machine writes its own members bare and another machine's as `name@machine`, in its
own name for that machine: the app's `[[daemon]] id` for the far one, and from the far side the
name the laptop's daemon chose when it first linked and keeps beside its store, since the laptop's
own id is usually `local` and its host name changes with the network (on a Mac, the Sharing
name). Whatever crosses the link is
turned into the receiver's names on arrival, so `who`, `read` and a wake read from where the reader
stands, and the laptop's human in a devenv group is `@human@laptop` there, woken by the laptop. A
bare name is the one kept here, or else the one elsewhere that goes by it, and `join` with a bare
name asks each linked machine before making the group here. Each answer from a home carries the
entries after the asker's head, so a replica that fell behind catches up in the reply, a refusal
by the guard included. Whatever a peer names is checked before it is turned: a call's group and
whoever acts in it are bare, so a peer cannot act as one of this machine's participants or land
entries on a group kept here, and every other name must be one a participant could have. A change
sent to the home and never answered is refused as `unanswered` rather than `unreachable`, since it
may have landed, and the replica says it may be behind until the home is next heard from. A name
written as another machine writes one of this machine's, `review@devenv` on the devenv, is this
machine's own.

A name in a post's `--to` or a `group add` that means nobody here is asked of each linked machine,
or of the one `name@machine` names, with a `whom` call: the machine answers the participant it
means, the one in the pane of that name, or the pane while an agent may start there. A post to
someone on another machine who shares no group with its author makes the pair group on the
author's machine, as on one machine, named from the members' own names without their machines; a
name a group holding anyone else already has takes `-2`, `-3`. A replica told of a join by a pane
or the human it has no participant for makes one, as `--to` does, and wakes it. One thing is not
built: a daemon reaches only the machines it is linked to directly, so an agent on one devenv
cannot reach an agent or group on a second devenv (the second hop).

**A group's policy binds at its home** (section 8). A forwarded post runs through the same checks
as one made there, whom its author may address and whether the group is paused among them, and a
forwarded join or leave is checked against `membership`. Every batch of entries the home sends
carries the policy, so a replica wakes its own members as the home would, and a pause or resume
there holds or wakes the replica's members too. The policy is the one at the batch's end, and the
log says that a policy was set but not which, so a replica reads each message under what the log
can say of the one it was posted under: a pause exactly, since each pause and resume is an entry,
and a ring set as the replica's previous policy until the batch sets a new one. Only the home
changes a policy, its members or its pause; those verbs on a replica are refused as
`kept_elsewhere`, naming the home. A policy may name a member on another machine as the home names
it, `director@devenv`, and a replica reads it in its own names, so a devenv agent can direct a
council kept on the laptop, or join a group whose `membership` names it.

**`@human` in a policy is the human on any machine.** There is one person, homed on the machine
the app runs on (section 10), so a policy's `@human`, like its `*`, names a role rather than a
participant: it is not turned into another machine's name, and it matches `@human@laptop` as it
matches `@human`. A group kept on the devenv with the default policy rings the laptop's human as
one kept on the laptop does, and a ring set's `*` passes over the human on every machine. The human
is woken by its home daemon, the only one that tells windows what waits for the human, and the
guard never holds the human's post on either machine.

### 12. Persistence

Each daemon stores the groups it is home to beside its persisted state (MIP-3 section 2): an
append-only log file per group, synced on each append, and a file of participants, wake addresses,
cursors and policies, written with an atomic rename as MIP-3's persisted state is. A group posts a
message every few seconds at most, so that is at most one sync every few seconds per group. Logs
survive a daemon restart, a daemon handoff (MIP-3 section 10) and a reboot. Wake addresses survive
too and are found dead at first use, which marks their participants gone until they return. Replicas
are not persisted; a replica refetches when its link returns. Until then a daemon that starts holds
an empty replica for each group a cursor names, with the participants holding those cursors as
members, so the group answers as there and behind rather than as unknown, and a bare name still
finds it.

**A log file is named after its group, so group names are unique regardless of case.** On macOS's
default filesystem `Review.log` and `review.log` are one file, so two groups named that way would
merge after a restart with their sequence numbers repeated. A group whose name differs from an
existing one's only in case is refused, naming the existing group. The other way was to encode case
into file names, which keeps both groups but makes the store unreadable by eye; nobody wants
`Review` and `review` as two groups anyway. For the same reason a group name is at most 250 bytes,
so its file name fits the 255 a filesystem allows, and a post whose pair group would be named past
that is refused asking for a named group. Participant names are not file names and keep their case.

A crash during an append leaves a last line with no newline, which was a post never answered.
Loading cuts it off, so the next append starts on a line of its own, and a cursor past its log's
head is brought back to it.

Logs are kept until the group is deleted (`muster msg group delete`). `muster msg groups` lists
the groups this machine is home to or replicates, with size and last activity. As built,
`groups` lists each group's members and whether it is paused, and `group delete` removes the
log (section 8).

**Message bodies never enter the daemon's log**, and so never the run's log that follows it (MIP-3
section 1). It records that message 42 of `review` was posted, its size and whom it woke, under the
rule that keeps what a person types out of it.

### 13. The verbs, and where they live

All under `muster msg` (Decision 3), each with `--json`:

| verb | does |
|---|---|
| `join [--name N] [--group G] [--pull]` | registers the caller; joins a group, creating it if absent |
| `leave [--group G]` | leaves a group, or with no group stops being a participant |
| `who [--group G]` | members and their presence |
| `post [--group G] [--to A,B] [TEXT \| --file F \| -]` | appends and wakes |
| `read [--group G] [--if-unread]` | prints unread messages and moves the cursor |
| `log --group G [--since N] [--follow]` | the transcript; moves nothing |
| `wait [--group G] [--timeout S] [--due]` | blocks until the caller has waking messages unread |
| `groups` | every group, its members, and whether it is paused |
| `group new G [--policy F]`, `group set G --policy F` | makes a group with a policy, or replaces its policy |
| `group add G NAME...`, `group remove G NAME...` | adds or removes members, by participant or pane name |
| `pause G`, `resume G` | holds a group's wakes, then wakes each member once |
| `group delete G` | deletes a group and its log, letting every member go |

`wait` is for a hook running in the background and for scripts. With `--due` it answers only a
wake section 5 would deliver, as the `Stop` hook needs (section 6); without it, it answers
whenever a waking message is unread, which a script expects. A new `wait` for a participant
ends the older one, so at most one runs. An agent that runs `wait` in the foreground has rebuilt
v1's `await`, and the skill says so.

The verbs send `msg.*` requests from `muster_daemon.proto`, so the CLI gains a connection to the
daemon beside its connection to the window. The app's view of messages and a post from the window
send the same requests, so GUI, CLI and agents share one path. That path runs through the daemon
rather than through the core, because messaging must work with no window open. This departs from
`CLAUDE.md`'s "every action runs through one shared path" as the core has meant it until now, and
keeps its purpose, parity by construction.

On Linux the verbs come from a musl build of `muster-cli`, bundled beside the Linux daemons and
installed to `~/.muster/bin/muster` by the same install that places the daemon. Window verbs there
exit 3, "no window to ask", until `a_29e9be7aQ` forwards a window's socket. `muster msg --help`
carries the protocol and `muster docs msg` the reference.

### 14. What happens to `muster pane send`

`pane send` stays as the keyboard: how a script answers a prompt or drives a full-screen program.
For telling an agent something it stops being the recommended path. `muster docs agents` and
`extras/skill/SKILL.md` point at `muster msg post --to <pane>`, and `docs/cli/limits.md` keeps its
account of the keyboard's limits under typing into a pane. The integrator flow becomes:

```
muster pane new --down --run claude --name "🤖 A"     # prints p1w3r07bsd
muster msg post --to p1w3r07bsd --file brief.md
```

The integrator's `post` registers it (section 3). The doorbell rings the new agent once its
prompt shows, empty. A Claude Code that opens on its trust dialog is not at its prompt, so the
ring waits for whoever answers the dialog; the doorbell never does. The agent then runs `muster
msg read`, and the brief arrives whole however long it is. Its answer is a message that wakes the integrator, rather than a `pane read` of its screen.
`./dev --claude-code` runs this flow against the Claude Code installed here, with a 20 KB brief
and the worker bypassing permission prompts. When its trust dialog shows, the test checks that
nothing is rung while it is up, then answers it as a person would.

### 15. Testing

Policy and delivery are functions of a log, a policy and presence, so they are conformance cases
in `corpus/conformance/messaging.json`: given members, presence, a policy and a post, the expected
append, refusal or wakes. Each row of the v1 table that policy or delivery answers is a named case.
`muster-msg` runs them with in-memory adapters and no daemon. The file's `v1` rows name the case
for each row of that table, or where a row is answered instead, and a test fails when they and
the table disagree.

The daemon tier runs the real daemon with its store in the test's scratch directory, so no test
writes to a real `~/.muster`. Claude Code's inbox socket is an outside behavior, so it gets an
observation, `docs/observations/claude-code-<version>.md`, recorded from a real Claude with raw
transcripts in `corpus/`, and the adapter is tested against a socket that behaves as that
recording shows. Cross-machine cases join the `--ssh` tier.

## Delivery

Each stage lands after MIP-3's cut-over, is sized for a round by two agents, and leaves the suite
green on its own.

1. **One machine, two agents, no panes.** `muster-msg` with the default policy; the `msg.*`
   requests; participants, groups, the log and its persistence, the cursor and the guard; `join`,
   `leave`, `who`, `post`, `read`, `log`, `wait`; the inbox-socket adapter; the Linux build of
   `muster-cli`. It opens with the Claude observation, because its answer decides whether a
   session that bypasses permission prompts needs `crossSessionInbound` set or the hooks adapter.
   Proves: two Claude sessions in plain terminals, on the Mac and on the devenv, exchange messages
   with nothing polling; the guard refuses a post on unread; a log survives a daemon restart.

   As built, the devenv half of the Claude proof is not run: no Linux machine this repository
   reaches has Claude Code with credentials. The `--ssh` tier proves the same exchange there
   between two callers of the installed `muster`, one of them woken from a blocked `wait`. Left
   for later: a `msg` verb starting a daemon when none runs, which needs the launch code out of
   `muster-daemon-client`, and a remote pane's `PATH` reaching `~/.muster/bin`. A daemon adopted
   rather than installed got the CLI only at its next install, until adopting one whose machine
   has no CLI installed it there.

2. **Presence, panes, and the end of `pane send` for messages.** Presence from detection; pane
   participants addressed by pane name; the doorbell and its guards; the post answer and its exit
   code for waking nobody; the docs and the muster skill switched. Proves: the integrator flow in
   section 14 end to end, a 20 KB brief reaching a freshly started Claude intact, and a blocked
   pane never rung.

3. **The human.** `@human`, attention routing for messages that wake the human, the notification
   landing on the transcript, `log --follow`, and whatever Decision 1 adds. Proves: a message to
   the human raises exactly one notification and lands on the transcript, and chatter that does
   not wake the human raises none.

   As built, under Decision 1 (a), with the human exempt from the guard (section 4); the proof
   is `crates/muster-seam/tests/seam/messages_for_the_human.rs`, against a real daemon. Left for
   later: the display name of section 10, and Decision 1 (b).

4. **Groups with policy, and the council skill.** `ring`, `allow`, `membership` and `paused`; the
   hooks adapter and its snippet; the skill replacing `council-participant`, with its presets.
   Proves: every v1 failure that policy or delivery answers as a passing case, and one real
   directed council of three agents and the human running past forty messages with no agent
   leaving or stranded.

   As built, the forty-message proof is `claude_code_council.rs` in `./dev --claude-code`: three
   Haiku sessions reached 56 messages in six minutes for about $1.59. Its first four runs failed
   on the skill, not on delivery, and the skill was fixed from them. Left for later: `group
   delete`, a session idle for more than an hour, and the hooks on Linux.

5. **Across machines.** `msg.peer`, peer links over the forwarded socket, replicas, presence over
   the link, forwarding to the home, and loud failure when the link is down, in the `--ssh` tier.
   Proves: a laptop agent and a devenv agent in a plain terminal share one group, the guard holds
   across the link, and local groups keep working while it is cut.

   As built, the `--ssh` tier's proof is `crates/muster-seam/tests/devenv_messages.rs`, with a
   window attached to both machines, and the gate runs the same exchange between two daemons on
   one machine (`crates/muster-daemon/tests/daemon/linked.rs`). Later, pair groups and `--to` a
   pane across machines, a policy naming another machine's member, and the person's requests on
   a far machine carried to the human's home (sections 10 and 11), with the same two proofs.

## Rationale

**Inside `muster-daemon`, because every part the service needs is already there.** The daemon
runs on every machine Muster reaches, outlives the app, is installed over ssh, started through
Launch Services, handed off without ending its panes, and reached through a forwarded socket.
Presence is its detection, in the same process, and the doorbell is a write on a queue it owns. A
separate message daemon would need all of that a second time, plus a subscription back into the
pane daemon for presence. Kan `a_2Rtd0Ed0l` wanted a separate process so that messaging would not
depend on Muster; the crate boundary gives that, since `muster-msg` builds and tests with no daemon
and no panes. Moving it into its own process later would mean implementing the wake and presence
traits over the daemon's socket, with `muster-msg` unchanged.

**Wakes instead of waits.** Most rows of the v1 table trace to one fact: an agent could receive
only while blocked in a loop it had to keep choosing to run. A pushed notice removes the loop, and
with it the timeouts, the stranded designations, every waiter released at once, and the
structural reason to leave.

**Only a read moves the cursor.** The guard is only as true as the cursor, and only a read by the
agent itself shows the text reached the model. The same rule means the doorbell carries no body,
so it fits in one line, the only length a PTY carries safely.

**One writer per log**, because the guard compares a cursor with a head, which needs one order,
and there is no order across machines.

**No turn order.** v1's `next` field produced two-agent ping-pong by default and stranded everyone
when it named an agent that had stopped. Addressing says who a message is for, and the ring set
who hears an unaddressed one.

## Alternatives Considered

**Fix `pane send` and keep it as the message channel.** Its failures belong to the keyboard:
Return semantics, paste folding, dialogs, no evidence of receipt. `--confirm` and bracketed paste
already went as far as a PTY allows.

**Claude Code's `SendMessage`, and nothing of Muster's.** It needs no code, but it serves one
harness, has no groups, policy or shared log for the human, and crosses machines only through
Remote Control, which organization policy disables on the work devenv (`claude remote-control
--help`, 2026-09-17). Its inbox socket is kept as one adapter.

**A separate message daemon beside `muster-daemon`.** Kan `a_2Rtd0Ed0l`'s shape, from when herdr
was the only daemon. Rejected under Rationale; the crate boundary keeps it available.

**Council v1's transport, a shared file polled.** Single machine, and it requires the blocking wait
behind most of v1's failures.

**A relay service, as hcom (a prior-art agent messaging tool) uses MQTT.** An outside service is
what the work network disallows, and Muster already has a transport to every machine it reaches.

**Message bodies in every wake.** Saves the receiver a call. Rejected because the cursor would move
on text that may have been held or dropped, and a doorbell cannot carry a body.

**Queue posts to a remote group while the link is down.** A queued post lands behind messages its
author never saw, which is what the guard prevents; failing loudly lets the agent decide.

**A log every daemon writes, merged.** It tolerates partitions but gives no single order, which the
guard and the numbering require. A group spans a laptop and a few devenvs, where failing loudly on
a partition is acceptable.

**Enforce turn limits, rate limits or required roles.** Each is a workflow, and each can be a
skill's instruction.

## Consequences & Trade-offs

**`muster-daemon` holds agents' words.** Logs can contain anything an agent wrote, including
secrets pasted into a brief. They are readable only by the user, as the daemon's socket is, and stay
out of the run log. No verb deletes a group yet, so a log stays until its file is removed.

**Every wake costs the receiver a turn.** The default ring set wakes every member, which suits two
agents and is expensive for six. The council presets narrow it. The default stays permissive
because a default that woke nobody would look broken.

**Cross-machine messaging stops while no window runs**, under Decision 4's recommendation. Local
groups do not.

**A daemon from before messaging** answers `msg.*` requests refused, as MIP-3 section 9 requires,
and its protocol minor version lets the CLI tell before sending. The CLI says to update that
machine's daemon, which handoff makes free.

**A log written since stage 4 can hold a kind of entry older daemons cannot read**: who set a
group's policy, paused it or resumed it. A daemon from before then skips such a line with a
warning, as it skips any line it cannot parse, and loses nothing else. If the skipped line was the
log's last, that daemon's next post reuses its number, so going back to an older daemon after
changing a group's policy can leave two entries with one number.

**The build gains two musl `muster` binaries**, pure clients, and the gate gains a conformance file
and a Claude observation that needs re-recording when a Claude release changes the inbox socket.

## What is not yet verified

**Whether Claude Code delivers the daemon's wake on Linux.** Settled for macOS in
`docs/observations/claude-code-2.1.283.md`: from a process that is not the session's child, a
session in default mode delivers, one in bypass mode holds for approval whether or not the token is
sent, and `crossSessionInbound: "accept"` passed with `--settings` makes a bypass session deliver.
The four Linux cases were not run, because no Linux machine this repository reaches has a Claude
Code with credentials. Until they are, the inbox adapter is assumed to behave the same there.

**Whether `asyncRewake` wakes a session idle for hours.** Settled up to 65 minutes in
`docs/observations/claude-code-2.1.283.md`, sections 4 and 5, for a session bypassing
permission prompts on macOS: a `Stop` hook keeps running after its turn, its exit 2 starts a turn
in the idle session, every turn's end starts it again, and without a `timeout` Claude Code ends it
before eleven minutes. Longer than 65 minutes was not measured; nor was Linux.

**Which screens Claude Code's prompt rule reads as its prompt.** The rule is checked against
the screens recorded in `corpus/claude-code-2.1.283/`, and a live check holds that a new
session's suggestion is drawn faint. A dialog or menu that draws above an unchanged prompt box,
and that no rule recognizes, would read as an empty prompt and be rung. Each release that adds
one needs a rule for it, the same as for its state.

**How other harnesses draw an empty prompt.** Codex's was recorded for MIP-5; no other harness
has a prompt rule, so none is rung (Future Directions).

**Claude Code's limits** come from its documentation: about a million characters per message, at
most 50 accepted messages queued and 100 held. Only the notice crosses the socket, so none should
bind.

## Future Directions

- **Prompt rules for other harnesses**, recorded from each harness's empty prompt, so the
  doorbell can ring them. Codex's came with MIP-5, which says how a harness gets one.
- **Presence from `claude agents --json`** for Claude sessions outside a pane; it reports working,
  blocked and done per session (kan `a_2XMXOShAA`).
- **The daemon holding cross-machine links**, if Decision 4 is revisited.
- **Collision notices**, from hcom: telling two agents they edited the same file seconds apart.
- **Shared artifacts and forked sub-groups**, open cards on v1's board. Paths in messages and a
  second group cover both for now.

## Open Questions

- **Whether replicas should persist**, so a remote group's history stays readable while its home
  is unreachable.
- **Whether an idle group ever closes on its own**, or stays until deleted as proposed.

## References

- kan `a_2Rtd0Ed0l`: the design conversation of 2026-09-17, and what was established about
  Claude's inbox socket.
- Council v1: `amterp/council` at `1b6ff51`, its board, and `~/.council/sessions/`.
- MIP-3, sections 1, 9, 10, 11 and 12, and Future Directions; MIP-1 for one schema serving GUI,
  CLI and agents.
- kan `a_29e9be7aQ` (a devenv pane cannot drive its window); MIP-3 section 2 for agent-reported
  facts; `a_2AJS0Xz7I` (one harness we support, never the one we are).
- `docs/cli/limits.md`, on what `pane send` cannot promise.
- Claude Code documentation, read 2026-09-26: code.claude.com/docs/en/cross-session-messaging and
  code.claude.com/docs/en/hooks.
- Prior art: hcom (aannoo/hcom), Cyclops (cyclops-team/cyclops).

---

## History
- 2026-09-26 Draft, from kan `a_2Rtd0Ed0l`, a research pass over council v1 and its real sessions,
  and MIP-3.
- 2026-09-28 Stage 1 built: the Claude observation on macOS, the default name and the re-wake's
  timing answered (sections 3 and 5), `wait`'s output convention (section 6).
- 2026-09-28 Stage 1 reviewed: group names unique regardless of case, and a torn log line cut at
  load (section 12); `--as` held to the rule `join --name` is (section 3).
- 2026-09-28 Stage 2 built: presence from detection, panes addressed by name before they join,
  the doorbell rung only while idle or waiting and after three quiet seconds, and exit 6 for a
  post that wakes nobody live (sections 3, 4, 6, 7 and 14).
- 2026-09-28 A ring typed while Claude Code starts sits unsent in its prompt: Return is pressed
  again until the agent takes the ring (section 6).
- 2026-09-28 Stage 2 reviewed: the doorbell rings only a prompt it has just read as empty, and
  repeats Return only over its own text, so it never rings a harness it cannot read; a post
  counts only who can still be woken (sections 4, 6, 7 and 14).
- 2026-09-28 Stage 3 built: windows attend the daemon on their machine, what waits for the
  human is state in events and the snapshot, one banner per group lands on the transcript, and
  the human is exempt from the guard (sections 4 and 10).
- 2026-09-28 Decision 3 decided: `muster msg <verb>` on both platforms.
- 2026-09-28 Stage 4 built: policy enforced, `pause` and `resume`, the hooks adapter with `wait
  --due`, and the council skill (sections 6, 8, 12 and 13); `asyncRewake` checked live.
- 2026-09-28 Stage 5 built: groups across machines, with names turned on arrival, presence asked
  for rather than sent, and no pair groups or second hop across machines (sections 7 and 11). A
  group's policy binds at its home, replicas are sent it with every entry, and `@human` in a
  policy is the human on any machine (sections 8 and 11).
- 2026-09-28 Stage 5 reviewed: a daemon holds an empty replica from its start, names from a peer
  are checked, a change never answered is `unanswered`, and the near machine's name is kept
  (sections 11 and 12).
- 2026-09-28 Across machines, continued: `--to` finds a name or pane on a linked machine and
  makes the pair group there, a policy may name another machine's member, the person's requests
  on a far machine are carried to the human's home, and a replica reads a batch in its order
  (sections 10 and 11). The second hop is still not built.
- 2026-10-03 Urgent posts: `post --urgent` rings an agent in a pane while it works, at a prompt
  box read as empty, and wakes one woken already; a policy's `urgent` says who may (sections 5, 6
  and 8). Detection engine 6 reads a working agent's prompt.
- 2026-10-04 `group delete`: a group and its log go, every member is let go on every machine,
  and a replica that missed it forgets it at its next refetch (sections 8, 12 and 13).
