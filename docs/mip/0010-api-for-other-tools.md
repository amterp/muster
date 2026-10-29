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
and its agent's state, every linked machine's status, and every message in every group. It can post
and answer as the human, start an agent in a new pane, type into a pane and close one. One daemon
answers for every machine linked to it, so a client connected to the laptop's daemon also sees and
reaches the devenv's agents.

Three of amterp's tools already drive Muster from outside by running `muster` and parsing its
`--json`, which nothing promises to keep stable. Foreman, his tool for working with his agents from
his phone, would be the fourth, and needs a stream that survives a restart. They are the first
cases, not the scope.

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
| Lectern | `muster msg post --as lectern-adapter` from its TypeScript adapter (`adapters/muster/src/runtime.ts`) | sending a reader's comment thread to the agent that published the page |
| Foreman (planned) | nothing yet | a phone app: agents, their states, messages, starting agents |

Each one starts a process per request and parses output that `docs/cli/` writes for agents and
people, with no promise that it stays the same. A `muster window --watch` that restarts has missed
whatever changed while it was gone, and messages stream only one group at a time, through `muster
msg log --follow`.

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

A fourth kind of connection joins control, input and stream (MIP-3 section 9): `api`. A client
opens the daemon's socket, sends a Hello naming `api` and the API version it speaks, and from then
on sends requests and receives answers and events on that one connection.

The API has a schema of its own, `proto/muster_api.proto`, and a version of its own, apart from the
daemon's internal protocol. It follows MIP-3's rules: a major and a minor, a client served only by
its own major, a minor for every change, and `proto/muster_api.v1.baseline.proto` with a test that
fails when the schema stops reading it. The internal protocol can then change every week, and the
API changes only when Muster decides it should.

Every API request runs the same daemon code that the CLI's request reaches with no window open, so
the desideratum "every action runs through one shared path exposed to GUI, CLI, and API alike"
holds by construction.

A client finds the socket as `muster msg` does: `$MUSTER_DAEMON_SOCKET`, which every pane has, and
otherwise this install's daemon under `~/.muster/daemon/`.

### 2. Built for integrations nobody has written yet

The API's reach is the CLI's. Every `muster` verb gets an API request, in the CLI's vocabulary, so
nothing a person or an agent can do through `muster` is out of a program's reach. The stages under
"How it lands" deliver it in the order integrations need it, starting with what sections 3 and 4
list. A verb only a window can carry out (anything about tabs, layout, focus or zoom) reaches the
API the way the CLI reaches it: the daemon carries the request to an attached window, and refuses
it naming the reason when none is attached.

Six rules keep later additions to minor versions:

- **Requests are namespaced** by what they act on: `pane.*`, `tab.*`, `msg.*`, `machine.*`,
  `agent.*`, `window.*`. A new kind of thing gets a namespace of its own.
- **A client subscribes to topics** it names: panes, machines, messages and rings at first, others
  later, such as layout or a pane's output. A topic added later reaches only the clients that ask
  for it, so it never floods a client written before it.
- **A client can list what the daemon serves.** The answer to Hello names every request and topic
  this daemon knows, so a client works against older and newer daemons of its major by asking
  rather than by reading the minor.
- **Unknown values are normal.** Every enum has an unspecified zero, a client treats a value it
  does not know as unknown, and it skips fields and events it does not know. Agent states show why:
  `waiting`, `stuck` and `unreachable` were added after the first four. The baseline test keeps a
  field's number and type from being reused.
- **A client names itself** in Hello, with a name and a version. Section 4 uses the name today. It
  also leaves room for per-client permissions, such as a read-only client, without a new major; in
  the first version every client may do everything.
- **The messages do not depend on the transport.** They are framed the same way over any byte
  stream, so a later authenticated transport for other machines, or a gateway to HTTP run outside
  the daemon, carries the same schema unchanged. That is what keeps "local only" (section 5)
  reversible.

Two integrations the rules leave room for, which no stage below delivers yet. A terminal on a phone
needs a pane's replay and output, and to type into it, as the internal stream connection gives a
bridge; the API would offer it as a second kind of API connection, so its bytes never share a
connection with events. And a harness Muster ships no adapter for could report its own state,
context and session through `agent.report`, which the internal protocol already takes from
Muster's adapters (MIP-5), so anybody could write an adapter.

