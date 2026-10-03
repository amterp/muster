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

An agent in a pane on an SSH machine does the same with the agents on that machine. Muster
installs a `muster` there beside the daemon, every pane there finds it on its `PATH`, and it asks
that machine's daemon. While a window is attached to both machines, agents on each reach the
other's by name or pane, and share groups with them (below).

## Who you are

Every verb works out who is asking, in this order:

- `--as NAME`, on any verb.
- The Claude Code session it runs in, from `$CLAUDE_CODE_MESSAGING_SOCKET`, which Claude Code
  sets in every command a session runs.
- The pane it runs in, from `$MUSTER_PANE`, once the daemon has found an agent there.
- Otherwise the human: a person's own shell has no agent identity, and is `@human`. So is a
  shell in a pane with no agent in it. On a devenv the laptop has linked to, that is you as the
  laptop's member, `@human@<laptop>` (below).

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
and joined by `+` (`builder+critic`), created on first use. A member on another machine counts by
its own name there, and a name already taken by a group holding anyone else gets `-2`, `-3`. A post that fits several groups, or
an unaddressed post from someone in several, is refused until it says which.

`--to A,B` decides who is woken, never who may read: every member of the group can read every
message in it. An unaddressed post wakes every member but its author, unless the group's policy
says otherwise.

**`--to` takes a pane's name too**, such as the one `muster pane new` prints. An agent in that
pane need not have joined anything, or even have started yet: it becomes a participant named
after the pane, and whatever runs `muster msg` in that pane from then on is it. A name or pane
that means nobody on this machine is looked for on the machines linked to it (below).

## A group's policy

A group made with `group new G --policy F` has the policy in file F, TOML with five keys:

    # a director and members who answer to it
    ring = { director = ["*"], "*" = ["director"] }
    allow = { director = ["*"], "@human" = ["*"], "*" = ["director", "@human"] }
    membership = ["director", "@human"]
    urgent = ["director", "@human"]
    paused = false

- **`ring`** says, per author, whom an unaddressed post wakes; `*` as the author is everyone
  not listed, and as a name is every member but the human. Waking the human raises a
  notification, so a set wakes the human only by naming `@human`. Here the director's post
  wakes every member but the human, and a member's wakes only the director.
- **`allow`** says, per author, whom a post may name in `--to`. Anyone else is refused, and the
  refusal says whom you may address. Here the director and the human may address anyone, and a
  member only the director or the human.
- **`membership`** says who may change the group: join it, leave it, `group add` and `group
  remove`, `group set`, `pause` and `resume`. Here a member cannot leave on its own, and the
  refusal names who may dismiss it.
- **`urgent`** says who may post with `--urgent` (below). Here only the director and the human
  may interrupt an agent at work; a member's urgent post is refused, and the refusal names who
  may. It is refused rather than sent as an ordinary post, which would arrive later than its
  author was told.
- **`paused`** makes a new group paused, as `pause` does. `group set` leaves whether a group is
  paused alone, and refuses a file saying `paused = true`: `pause` and `resume` change it.

A key left out keeps its default, which lets anyone do anything: `ring` of
`{ "*" = ["*", "@human"] }`, `allow` of `{ "*" = ["*"] }`, `membership` and `urgent` of `["*"]`. A key no
field reads is refused, so a misspelled key is not quietly the default. Names are participants' names, `*`, or `@human`. A group made by
`join` or by a post has the default. `group set` replaces the rest of the policy with a file's, and
every change of policy, and every pause and resume, is a line in the group's log.

`pause G` keeps every post and wakes nobody but the human, and a post says who it held.
`resume G` wakes each member once for what it has unread, including a member woken before the
pause, and says whom it woke as a post does. Pausing is how a person reading along asks a busy
group to stop for a moment.

## Being woken

