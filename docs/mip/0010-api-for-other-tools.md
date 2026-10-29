---
mip: 10
title: An API for other tools, served by the daemon
status: Draft
kind: Architecture
created: 2026-10-28
decided:
supersedes:
superseded-by:
related: 3, 4, 5, 7, 9
---

# MIP-10: An API for other tools, served by the daemon

## Summary

muster-daemon gets a public API: a protobuf protocol on its Unix socket, with a schema and a
version of its own, through which another program on the same machine reads what Muster's agents
are doing and acts on them. It is meant to last and to serve integrations nobody has written yet,
so its reach is the CLI's: over time, everything a person or an agent can do through `muster`, a
program can do through the API. Its rules (topics a client chooses, requests a client can list,
values a client may not know yet) let it grow by minor versions without a new major.

The first stages give a client one subscription, a snapshot followed by numbered events: every pane
and its agent's state, every linked machine's status, every group and its posts, and what waits on
the human. It can post and answer as the human, acknowledge what it showed them, start an agent in
a new pane, type into a pane and close one. One daemon answers for every machine linked to it, so a
client connected to the laptop's daemon also sees and reaches the devenv's agents and their
conversations. A client that restarts or loses its connection resumes without missing anything,
and when it cannot know whether a request happened, the answer says so.

Three of amterp's tools already drive Muster from outside by running `muster`, and two of them
parse its `--json`, which nothing promises to keep stable. Foreman, his tool for working with his
agents from his phone, would be the fourth, and needs a stream that survives a restart. They are
the first cases, not the scope.

The API is local. It listens only on the daemon's socket, which only the user can open, so it
grants nothing that a program running as the user cannot already do through `muster`. This
reverses the non-goal in `AGENTS.md` that the daemon "has no interface of its own", and builds the
API that the desideratum "AI-native surface" already names beside the GUI and the CLI. Five
questions for amterp end the document.

## Context / Motivation

### What was asked for

amterp, 2026-10-28, while designing Foreman (dictated, lightly cleaned up): "we need to formalize
some sort of Muster API. I guess the daemons would probably be the providers of this API, but it
should be an API for other tools to basically plug in and be able to interact with agents running
through Muster." Foreman "should be able to subscribe to all the various Muster state and Muster
messages going around, be able to send messages, and the Muster daemon is basically able to route
things correctly." The banner and the group conversations keep working at the desk, and when he is
away a Foreman phone app shows the same messages.

On machines: "if I theoretically had a second daemon running on a remote where the two daemons are
connected, then the Foreman server should be able to fully read and understand all the agents
running on both daemons. And ideally, it only needs to connect to one daemon ... they kind of work
as a cluster." He does not see authentication between the daemons as an issue, since they are
already connected.

And on scope: "this API is not just built for what we need and what maybe some existing projects
need. Ideally, it is like a long-term API that can support pretty much any sort of Muster
integrations we want or anyone would want. Obviously within reason: if it becomes too difficult to
design, then hopefully there are no one-way doors. ... we're not overly myopic to the current cases
we know about."

Foreman's side of this is step 1 of `~/src/foreman/docs/roadmap.md`.

### What uses Muster from outside today

| Tool | What it runs | For |
|---|---|---|
| Quartermaster | `muster window --json`, polled by `qm-daemon` under a deadline (`crates/qm-daemon/src/muster.rs`) | which panes run agents and in what state, to share the machine |
| Scribe | `muster window --json`, `muster pane read`, `muster pane send --confirm` (`crates/scribe-core/src/muster.rs`) | delivering dictated text to an agent's prompt |
| Lectern | `muster msg post --as lectern-adapter` from its TypeScript adapter, reading only the exit code (`adapters/muster/src/runtime.ts`, `index.ts`) | sending a reader's comment thread to the agent that published the page |
| Foreman (planned) | nothing yet | a phone app: agents, their states, messages, starting agents |

Each one starts a process per request and relies on output or exit codes that `docs/cli/` writes
for agents and people, with no promise that they stay the same. A `muster window --watch` that
restarts has missed whatever changed while it was gone, and messages stream only one group at a
time, through `muster msg log --follow`.

### What the daemon already has