### 3. What a client reads

One request, `api.subscribe`, returns a snapshot and the sequence number it is current to, then
streams every later event:

- **Panes.** What `muster window --json` gives today, as typed fields: id, callsign, given name,
  title, machine, directory, harness, state, `since`, facts, `unread_input`, context used, model,
  cost, session, `maker`, hidden and quiet, and whether the adapter is reporting. Events: a pane
  appears, changes or closes.
- **Machines.** Each linked daemon, `connected` or `stale`, as `daemons[]` has them.
- **Messages.** Every post in every group this daemon keeps or holds a replica of: group, number,
  author, addressees, body, offered answers, `answering` and `urgent`. Events: a post; a group made,
  changed or deleted; a participant joining or leaving.
- **Rings.** An event each time Muster rings the human, with a banner or a notification, naming the
  message and saying whether a window was in front and had the keyboard. This is what a client
  uses to decide that the person is away and needs the message somewhere else.

Separately, `api.read_pane` returns a pane's text in pages, as `muster pane read` does.

Reading changes nothing. Subscribing moves nobody's place in any group, reads nothing for the
human, and clears no `done`.

A client that reconnects names, for each group, the number of the last post it holds, and the
snapshot carries every later post. Panes need no such resume point, because the snapshot is the
present.

Layout, tabs, windows, focus and sizes are later topics and requests, carried through a window
(section 2).

### 4. What a client does

Each request answers as MIP-3's mutations do: done, already so, refused because the thing named
does not exist, or refused for another stated reason.

- Post a message, with offered answers or answering a question, as `muster msg post` does.
- Read the human's messages in a group, moving their place as `muster msg read` does.
- Make a pane that runs a command in a directory, on a machine, as a helper of a pane or not, as
  `muster pane new --run` does. With no pane named it gets a tab of its own, as it does from a
  plain terminal today.
- Type into a pane: text, Enter or keys, with the `--confirm` read-back, as `muster pane send` does.
- Close a pane, and compact or restart its agent.

A request acts as the human unless it names another participant, as `--as` does. Every post records
the client that sent it, which the conversation and `muster msg log` show beside the author:
`Alex (@human) via foreman`.

### 5. One daemon answers for every machine linked to it

A client connects to one daemon, and sees and reaches every machine that daemon is linked to.
Another machine's panes arrive in the same snapshot, named by their machine. A request about one of
them is carried over the link to the daemon holding it, and that daemon's answer comes back
unchanged. While a link is down, its machine's panes read `unreachable` with their `last_state`, as
`muster window` shows them, and requests about them are refused naming the machine.

This needs two things the daemon does not do today:

- **Panes over the link.** The link between two daemons carries messages. It also carries each
  side's pane snapshot and events, and requests about panes, in both directions.
- **A link that does not need a window.** Today the app dials. For a client to see every machine
  with no window open, the daemon keeps the list of machines the app last gave it and holds those
  links itself, starting each ssh master and forward as the app does now. This is MIP-4's decision
  4 (b), which MIP-4 left with two questions nobody had measured: whether a daemon started through
  Launch Services reaches the user's ssh agent, and what happens when ssh needs an interactive
  login. The stages under "How it lands" measure both before building it.

A daemon links only to the machines it is linked to directly. A client on the laptop sees the
laptop and every devenv, and a client on a devenv sees that devenv and the laptop but no other
devenv, until a second hop exists (card a_2YXDyGQIi).

**Muster adds no authentication.** A link is an ssh forward that the user's own ssh authenticated,
and the API never leaves the machine, so the API grants a program what the user's shell already
has. A program that reaches the network for Muster's agents, such as Foreman's server answering a
phone, authenticates its own clients.

### 6. The human, at the desk and away

The human is one participant with one place in each group, whichever program reads for them. A
question answered from a phone shows as answered in the banner and the conversation through
`answering`, as it does today. A client that reads the human's messages moves the human's place,
so the window does not ring for them again.

Muster sends nothing to a phone. It says when it rang the human and whether anyone was at a window,
and the client decides what to do about it.

### 7. The CLI