A post wakes each participant it is for once, and not again for that group until it reads. Ten
messages arriving while an agent works cost it one wake. The wake is one line and never the
message:

    [muster] review: 3 new (#40-42), 1 to you, from director, critic. Read: muster msg read --group review

**An agent in a pane is woken by the doorbell**: the wake is typed into its pane as one line and
a Return, and only into a prompt the daemon has just read as empty. Just before it types, the
agent has to be idle or waiting, nothing may have been typed into the pane for three seconds
nor drawn there for half a second, the agent must still be running there, and its screen must be
its prompt with nothing typed in it. So a dialog, a menu, a prompt holding a draft, or a screen detection does not recognize is
never rung. Until then the wake waits in the daemon, and is rung as soon as the pane
allows.

**Only Claude Code and Codex are rung.** Reading an empty prompt needs a rule for it in the
harness's manifest, and so far only theirs have one; `muster docs harnesses` says what Muster gets
from each harness. An agent of any other harness in a pane is not woken, and the post says `its
prompt cannot be read`. An urgent post rings Claude Code at work, and waits for Codex to be idle.

An agent that neither starts work nor reads within five seconds of a ring has Return pressed
again, a few times, but only while its prompt holds the ring's own text and nothing else, and
nobody has typed into the pane since the ring. Claude Code keeps what is typed while it is
starting as its prompt, unsent, and the later Return sends it. If the prompt then holds anything
else, or somebody types into the pane - even something they take back, since the screen may not
show it yet - the ring ends, and the next post rings afresh.

An agent that goes idle with what it was woken for still unread is woken once more, with `still
unread` on the end, and then not again until it reads.

**An urgent post reaches an agent at work.** `post --urgent` is for what should change what an
agent is doing now, not once it is done:

    muster msg post --urgent --to builder "Stop: the schema changed under you. Read #41."

Its ring may be typed while the agent works. Claude Code queues a line typed into its prompt box
during a turn, and hands it to the model once the tool call it is in returns, with a reminder to
address it before going on (`docs/observations/claude-code-2.1.288.md`). Everything else above
still holds: nothing typed into the pane for three seconds, the agent still running there, and
its prompt box read as empty just before the ring. So a draft in the box, a dialog, a menu, or
a blocked agent is never rung; the post waits, and says what for. The wake counts what is
urgent and says to read now:

    [muster] review: 2 new (#41-42), 1 urgent, 1 to you, from director. Read it now, before you go on: muster msg read --group review

An urgent post wakes even an agent already woken for the group and not yet read, so each one
rings: once per batch is the rule for what can wait until a turn ends. A ring rung at work counts
as taken once the prompt box is empty again, which is how Claude Code shows a queued line. Whether
the model stops to read is its call: Claude Code's reminder asks it to, Sonnet does, and Haiku
4.5 has been seen finishing its task first.

Urgency changes nothing where messages already arrive mid-turn. A session whose hooks fetch its
messages reads it at its next tool call, as it would any post, and is still not rung; a session
outside a pane is sent it on its inbox. Nor does it change anything for the human, whom every
post that wakes them notifies at once.

**A Claude Code session can fetch its own messages with hooks** instead of being rung.
`extras/claude-code/messaging-hooks.json` holds two: after every tool call, `read --if-unread`
hands the model any message that arrived, and when a turn ends, `wait --due` waits in the
background and starts the next turn when a wake is due. Merge them into the session's settings or
pass the file with `--settings`. While a session's hooks are running, nothing is typed into its
pane: while it works, if one of its `muster msg` commands ran in the last five minutes, and
between turns while its `wait` is waiting. A session idle with no `wait` waiting ended its turn
without its `Stop` hook - an API error, or Esc - and is rung for what it has unread, as any agent
in a pane is.

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
    rung once it is not blocked: scout (blocked)
    rung once an agent is found: p2w3r07bsd
    not woken: scout (gone), p3w3r07bsd (no agent in its pane), @human (notified when a window opens)

"woke" means the wake was handed over - rung, or sent to the session - not that it was read. A
ring still to come says what it waits for: the agent to be idle, an urgent post's agent to be
out of its dialog, a draft left in its prompt to be sent or cleared, or an agent to start in a
pane opened under 30 seconds ago. An urgent post's first line says so: `posted #42 to review,
urgent`. A pane with no
agent past that, or whose agent's prompt cannot be read, is not woken.

A post that woke nobody live - nobody woken, to be rung, already woken, held until a group is
resumed or a machine can be reached, or the human - is still kept, and exits 6: whoever you meant
to tell is not there to hear it, and no answer is coming. With `--json` the same answer is lists
of names under `woke`, `deferred`, `already_woken`, `waiting`, `gone`, `no_agent`, `no_doorbell`,
`paused` and `unreachable`, what each agent in a pane is doing under
`doing`, what each deferred ring waits for under `until`: `idle`, `unblocked`, `prompt` or
`agent`, and whether the post was urgent under `urgent`.

## Reading, and the guard

`read` prints your unread messages, across your groups or in one with `--group`, and moves your
place past them. It skips your own messages and shows joins and leaves as one line:

    --- review #41 | director -> builder ---
    Take the lexer; leave the parser to critic.
    --- end review #41 | director ---
    --- review #43 | scout joined ---

An urgent message says so on its first line: `--- review #44 | director -> builder, urgent ---`.

**A post is refused while you have unread messages from others in that group**, and the refusal
says how many and the `read` that clears it. There is no override: read, then post again. The
human is not held to it: the guard keeps a model from acting on a conversation it has not seen,
and a person reads the transcript as it arrives, on a screen the daemon cannot see. Joins
and leaves never count as unread. Your place in a group starts where you joined it; `log` shows
what came before.

`log --group G [--since N]` prints the transcript and moves nothing. With `--follow` it goes on
printing each entry as it lands until interrupted, across Muster updating its daemon, and with
`--json` prints one line per entry.

`read --if-unread` prints nothing at all unless a message is unread, for a hook to run after every
tool call. Joins and leaves on their own print nothing, though your place still moves past them.

## Waiting

`wait [--group G] [--timeout S]` blocks until you have an unread message that would wake you,
then prints what a wake would say and exits 0. It returns at once if you already have one. A
newer `wait` by the same participant ends the older, which exits 1, as does one whose participant
leaves. With `--timeout` it exits 5 when nothing arrived. A wait survives Muster updating its
daemon: it asks the new one and goes on waiting.

With `--due` it returns only for a wake you are due: once for each batch of messages, and once
more `still unread` if you have not read them, as the doorbell would. That is what a `Stop` hook
needs, since plain `wait` would return at the end of every turn that left a message unread.
`--due`, like `join --pull`, also says your hooks fetch your messages, so you are not rung while
they are running.

**Do not run it in the foreground of an agent's turn**: that is the blocking loop this exists to
remove. It is for hooks and scripts.

## The human

`@human` is the person at Muster's window, on the machine the app runs on. A message that
wakes them - addressed to `@human`, or unaddressed in a group whose policy rings them - raises a
notification naming the group and who wrote, and chatter between agents raises nothing. Choosing
it, or ⌘⇧A, opens the group's transcript: a tab running `muster msg log --group G --follow`, or
the pane already running it. Going there counts as reading the group. With no window open, the
post says `@human (notified when a window opens)`, and the next window to open notifies.

The human posts with `muster msg post` from any shell of their own, the transcript's included
once Ctrl-C has stopped the follow and left its shell.

## Every verb

| verb | does |
|---|---|
| `join [--name N] [--group G] [--pull]` | registers you, and joins a group, creating it if absent |
| `leave [--group G]` | leaves a group; with none, leaves every group and stops taking part |
| `who [--group G]` | who takes part: alive, gone, unreachable or the human, what each in a pane is doing, and their groups |
| `post [--group G] [--to A,B] [--urgent] [TEXT \| --file F \| -]` | appends a message and wakes whom it is for; `--urgent` reaches agents at work |
| `read [--group G] [--if-unread]` | prints your unread messages and moves your place |
| `log --group G [--since N] [--follow]` | the transcript, moving nothing; `--follow` keeps printing |
| `wait [--group G] [--timeout S] [--due]` | blocks until a message would wake you |
| `groups` | every group, its members, and whether it is paused; `--json` has each policy |
| `group new G [--policy F]` | makes a group with that policy, and joins it as its first member |
| `group set G --policy F` | replaces a group's policy |
| `group add G NAME...` / `group remove G NAME...` | adds or removes members, by name or pane |
| `pause G` | holds a group's wakes: its posts wake nobody but the human |
| `resume G` | wakes each member once for what it has unread, and lets posts wake again |

Every verb takes `--as NAME` and `--json`. Exit codes are the CLI's own: 1 refused, including
by a group's policy, 3 no daemon to ask, 4 no answer, so it may or may not have happened (the
daemon hung up, or another machine never answered), 5 a wait that timed out, 6 a post that woke
nobody live.

## Across machines

A group is kept on the machine it was made on. While a Muster window is attached to your laptop's
daemon and an SSH machine's, it links the two, and a group on either can be joined and posted to
from the other:

    # on the laptop
    muster msg join --name builder --group review
    # on the devenv
    muster msg join --name critic --group review    # joined review@your-laptop as critic

- **Names say which machine.** Each machine writes the other's members and groups with that
  machine's name: the laptop sees `critic@devenv`, the devenv sees `builder@your-laptop`. The
  laptop calls a machine by its `[[daemon]] id`, and the machine calls the laptop by the name the
  laptop's daemon chose when it first linked and keeps from then on: on a Mac the computer's name
  from Sharing settings, which a new network does not change. A bare name is the one kept on this machine, or else the one elsewhere that goes by it,
  and `review@devenv` says which when both machines have one. A name written as the other
  machine writes one of this machine's is this machine's own, so on the devenv `review@devenv`
  is `review`, and a name copied from the laptop's output works there. `join` with a bare name asks the
  linked machines first, and makes the group here only if none keeps one. While a machine linked
  to since the daemon started is down, it cannot be asked, so that `join` is refused,
  `unchecked`: join by the full name once the link is back, or make the group here with `group
  new`.
- **`--to` reaches the other machine.** A name or pane nobody on this machine goes by is asked
  of each linked machine, or of the one `name@machine` names. A post to someone who shares no
  group with you makes the group of the two of you on your machine, as on one machine: the
  laptop's `builder+critic` is `builder+critic@your-laptop` on the devenv. A pane named this way
  becomes a participant on its own machine and is rung there. `group add` finds names the same
  way.
- **Each machine wakes its own agents.** A post is numbered on the group's machine, and the guard
  counts what you have not read there, so it holds across machines as it does on one. A post's
  answer says what each machine did for its own agents.
- **Without the link, nothing crosses.** With no window attached to both, or while the connection
  is down, a post, join or leave for a group kept on the other machine is refused at once, naming
  the machine: exit 1, `unreachable` in `--json`. Nothing is queued, since a queued post would land
  behind messages its author never saw. Groups kept here work as ever. `read`, `log` and `wait`
  answer from this machine's copy, which after its daemon restarts or updates holds nothing until
  the link returns, and `read` and `log` say it may be behind; `log --follow` waits. What was
  posted meanwhile arrives when the link returns, and wakes whoever it is for, so a post whose
  only member is on a machine that cannot be reached exits 0, not 6.
- **A change with no answer may have happened.** When the other machine takes a post, join or
  leave and does not answer in time, it is refused as `unanswered`, exit 4: it may have landed
  there. `muster msg log` says whether, so look before posting again. Until the group is heard
  from again, `read` and `log` say it may be behind.
- **A group's policy is its home's.** A post, join or leave from another machine is held to it
  as one made there is, and a refusal names members as you name them. A policy can name a
  member on another machine as the group's machine names it, `director@devenv`, so an agent on
  the devenv can direct a council kept on the laptop, or join a group whose `membership` names
  it. `group set`, `group add`, `group remove`, `pause` and `resume` run only on the group's
  machine; elsewhere an agent is refused, `kept_elsewhere`, naming it. There, `group remove
  review critic@devenv` removes a member on another machine, which lets it go as a leave would.
  A pause holds wakes on both machines, and a resume wakes each machine's members.
- **The human is on the laptop.** `@human` in a policy means you wherever the group is kept, and
  a devenv post that wakes you notifies through the laptop's windows. The guard never holds your
  post, on either machine. Once the laptop has linked to a devenv, the devenv has no human of its
  own: an agent there that addresses `@human` means you, and your own shell there is you as the
  laptop's member. The devenv sends to the laptop, where your cursors are, whatever it cannot do
  for you there: reading, waiting, joining, leaving, making a group, posting in a group kept
  elsewhere or in a new group of two, changing a group kept on the laptop, and anything about a
  group it holds nothing of. The laptop does it and answers in its own names, `review@devenv`
  for the devenv's `review`. So `muster msg post --to <pane>` from a devenv shell reaches the
  agent in that pane. With no link to the laptop up, these are refused, `human_elsewhere`,
  naming it.

## Where messages are kept

In a directory beside the daemon's socket, `<install>.msg/`, readable by you only: a log per
group, synced before a post is answered, and a file of participants and their places. Logs
survive the daemon restarting, the machine going down, and a new daemon taking over from an old
one. Message bodies never enter the daemon's own log, which records who posted how many bytes
where.

## Not yet

An urgent post needs this machine's daemon, and the daemon of the machine its group is kept on,
to be from a Muster that knows it; either being older refuses it, saying which. A member on a
machine whose daemon is older is rung for it as for an ordinary post. A `group set` from an older
Muster, which knows nothing of `urgent`, leaves the group's list as it was.

A daemon links only to the machines a window attaches it to, so an agent on one devenv cannot
reach an agent or a group on another: messages cross from the laptop to each devenv and back,
not between devenvs. No verb deletes a group.
`docs/mip/0004-agent-messaging.md` is the
design and its order.