- **A protocol with the right rules.** Protobuf, length-prefixed, over a Unix socket, opened with a
  handshake naming a major and minor version. A baseline schema and a test make an incompatible
  change fail the gate. Subscribing returns a snapshot and the sequence number it is current to,
  then every later event in order (MIP-3 section 9).
- **The facts.** Each pane's agent state and facts, what its harness's adapter reports (context,
  model, cost, session), its maker, and whether it is hidden or quiet (MIP-3, MIP-5, MIP-7). It
  answers `muster window`, `--watch` and eleven `pane` verbs with no window open (`muster docs
  overview`, "With no window").
- **The messages.** Every group's numbered log, kept under `<install>.msg/`; the human as the
  participant `@human`; questions and their answers (MIP-4, MIP-9).
- **A private socket.** The daemon sets its socket to mode 0600
  (`crates/muster-daemon/src/main.rs`).

It lacks two things. Its protocol serves Muster's own app, bridges and CLI and changes with them:
every schema change is a new minor. And it knows only its own panes. Across machines, the window
attached to both is what joins them: it mirrors each daemon, and tells the local daemon where to
dial the far one for messages (`msg.peer`, MIP-4 section 11). MIP-4 recommended that the app hold
that link (its decision 4 (a)), and that is what was built, so with no window open, a laptop agent
and a devenv agent cannot message each other and no daemon can say what runs on the other machine.

### Why it needs a MIP

It reverses a non-goal, adds a contract Muster has to keep for programs it does not ship, and
decides how daemons on different machines answer for each other.

## Decision

### 1. A public API on the daemon's socket

A fifth kind of connection joins control, input, stream (MIP-3 section 9) and peer (MIP-4 section
11): `api`. A client opens the daemon's socket, sends a Hello naming `api` and the API version it
speaks, and from then on sends requests and receives answers and events on that one connection.

The API's Hello is read before anything about the internal protocol. Today the daemon checks the
internal version before it looks at the kind of connection (`crates/muster-daemon/src/server.rs`),
so an API client would otherwise depend on the internal Hello for life. An `api` Hello names only
the API's version.

The API has a schema of its own, `proto/muster_api.proto`, and a version of its own, apart from the
daemon's internal protocol: a major and a minor, a client served only by its own major, a minor for
every change, and `proto/muster_api.v1.baseline.proto` with a test that fails when the schema
stops reading it.

That test protects decoding, and a public API has to promise behavior as well. Within a major:

- A request that is served keeps being served, and a field it populated stays populated, with the
  same meaning, units and default. A field that should go is marked deprecated and still filled
  until the next major.
- An enum value keeps its meaning, and a refusal keeps its code.
- A corpus of recorded API conversations sits in `corpus/` beside the existing contract cases, and
  the gate fails when a newer daemon answers one of them differently.

Every API request runs the same daemon code that the CLI's request reaches with no window open, so
the desideratum "every action runs through one shared path exposed to GUI, CLI, and API alike"
holds by construction.

A client finds the socket as `muster msg` does: `$MUSTER_DAEMON_SOCKET`, which every pane has, and
otherwise this install's daemon under `~/.muster/daemon/`.

### 2. Built for integrations nobody has written yet

The API's reach is the CLI's. Every `muster` verb gets an API request, in the CLI's vocabulary, so
nothing a person or an agent can do through `muster` is out of a program's reach. The stages under
"How it lands" deliver it in the order integrations need it, starting with what sections 4 and 5
list. A verb only a window can carry out (anything about tabs, layout, focus or zoom) reaches the
API the way the CLI reaches it: the daemon carries the request to an attached window, and refuses
it naming the reason when none is attached.

Six rules keep later additions to minor versions:

- **Requests are namespaced** by what they act on: `pane.*`, `tab.*`, `msg.*`, `machine.*`,
  `agent.*`, `window.*`. A new kind of thing gets a namespace of its own.
- **A client subscribes to topics** it names: panes, machines, messages and attention at first,
  others later, such as layout or a pane's output. A topic added later reaches only the clients
  that ask for it, so it never floods a client written before it.