The CLI keeps working as it does. Its `--json` stays written for agents and scripts and gets no new
promise; a program that needs one uses the API. Quartermaster, Scribe and Lectern can move to the
API when their owners want to, and nothing makes them. A `muster` verb added later that reads or
acts on agents is added to the API in the same change, or the change says why not.

### 8. Doctrine and docs that change

- The non-goal in `AGENTS.md` becomes: "Not a multiplexer. Muster runs a session daemon of its own,
  which exists to serve Muster; its one interface to other programs is the API (MIP-10)."
- `docs/architecture.md` gains the `api` connection in the protocol section, and pane traffic on
  the link between daemons.
- `docs/cli/` gains an `api` topic, so `muster docs api` tells a client's author where the socket
  is and what each request does, and `muster docs api --schema` prints `muster_api.proto`, so a
  client in any language can generate its own bindings.

## Rationale

The CLI's reach is the API's scope because it is the boundary Muster has already drawn and keeps
documented: what a person or an agent may do to Muster from outside the window. A smaller, curated
API would make every new integration wait on a decision about whether it belongs.

The daemon serves the API because it holds the truth. The app owns none ("View = f(daemon
state)"), is often closed, and already hands the daemon every verb it can answer alone.

A schema of its own lets both sides move at their own speed. The internal protocol changes whenever
the app, the bridges or the CLI need something, and outside programs cannot follow that, nor can
Muster hold it still for them.

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

**Open the internal protocol to other programs.** No second schema to keep. It lost because Muster
would then have to hold still the protocol that the app, the bridges and the CLI change together,
or break outside tools with every minor. That protocol also carries pane bytes and input, which no
outside tool should drive.

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
- The daemon does more: it relays another machine's panes and, at stage 4, holds ssh links. A bug
  there now reaches outside clients as well as the window.
- The API's events are per state change and per post, never per byte of pane output, so a
  subscriber costs little against the "frequency times cardinality" budget.
- Every agent's messages become readable by any program running as the user, through one request.
  They already are, as files under `<install>.msg/`; the API makes it easy.
- Quartermaster, Scribe and Lectern keep working unchanged.

## How it lands

1. **Reads on one machine.** The `api` connection, `api.subscribe` with panes, machines and
   messages, resuming by post number, `api.read_pane`, and `muster docs api`. Enough for Foreman's
   server to show agents and messages without acting on them.
2. **Acting.** Posting and answering as the human, reading for them, the ring event, and making,
   typing into, closing, compacting and restarting panes.
3. **Machines, while a window holds the links.** Pane traffic and requests over the link the app
   already dials.
4. **Machines with no window.** Daemon-held links, once a spike has measured MIP-4's two questions
   for decision 4 (b) on macOS and Linux.
5. **Toward the CLI's reach.** The remaining verbs, window verbs carried through a window, a pane's
   output as a second kind of API connection, and `agent.report`. Each is a minor, in the order
   integrations ask for them.

## Decisions for amterp

**Question A. Whether the API has a schema of its own.**

- (a) A separate `muster_api.proto` with its own version (section 1).
- (b) A public subset of the internal protocol, marked as such, under the internal version.

Recommendation: (a). It is a second schema to keep, but (b) makes every internal minor a question
about outside clients, which is the coupling this MIP exists to remove.

**Question B. Which messages a client reads.**

- (a) Every group the daemon keeps or replicates (section 3).
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

- (a) `Alex (@human) via foreman`, with the client's name kept on the post (section 4).
- (b) `Alex (@human)`, the same as one typed at the window.

Recommendation: (a). An agent that is told something it did not expect can see where it came from,
and so can amterp reading the log later. It costs one field on a post.

**Question E. Whether the CLI's `--json` gets a stability promise as well.**

- (a) No: programs use the API, and the CLI's output stays free to change (section 7).
- (b) Yes, for the outputs Quartermaster and Scribe parse today.

Recommendation: (a). (b) holds still output written for agents, to spare two tools a migration
their owner can do when he chooses.

## Open Questions

- What a client in a sandbox that refuses sockets can do. It meets the same limits as `muster msg`
  (`muster docs limits`).
- How long a client may stay subscribed without reading before the daemon drops it, and what it is
  told when that happens.

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
