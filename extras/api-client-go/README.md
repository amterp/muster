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

The client says hello as API 0.7, subscribes to the available topics, and prints any session
transcript provenance in the snapshot. It makes a pane with `pane.new` and reads the pane's
text with `api.read_pane`. When the daemon advertises recipient outcomes, the client posts a
message to the pane it made and prints the initial routing result.

Run on its own, `./api-client-go [socket]` uses `$MUSTER_DAEMON_SOCKET` by default. It leaves its
pane and, on API 0.7, its message on that daemon. The recorded run in `transcript.txt` used a
scratch daemon. The IDs, callsigns, and epochs differ in every run.
