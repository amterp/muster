# A Go client of Muster's API

A program that reads and acts on Muster through the daemon's API, written against bindings
generated from `muster docs api --schema` and nothing else of Muster's. It is the proof MIP-10's
stage 1 asks for, that a client in another language can be built from what Muster publishes, and a
starting point for writing one. Nothing in Muster's gate builds or runs it: the gate installs no Go
and reaches no network.

    ./try

does all of it against this checkout's build, which `./dev -b` makes: it writes the schema with
`muster docs api --schema`, generates `musterapi/` from it with `protoc` and `protoc-gen-go`,
starts a muster-daemon on a socket of its own with a scratch home, runs the client, and stops the
daemon. It needs Go, `protoc` and `protoc-gen-go`, and the network once, for Go to fetch
`google.golang.org/protobuf`. `MUSTER_BINARY` and `MUSTER_DAEMON_BINARY` name other builds.

The client connects, says hello as API 0.2, subscribes to every topic, makes a pane with
`pane.new` that prints a line, and reads that line back with `api.read_pane`, printing what the
daemon said at each step. Run on its own, `./api-client-go [socket]` talks to any daemon,
`$MUSTER_DAEMON_SOCKET`'s by default, and leaves the pane it made running there.

`transcript.txt` is a run, with the versions it ran on. What it showed: the Welcome listed the
requests and topics the daemon serves; the pane's appearing reached the client as event 1,
carrying the correlation id of the request that made it, before that request's answer, whose
`through` was 1; and the pane's text came back as the command printed it. The daemon's ids,
callsigns and epochs differ in every run.
