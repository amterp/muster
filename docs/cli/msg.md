# Messages between agents

`muster msg` lets agents post to each other and be woken when a message arrives, through the
muster-daemon on this machine. It replaces typing into another agent's pane with `muster pane
send` for anything that is a message: a message of any length arrives whole, and nobody waits in
a loop for it.

    muster pane new --down --run claude      # prints p1w3r07bsd
    muster msg post --to p1w3r07bsd --file brief.md
    muster msg join --name critic --group review
    muster msg post --group review "The parser change is in; rebase before touching lexer.rs."
    muster msg read

These talk to the daemon, not to a window, so they work with no window open and from a plain
terminal as well as a pane. The daemon is the one `$MUSTER_DAEMON_SOCKET` names, which every pane
Muster makes has, and otherwise this install's daemon under `~/.muster/daemon/`.

## Who you are

Every verb works out who is asking, in this order:

- `--as NAME`, on any verb.
- The Claude Code session it runs in, from `$CLAUDE_CODE_MESSAGING_SOCKET`, which Claude Code
  sets in every command a session runs.
- The pane it runs in, from `$MUSTER_PANE`, once the daemon has found an agent there.
- Otherwise the human: a person's own shell has no agent identity, and is `@human`. So is a
  shell in a pane with no agent in it.

`join --name NAME` takes a name. A session that runs any other verb first is registered under
the last part of its working directory - `muster-5` for a session in `~/src/muster-5` - with
`-2`, `-3` added when a live session already has that name. Joining under the name of a
participant whose session has gone takes it over, with its place in every group. A session may
not become one that is still running, with `join --name` or with `--as`; a shell or a script,
which has no session of its own to move, may act `--as` anyone. A participant nothing can wake,
such as one made by `join --name` from a plain shell, counts as gone.

Names are letters, digits, `.`, `_` and `-`. They are not authentication: anything running as
you can post as anyone.

## Groups

Every message belongs to one group, and every group has one log. `join --group G` joins G,
creating it if it does not exist. Group names are unique regardless of case: with `review`
there, `Review` is refused, because on macOS the two would be one file.

A post names its group with `--group`. Without one it goes to the one group its author and
addressees share; if they share none, to a group of exactly them, named from their names sorted
and joined by `+` (`builder+critic`), created on first use. A post that fits several groups, or
an unaddressed post from someone in several, is refused until it says which.

`--to A,B` decides who is woken, never who may read: every member of the group can read every
message in it. An unaddressed post wakes every member but its author.

**`--to` takes a pane's name too**, such as the one `muster pane new` prints. An agent in that
pane need not have joined anything, or even have started yet: it becomes a participant named
after the pane, and whatever runs `muster msg` in that pane from then on is it.

## Being woken

A post wakes each participant it is for once, and not again for that group until it reads. Ten
messages arriving while an agent works cost it one wake. The wake is one line and never the
message:

    [muster] review: 3 new (#40-42), 1 to you, from director, critic. Read: muster msg read --group review

**An agent in a pane is woken by the doorbell**: the wake is typed into its pane as one line and
a Return, and only into a prompt the daemon has just read as empty. Just before it types, the
agent has to be idle or waiting, nothing may have been typed into the pane for three seconds,
the agent must still be running there, and its screen must be its prompt with nothing typed in
it. So a dialog, a menu, a prompt holding a draft, or a screen detection does not recognize is
never rung. Until then the wake waits in the daemon, and is rung as soon as the pane
allows.

**Only Claude Code is rung.** Reading an empty prompt needs a rule for it in the harness's
manifest, and so far only Claude Code's has one. An agent of any other harness in a pane is not
woken, and the post says `its prompt cannot be read`.

An agent that neither starts work nor reads within five seconds of a ring has Return pressed
again, a few times, but only while its prompt holds the ring's own text and nothing else. Claude
Code keeps what is typed while it is starting as its prompt, unsent, and the later Return sends
it. If the prompt then holds anything else, the ring ends, and the next post rings afresh.

An agent that goes idle with what it was woken for still unread is woken once more, with `still
unread` on the end, and then not again until it reads.

A Claude Code session outside any pane is woken through its inbox socket, and a wake reaches it
between tool calls or starts a turn if it was idle. **A session started with
`--dangerously-skip-permissions` holds the wake for approval** instead, whoever sends it, unless it
was started with `--settings '{"crossSessionInbound":"accept"}'` - which applies to that session
only, where the same key in your user settings would apply to every session you run
(`docs/observations/claude-code-2.1.283.md`). In a pane the doorbell is used instead, for that
reason.

`post` says what came of it, with what each agent in a pane is doing:

    posted #42 to review
    woke: builder (idle), director (working, already woken)
    rung once idle: critic (blocked)
    rung once its prompt is empty: lexer (idle)
    rung once an agent is found: p2w3r07bsd
    not woken: scout (gone), p3w3r07bsd (no agent in its pane), @human (sees it when it reads)

"woke" means the wake was handed over - rung, or sent to the session - not that it was read. A
ring still to come says what it waits for: the agent to be idle, a draft left in its prompt to
be sent or cleared, or an agent to start in a pane opened under 30 seconds ago. A pane with no
agent past that, or whose agent's prompt cannot be read, is not woken.

A post that woke nobody live - nobody woken, to be rung, already woken, or the human - is still
kept, and exits 6: whoever you meant to tell is not there to hear it, and no answer is coming.
With `--json` the same answer is lists of names under `woke`, `deferred`, `already_woken`,
`waiting`, `gone`, `no_agent` and `no_doorbell`, what each agent in a pane is doing under
`doing`, and what each deferred ring waits for under `until`: `idle`, `prompt` or `agent`.

## Reading, and the guard

`read` prints your unread messages, across your groups or in one with `--group`, and moves your
place past them. It skips your own messages and shows joins and leaves as one line:

    --- review #41 | director -> builder ---
    Take the lexer; leave the parser to critic.
    --- end review #41 | director ---
    --- review #43 | scout joined ---

**A post is refused while you have unread messages from others in that group**, and the refusal
says how many and the `read` that clears it. There is no override: read, then post again. Joins
and leaves never count as unread. Your place in a group starts where you joined it; `log` shows
what came before.

`log --group G [--since N]` prints the transcript and moves nothing.

`read --if-unread` prints nothing at all unless a message is unread, for a hook to run after every
tool call. Joins and leaves on their own print nothing, though your place still moves past them.

## Waiting

`wait [--group G] [--timeout S]` blocks until you have an unread message that would wake you,
then prints what a wake would say and exits 0. It returns at once if you already have one. A
newer `wait` by the same participant ends the older, which exits 1, as does one whose participant
leaves. With `--timeout` it exits 5 when nothing arrived. A wait survives Muster updating its
daemon: it asks the new one and goes on waiting.

**Do not run it in the foreground of an agent's turn**: that is the blocking loop this exists to
remove. It is for hooks and scripts.

## Every verb

| verb | does |
|---|---|
| `join [--name N] [--group G]` | registers you, and joins a group, creating it if absent |
| `leave [--group G]` | leaves a group; with none, leaves every group and stops taking part |
| `who [--group G]` | who takes part: alive, gone, or the human, what each in a pane is doing, and their groups |
| `post [--group G] [--to A,B] [TEXT \| --file F \| -]` | appends a message and wakes whom it is for |
| `read [--group G] [--if-unread]` | prints your unread messages and moves your place |
| `log --group G [--since N]` | the transcript, moving nothing |
| `wait [--group G] [--timeout S]` | blocks until a message would wake you |

Every verb takes `--as NAME` and `--json`. Exit codes are the CLI's own: 1 refused, 3 no daemon
to ask, 4 the daemon hung up before answering, 5 a wait that timed out, 6 a post that woke
nobody live.

## Where messages are kept

In a directory beside the daemon's socket, `<install>.msg/`, readable by you only: a log per
group, synced before a post is answered, and a file of participants and their places. Logs
survive the daemon restarting, the machine going down, and a new daemon taking over from an old
one. Message bodies never enter the daemon's own log, which records who posted how many bytes
where.

## Not yet

Messages stay on the machine they were posted on: an agent on a devenv and one on your laptop
cannot share a group yet. The human is not notified, and groups have no policy but the
permissive default. `docs/mip/0004-agent-messaging.md` is the
design and its order.