- **A client learns what is served, down to each option.** The answer to Hello lists features:
  every request, every topic, and every option of a request that came in a later minor. Each
  machine in the snapshot carries its own daemon's list, so a client knows whether a request about
  a pane on an older devenv will be understood there. Today's internal protocol already has to gate
  individual options this way, because an older daemon drops fields it does not know
  (`crates/muster-daemon-proto/src/version.rs`).
- **Unknown values are normal.** Every enum has an unspecified zero, a client treats a value it
  does not know as unknown, and it skips fields and events it does not know. Agent states show why:
  `waiting`, `stuck` and `unreachable` were added after the first four.
- **A client names itself** in Hello, with a name and a version, and posts record the name
  (section 5). The name is attribution, not identity: anything running as the user can claim any
  name. A later restricted kind of client, such as a read-only one or one reaching the daemon some
  other way, needs a trusted identity or a credential, which a later minor adds; a client on the
  user's own socket stays fully trusted.
- **The messages do not depend on the transport.** They are framed the same way over any byte
  stream, so a later authenticated transport for other machines, or a gateway to HTTP run outside
  the daemon, carries the same schema unchanged. That is what keeps "local only" (section 6)
  reversible.

Two integrations the rules leave room for, which no stage below delivers yet.

A terminal on a phone needs a pane's replay and output, and to type into it. It cannot attach as
another bridge does: a pane has one bridge, which sets the program's terminal size, and a second
attach either is refused or displaces the first (`Attach.takeover` in
`proto/muster_daemon.proto`; `docs/architecture.md`). So the API keeps two things apart. An
observer reads a pane's replay and output on a connection of its own, never sets its size, and can
neither displace the window's bridge nor slow it down, because the daemon drops an observer that
falls behind rather than waiting for it. Taking control of the pane, its size and its input, is a
separate, explicit request, and the window shows that someone else has it.

A harness Muster ships no adapter for could report its own state, context and session through
`agent.report`, which the internal protocol already takes from Muster's adapters (MIP-5), so
anybody could write an adapter.

### 3. Events, snapshots and numbering

A subscription gets a snapshot, then events, under rules that hold when topics are filtered and
other machines are added:

- **Numbers belong to the subscription.** Each subscription has an epoch, chosen when it starts,
  and numbers the events it delivers, one after another. A topic the client did not choose takes
  no numbers, so filtering never looks like a gap. A gap, or a new epoch, means the client
  subscribes again.
- **Every event names its source**: the stable identity of the daemon it came from, and that
  daemon's instance (MIP-3's `instance`). A daemon's identity is fixed when its install first
  starts and never changes. It is not the machine's configured name, and not the name another
  daemon calls it by, which is relative to that daemon today (`crates/muster-msg/src/across.rs`).
  Display names travel beside it. The source lets the daemon a client talks to drop duplicates
  and anything from an older link or a replaced daemon.
