# The API, for programs

A program that is not an agent in a pane, such as a dashboard, a phone's server or a notifier,
reads and acts on Muster through the daemon's API: protobuf over the daemon's Unix socket, with a
schema and a version of its own (MIP-10). `muster window --json` is written for agents and scripts
and makes no promise to a program; the API does.

    muster docs api --schema > muster_api.proto

prints the schema this `muster` was built with. Generate bindings from that file alone, in any
language: it imports nothing. Its header has every rule a client needs, and this page is the way
in. `extras/api-client-go/` in Muster's repository is a client generated from it, in Go.

## Connecting

The daemon listens on a socket only its user can open. A program finds it as `muster msg` does:
`$MUSTER_DAEMON_SOCKET`, which every pane Muster makes has, and otherwise this install's daemon,
`~/.muster/daemon/release.sock` for the installed app. `muster daemons` lists every daemon on
this machine with its socket.

Every frame either way is a four-byte big-endian length, then one message of at most 16 MiB. The
first frame a client sends is a `Hello`:

    kind:   KIND_API
    api:    the version the client was generated from, such as 0.2
    client: a name and a version, which the daemon logs and scopes operation ids by

The daemon answers with a `HelloAnswer`: a `Welcome`, or a `Refused` saying why. After a Welcome
the client sends `Request`s and the daemon sends `DaemonMessage`s, until either end hangs up.
Nothing starts a daemon for a program that finds none listening: Muster's app starts one, and so
does a `muster msg` verb.

## Versions

A version is a major and a minor. The daemon serves a client of its own major no older than the
oldest version it still serves, and a newer minor too, saying its own version in the Welcome; the
client then checks that it can read that daemon. Every change to the schema raises the minor.
Until Muster's 1.0 the API is 0.x and a minor may break clients; the release notes say when one
does. `Welcome.machine.features` lists the requests and topics this daemon serves, so a client
asks the Welcome rather than finding out from a refusal.

## A first session

    → Hello            kind KIND_API, api 0.2, client "dashboard"
    ← HelloAnswer      welcome: this daemon's version, identity and features
    → Request          correlation "1", api.subscribe: TOPIC_PANES, TOPIC_ATTENTION
    ← Answer           correlation "1", DONE, a snapshot: every pane, what waits on the human
    ← Event            number 1: a pane changed
    ← Event            number 2: a message is waiting for the human
    ...

Every request carries a correlation id the client chooses, and its answer repeats it. Every
answer is one of five outcomes: DONE, ALREADY_SO, REFUSED with a refusal code and a reason,
NOT_SENT, or UNKNOWN, which may have happened and is never read as refused. The header's
"Requests and answers" says what each promises.

## Requests

Requests are namespaced by what they act on, each an arm of `Request`'s oneof. A request the daemon
does not know is refused as UNSUPPORTED and the connection stays open. A connection carries out its
requests one at a time, in the order sent, so a program that wants to read while a confirmed send or
a restart runs does so on a second connection.

### api.subscribe

Names the topics the client wants. The answer carries a snapshot of them, and every event after it
follows on the same connection, numbered from 1 in an epoch of the subscription's own. A gap in
the numbers, or a new epoch, means subscribe again: the fresh snapshot is the whole repair. One
connection holds one subscription, and subscribing again replaces it. Subscribing reads nothing
for the human and moves nobody's place in any group.

### api.read_pane

One page of a pane's text, at most 4 MiB: the newest rows, the rows from a row on, or what its
agent printed in its last turn, the pages `muster pane read` reads. Rows are numbered from the
start of the pane's history, so a page's `first_row` and `rows` say where the next one starts. A
pane read is not a pane seen: a finished agent stays DONE.

### msg.history

A page of a group's log: the entries after `after` up to `through`, oldest first, bounded by bytes
per page, with `more` when older entries of the range were left for another page. A client passes
the snapshot's `latest` as `through`, since the live stream carries every entry after it, so after
reconnecting it fetches only what it lacks. A group deleted, or whose earlier entries are gone,
says so rather than answering nothing.

### msg.post