- **A snapshot is a view of independent sources, each with its status**: current, restoring (as
  MIP-3's snapshot says today), catching up, or unreachable. It is not one atomic picture across
  machines. Events from one source keep that source's order, and events from different machines
  have no order between them, as `docs/architecture.md` already says of daemons' streams.
- **The snapshot joins the stream without a gap.** It says the number it is current to, and the
  first event after it carries the next.
- **An answer comes after its events.** As in MIP-3, the events a request caused reach the
  subscription before the answer, which names the last of them. For a request carried to another
  machine, the daemon the client talks to applies the other machine's change to its own view
  before it passes the answer back.
- **A daemon replaced by a newer one** (MIP-3 section 10) ends every API connection, as it ends
  the app's. The client connects to the successor and subscribes again, in a new epoch.

### 4. What a client reads

`api.subscribe` names its topics and returns a snapshot, then events:

- **Panes.** What `muster window --json` gives today, as typed fields: id, callsign, given name,
  title, machine, directory, harness, state, facts, `unread_input`, context used, model, cost,
  session, `maker`, hidden and quiet, and whether the adapter is reporting. Events: a pane
  appears, changes or closes.

  `since` is when the pane's daemon saw the agent enter its current state. The daemon does not
  keep that today: `muster window` with no window prints no time, and the window counts from when
  it first saw the pane (`crates/muster-cli/src/windowless.rs`). The daemon starts recording it
  where it publishes a state change, and carries it across a handoff. When the daemon did not see
  the change, after a restart for instance, `since` is unknown and says so; it is never filled in
  with the moment a client first looked. A machine that stopped answering has its own
  `unreachable_since`, kept apart from how long its panes' last known state had lasted. Times are
  UTC milliseconds by the clock of the daemon that saw them, and two machines' clocks may differ.
- **Machines.** Each linked daemon: its stable identity and display name, `connected`, `stale` or
  restoring, and its list of features (section 2).
- **Messages.** The snapshot carries every group on every linked machine, including groups with no
  member on this machine (section 6): its name, home daemon by identity, members, policy, whether
  it is paused, the number of its latest post, and the human's place in it. It carries no bodies.
  `msg.history` then pages a group's posts, bounded by bytes per page, from any number up to the
  number the snapshot was current to, and the live stream carries every post after that. A client
  that reconnects asks only for the pages it lacks. A group deleted, or whose earlier posts are
  gone, says so rather than returning nothing. Events: a post; a group made, changed or deleted; a
  participant joining or leaving; the human's place moving.
- **Attention.** What waits on the human now, as state rather than as a record of notifications:
  each unread message that would ring the human (addressed to them, or rung by a group's policy),
  and each pane asking for them (blocked, stuck, or a quiet helper whose maker cannot hear it, as
  `muster docs agents` describes). It is in the snapshot, and each change is an event, so a client
  that restarts finds everything still waiting. Beside it, the human's **presence** at the desk, as
  the app reports it: whether a window attends, whether Muster is in front, and when the person last
  used it. A report expires after a set time, so a crashed app reads as unknown rather than as
  present. With Muster quit there is no presence, and attention still accumulates. When the app
  shows a banner or posts a notification, an event says so, as information a client may use; nothing
  depends on it.

Separately, `api.read_pane` returns a pane's text in pages, as `muster pane read` does.

Reading changes nothing. Subscribing and fetching move nobody's place in any group, read nothing
for the human, and clear no `done`.

Every subscription has a budget, in queued bytes as well as messages: the daemon's queue for a
control connection holds 4096 messages on the reasoning that events are a few hundred bytes
(`crates/muster-daemon/src/control.rs`), and message bodies of up to 1 MiB break that reasoning.
A client that falls behind its budget is disconnected with the number it had reached, and resumes
from there by subscribing again and fetching the pages it lacks.

Layout, tabs, windows, focus and sizes are later topics and requests, carried through a window
(section 2).

### 5. What a client does

Every request carries a correlation id that its answer and its events repeat. Each answer is one
of:

- done;
- already so;
- refused, because the thing named does not exist or for another stated reason;
- not sent: it never left this daemon, so nothing happened and it is safe to retry;
- unknown: it was sent to another machine, or the connection broke, and the reply is lost, so it
  may have happened.

The daemon already tells the last two apart internally (`Unsent` and `Unanswered` in
`crates/muster-daemon/src/messages/peer.rs`), and MIP-9 gives the window an Unknown state for the
same reason. A client never treats unknown as refused.

A post and a new pane carry an operation id the client chooses. The daemon that carries out the
request, a group's home or the pane's machine, remembers the ids it has seen for a retention
period, across a reconnect and a handoff, and answers a repeat with "already so" and the original
result. So a client can retry an unknown post or pane with the same id and never act twice within
that period. Typing into a pane is never retried for the client: its answer says what the
`--confirm` read-back saw, and input whose fate is unknown is reported as unknown.

The requests:

- Post a message, with offered answers or answering a question, as `muster msg post` does.
- Acknowledge the human's messages in a group through a given number, `msg.ack`. It moves the
  human's place to that number at the human's home if that is further on, and never back, so a
  phone acknowledges what it showed and nothing that arrived since. Fetching, replying and
  dismissing a notification are separate requests, and none of them acknowledges anything.
  `muster msg read`, which reads to the end, keeps working as it does.
- Make a pane that runs a command in a directory, on a machine, as a helper of a pane or not, as
  `muster pane new --run` does. With no pane named it gets a tab of its own, as it does from a
  plain terminal today.
- Type into a pane: text, Enter or keys, with the `--confirm` read-back, as `muster pane send` does.
- Close a pane, and compact or restart its agent.

A request acts as the human unless it names another participant, as `--as` does. Every post records
the client that sent it, which the conversation and `muster msg log` show beside the author:
`Alex (@human) via foreman`.

### 6. One daemon answers for every machine linked to it

A client connects to one daemon, and sees and reaches every machine that daemon is linked to.
Another machine's panes and groups arrive in the same snapshot, each naming its daemon. A request
about one of them is carried over the link to the daemon holding it, under the rules of sections 3
and 5. While a link is down, its machine's panes read `unreachable` with their `last_state`, as
`muster window` shows them, and requests about them answer "not sent".

This needs three things the daemon does not do today:

- **Panes over the link.** The link between two daemons carries messages. It also carries each
  side's pane snapshot and events, and requests about panes, in both directions.
- **Groups over the link without joining them.** Today a daemon holds a copy of another machine's
  group only if one of its own participants is a member (`crates/muster-msg/src/across.rs`), so a
  conversation between two devenv agents is invisible from the laptop. A new peer request lists
  and follows the far daemon's groups: those that exist, those made later, changes to policy and
  members, deletion, and catching up after a reconnect, with each copy saying whether it is
  behind. Following makes nobody a member, and a group's home stays the only daemon that writes
  its log.
- **A link that does not need a window.** Today the app dials, and a link lasts as long as the
  app's request for it. For a client to see every machine with no window open, the daemon keeps
  the list of machines the app last gave it and holds those links itself. This is MIP-4's decision
  4 (b), and it has to settle more than MIP-4's two unmeasured questions (whether a daemon started
  through Launch Services reaches the user's ssh agent, and what happens when ssh needs an
  interactive login):

  - who owns each tunnel while an app is open, and how that passes to the daemon when the app
    closes, without the two contending for one forward;
  - what list of machines the daemon keeps, and when a machine taken out of the config is dropped;
  - what a daemon that replaces another restores, since a handoff closes every link today;
  - two apps attached to one daemon;
  - a login that fails: ssh runs in batch mode and refuses a password, and with no app the failure
    has to reach a client as the machine's status;
  - where the human is at home. Today the daemon infers it from which side dialed last, unless a
    window attends (`crates/muster-msg/src/service.rs`). With links held by daemons the home is
    declared, by the machine the app runs on, instead of inferred.

  Each of these is an acceptance case for stage 4.

A daemon links only to the machines it is linked to directly. A client on the laptop sees the
laptop and every devenv, and a client on a devenv sees that devenv and the laptop but no other
devenv, until a second hop exists (card a_2YXDyGQIi).

**Muster adds no authentication.** A link is an ssh forward that the user's own ssh authenticated,
and the API never leaves the machine, so the API grants a program what the user's shell already
has: full control, including acting as any participant. A program that reaches the network for
Muster's agents, such as Foreman's server answering a phone, authenticates its own clients.

### 7. The human, at the desk and away

The human is one participant with one place in each group, whichever program reads for them. The
snapshot carries that place and every move of it is an event, so a phone learns when the person
read at the desk, and the desk learns when the phone acknowledged. A question answered from a
phone shows as answered in the banner and the conversation through `answering`, and the group's
home refuses a second answer, as it does today.

Muster sends nothing to a phone. It keeps what waits on the human and what it knows of their
presence (section 4), and a client such as Foreman decides when to push, keeps its own record of
what it delivered, and acknowledges only what the person saw.

### 8. The CLI

The CLI keeps working as it does. Its `--json` stays written for agents and scripts and gets no new
promise; a program that needs one uses the API. Quartermaster, Scribe and Lectern can move to the
API when their owners want to, and nothing makes them. A `muster` verb added later that reads or
acts on agents is added to the API in the same change, or the change says why not.

### 9. Doctrine and docs that change

- The non-goal in `AGENTS.md` becomes: "Not a multiplexer. Muster runs a session daemon of its own,
  which exists to serve Muster; its one interface to other programs is the API (MIP-10)."
- `docs/architecture.md` gains the `api` connection and its event rules in the protocol section,
  and pane and group traffic on the link between daemons.
- `docs/cli/` gains an `api` topic, so `muster docs api` tells a client's author where the socket
  is and what each request does, and `muster docs api --schema` prints `muster_api.proto`, so a
  client in any language can generate its own bindings.

## Rationale

The CLI's reach is the API's scope because it is the boundary Muster has already drawn and keeps
documented: what a person or an agent may do to Muster from outside the window. A smaller, curated
API would make every new integration wait on a decision about whether it belongs.

The daemon serves the API because it holds the truth. The app owns none ("View = f(daemon
state)"), is often closed, and already hands the daemon every verb it can answer alone.

A schema of its own lets the public surface be smaller than the internal one and change on its own
clock. The internal protocol's minors are compatible too, but its fields exist for the app, the
bridges and the CLI, and grow whenever they need something; an outside program would depend on
all of it, and on its Hello, for life.

Protobuf is what the daemon already speaks, and what amterp's practice for parts in different
languages recommends. The clients so far are in Rust (Quartermaster's daemon, Scribe's core) and
TypeScript (Lectern), and Foreman's language is not chosen yet; each generates bindings from one
file.

One daemon answers for the cluster because that is how the person sees it, in one window, and
because the daemons already link for messages. A client that dialed every machine itself would
rebuild the ssh masters and forwards that the app supervises.

The API stays local because the socket's mode already says who may connect, and anything wider
needs accounts, network exposure and a threat model that belong to the program choosing to serve
the network.

## Alternatives Considered

**An API curated for the tools that exist.** Smaller to build and to keep. It lost because amterp
asked for an API any integration could use, and because each new integration would then begin with
a design argument about whether its request belongs in the API.

**Declare the CLI's `--json` the contract.** It costs least, and it is what three tools do today.
It lost because a long-lived subscriber would still start processes and parse lines, nothing would
resume after a reconnect, and every change to output written for agents would become a
compatibility question. The CLI should stay free to change for the agents reading it.

**Open the internal protocol to other programs.** No second schema to keep, and its minors are
already compatible on the wire. It lost because every field and request the app, the bridges and
the CLI ever need would become a public promise about behavior, which is a stronger promise than
wire compatibility (section 1). That protocol also carries pane bytes, input and the bridge's
control of a pane's size, which outside tools should reach only through the explicit requests in
section 2.

**HTTP and JSON from the daemon.** Easiest for a web client. It lost because it puts a network
listener and an HTTP stack in the process that owns every terminal, for no client that needs it:
Foreman's server, which does serve the network, runs beside the daemon on the same machine.

**The app serves the API.** It lost because the app is often closed and owns no truth, and the
daemon already answers without it.

**Each client dials every machine's daemon.** A simpler daemon. It lost because each client would
rebuild the app's ssh supervision, and amterp asked for one connection.

**Muster pushes to the phone itself.** It lost because that needs a server on the network, accounts
and a phone app, which are Foreman's job. Muster stays on the machine.

## Consequences & Trade-offs

- Muster keeps a second contract, for programs it does not ship, and it grows with the CLI: a new
  verb is also a new request. Each change to the API is a deliberate minor, and an incompatible one
  is a new major.
- The daemon does more: it relays another machine's panes and groups, remembers operation ids, and
  at stage 4 holds ssh links. A bug there now reaches outside clients as well as the window.
- The API's events are per state change and per post, never per byte of pane output, and message
  bodies move in bounded pages, so a subscriber costs little against the "frequency times
  cardinality" budget. A subscriber that stops reading is dropped at its byte budget, not held.
- Every agent's messages become readable by any program running as the user, through one request.
  They already are, as files under `<install>.msg/`; the API makes it easy.
- Quartermaster, Scribe and Lectern keep working unchanged.

## How it lands

1. **Reads on one machine.** The `api` connection and its Hello, the event rules of section 3,
   panes with a `since` the daemon keeps, machines with their features, groups with their heads
   and the human's place, `msg.history`, attention and presence, byte budgets, `api.read_pane`,
   the API corpus and `muster docs api`. Enough for Foreman's server to show agents, messages and
   what waits on the owner, without acting.
2. **Acting.** Correlation and operation ids, the "not sent" and "unknown" answers, posting and
   answering as the human, `msg.ack`, and making, typing into, closing, compacting and restarting
   panes.
3. **Machines, while a window holds the links.** Stable daemon identities, panes and followed
   groups over the link the app already dials, and requests carried across it.
4. **Machines with no window.** Daemon-held links, once a spike has measured MIP-4's two questions
   for decision 4 (b) on macOS and Linux, with the acceptance cases of section 6.
5. **Toward the CLI's reach.** The remaining verbs, window verbs carried through a window, observing
   a pane's output, and `agent.report`. Each is a minor, in the order integrations ask for them.

Every stage adds its cases to the API corpus. Among them: a subscription filtered to some topics;
a group on another machine with no member here; catching up a long backlog in pages; a handoff
mid-subscription; a reply lost after the other machine acted; the desk and a phone answering one
question at once; acknowledging an older message the phone showed while a newer one arrives; a
client restarting while something waits on the human; and Muster's app not running at all.

## Decisions for amterp

**Question A. Whether the API has a schema of its own.**

- (a) A separate `muster_api.proto` with its own version and Hello (section 1).
- (b) A public subset of the internal protocol, marked as such, under the internal version.

Recommendation: (a). It is a second schema to keep, but (b) makes every internal minor a question
about outside clients, which is the coupling this MIP exists to remove.

**Question B. Which messages a client reads.**

- (a) Every group on every linked machine, followed without joining (sections 4 and 6).
- (b) Only groups the human is in.

Recommendation: (a). It is what was asked ("all the various Muster messages going around"), and
the logs are already files the user can read. (b) would hide the agents' own conversations, which
Foreman needs to show what a team is doing.

**Question C. Whether stage 3 ships before the daemon holds its own links.**

- (a) Yes: a client sees other machines only while a window is open, until stage 4.
- (b) No: stages 3 and 4 ship together.

Recommendation: (a). The home Mac usually runs Muster, so Foreman loses little, and stage 4 waits on
measurement that may take a while.

**Question D. How a post a tool made for the human shows in the log.**

- (a) `Alex (@human) via foreman`, with the client's name kept on the post (section 5).
- (b) `Alex (@human)`, the same as one typed at the window.

Recommendation: (a). An agent that is told something it did not expect can see where it came from,
and so can amterp reading the log later. It costs one field on a post.

**Question E. Whether the CLI's `--json` gets a stability promise as well.**

- (a) No: programs use the API, and the CLI's output stays free to change (section 8).
- (b) Yes, for the outputs Quartermaster and Scribe parse today.

Recommendation: (a). (b) holds still output written for agents, to spare two tools a migration
their owner can do when he chooses.

## Open Questions

- What a client in a sandbox that refuses sockets can do. It meets the same limits as `muster msg`
  (`muster docs limits`).
- How long a daemon remembers operation ids, and whether that is a time or a count.
- How long the app's presence report lasts before it reads as unknown.
- How a followed group on another machine behaves after a long disconnect: refetch in pages, or
  start again from the group's head with the earlier posts marked as not held.

## References

- MIP-3 section 9, the protocol, its versions and its baseline.
- MIP-4 decision 4 and section 11, links between daemons.
- MIP-5, what each harness's adapter reports. MIP-7, helpers and `maker`. MIP-9, the human's banner
  and conversation.
- `~/src/foreman/docs/roadmap.md` and Foreman's board, card "Muster API" (the first consumer).
- The consumers today: Quartermaster `crates/qm-daemon/src/muster.rs`, Scribe
  `crates/scribe-core/src/muster.rs`, Lectern `adapters/muster/src/runtime.ts`.

---

## History
- 2026-10-28 Draft.
- 2026-10-28 Revised after a cross-model review (Codex gpt-6.1-sol): the event rules of section 3,
  groups followed across machines without joining, `msg.ack` through a number, attention and
  presence as state in place of ring events, a `since` the daemon keeps, "not sent" and "unknown"
  answers with operation ids, paged history and byte budgets, behavioral compatibility and
  per-option features, the lifecycle questions for daemon-held links, and an observer for a pane's
  output that cannot displace the window.