Posts as `muster msg post` does: as the human unless `as` names another participant, to the group
it names or, naming none, the group the author and `to` share, made for it when there is none.
Every post records the client's name from its Hello, shown beside the author as `Alex (@human) via
foreman`. That name is attribution, not identity: any program can give any name. A post offering
`answers` is a question, and one with `answering` answers a question. A question takes one
answer, so a second is refused ANSWERED, and a phone can say it was answered at the desk.

It carries a required operation id, as `pane.new` does. The group's home keeps it in the post's
own log entry, so a repeat is ALREADY_SO with the first post's group and number, across a
reconnect, a handoff and a restart, and after the group is deleted. A post to a group kept on
another machine is NOT_SENT when it never left this daemon, and UNKNOWN when it was sent and the
reply was lost; asking again under the same id is safe in both. Where the human is homed on
another machine, a post as them is refused UNSUPPORTED: post through that machine's daemon.

### msg.ack

Moves the human's place in a group on to `through`, the number of the last message the client
showed them, and never back. What arrived since stays unread at the desk, and an Unread event says
the new place before the answer. Only this acknowledges anything: fetching history, replying and
dismissing a notification read nothing for the human.

### pane.new

Makes a pane as `muster pane new --run` does: a command in a directory, which must be absolute, in
a tab of its own and hidden if asked. As a helper of another pane it is hidden among its maker's
helpers, quiet unless asked otherwise, and in its maker's directory unless given one.

It carries an operation id the client chooses, a UUID will do, which the daemon remembers for 24
hours by the client's name, across a reconnect, a handoff and an orderly restart. A repeat is never
carried out again: it is ALREADY_SO with the first pane, or UNKNOWN when the first may have run and
lost its result. The header's "Operation ids" has the rest.

### pane.send

Types into a pane as `muster pane send` does: the text, then each key as a keystroke a dialog or
menu reads, then Return if asked. Text sent to an agent whose prompt already holds a draft is
refused unless the request asks to append, since the next Return would submit both. A key the
daemon cannot read, or a send of nothing, is refused INVALID before anything is typed.

With `confirm`, the daemon reads the pane back: DONE once the text shows more often than it did,
REFUSED when the pane shows it was not taken, and UNKNOWN when the pane changed without showing it
or could not be read. That says the text arrived, not that it was submitted. Without `confirm`,
DONE means the daemon handed the send to the pane. Either way, a send is refused with nothing typed
while the pane's agent is restarting, or while its program has stopped reading and the daemon holds
as much of its input as it will, and sending it again later is safe. A send is never retried for
the client: one answered UNKNOWN may have been typed, and sending it again may type it twice.

### pane.close

Closes a pane, ending whatever runs in it. The pane's PaneClosed event reaches the subscription
before the answer, carrying the request's correlation id. A pane already gone is refused NOT_FOUND,
so asking again is safe without an operation id.

### pane.compact

Compacts the context of the agent in a pane, as `muster pane compact` does, with an optional focus
and a prompt to carry on with, each one line. DONE means the compaction is queued: the daemon types
it once the agent is idle at an empty prompt, never in the middle of a turn. A pane running no
agent, or one whose harness Muster cannot compact, is refused.

### pane.restart

Ends the agent in a pane and resumes its session at the shell left behind, with the flags it ran
with, as `muster pane restart` does. The answer names the program ended, whether it had to be
killed, and the line typed. An agent whose harness reported no session is refused with nothing
ended. An agent that was ended but whose session could not be resumed is answered UNKNOWN, since
REFUSED would say nothing happened: the reason says what was done and gives the line that resumes
the session.

## Topics

### Panes

`TOPIC_PANES`: every pane the daemon runs, each as a whole `Pane`, with its name, callsign,
directory, harness, the agent's state and since when, what the agent reports of itself, its maker
and whether it is hidden or quiet. An event when a pane appears, when anything a client reads of
it changes, and its id when it closes. A client replaces what it holds of a pane with each event's.
Output fires nothing, and neither does a title on its own.

### Machines

`TOPIC_MACHINES`: each daemon in the snapshot, by its identity, its display name, its version and
its features. Today that is the daemon answering.

### Messages

`TOPIC_MESSAGES`: every group the daemon keeps, without bodies: its members, its policy, whether it
is paused, the number of its latest entry and the human's place in it. An event when a group is
made, changes or is deleted, and one for every entry logged, a post with its body. A post can be
up to 1 MiB, so `msg.history` is how a client catches up on a long backlog.

### Attention

`TOPIC_ATTENTION`: what waits on the human now, as state rather than as a record of
notifications. Each pane asking for them, blocked or stuck; each group with messages that would
ring them, with how many and the newest; and their presence at the desk as Muster's app reports
it, which reads as unknown once the app stops reporting. With Muster's app not running there is no
presence, and attention still accumulates. An event when any of it changes, and one when the app
shows the human a banner or posts a notification, which a client may use and nothing depends on.
What the human has read of a group is one event to a client hearing either this topic or Messages.

## Ending and budgets

The daemon ends a connection with an `Ended` saying why, when it can: REPLACED when a newer daemon
took over, so connect to the same socket and subscribe again; STOPPING when it exits; FELL_BEHIND
when the client stopped reading and more than 4096 messages or 32 MiB waited for it, with the
number of the last event it was sent. A client ended for falling behind subscribes again and
fetches the pages it lacks.

## What the API does not do yet

The CLI reaches further. Layout, tabs, windows and focus, and other machines' panes are later
requests and topics (MIP-10, "How it lands"). A `muster` verb added later that reads or acts on
agents comes to the API in the same change, or the change says why not.
