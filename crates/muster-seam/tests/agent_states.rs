//! What a caller outside the window can learn about what its agents are doing, and when.
//!
//! A snapshot answering `working` twice cannot say whether that was one turn or two with a finish
//! between them, and a caller that wants to know when an agent finishes has had nothing to do but
//! poll (kan a_2M9T8O6dL). These tests are against a real daemon, which is where agent states
//! come from, and they drive states through herdr's own `pane.report_agent`.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use herdr_harness::{Daemon, PATIENCE, until, until_some};
use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    ClosePane, OpenWindow, PaneStateChanged, ReadWindow, Request, Response, SplitPane, Startup,
    WatchPanes, Window, WindowFocus, request, response,
};
use prost::Message;
use serde_json::{Value, json};

/// A pane's agent says since when it has been doing what it is doing, and a look does not move it.
///
/// The clock belongs to the agent rather than to the window. A `done` pane somebody looks at
/// becomes `idle`, and it has been resting since it finished rather than since it was noticed -
/// so an integrator reading how long a worker has been idle reads how long its work has been
/// waiting.
#[test]
fn a_pane_says_since_when_its_agent_has_been_doing_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    let first_seen = agent(&open.socket, &open.pane).since_ms;
    assert!(
        first_seen > 0,
        "a pane the window has seen carries no time at all, so a caller cannot say how long it \
         has been in its state: {:?}",
        agent(&open.socket, &open.pane)
    );

    let before = now_ms();
    open.report("working");
    let working = until_state(&open, "working");
    let after = now_ms();
    assert!(
        (before..=after).contains(&working.since_ms),
        "the agent started working between {before} and {after}, and the window says it has been \
         working since {} - a caller subtracting that from now gets a turn of the wrong length",
        working.since_ms
    );

    // Nothing has told the window it has focus, so a finish nobody saw is `done`.
    open.report("idle");
    let finished = until_state(&open, "done");
    assert!(
        finished.since_ms >= working.since_ms,
        "the agent finished at {} and started at {}, so the window's clock ran backwards",
        finished.since_ms,
        working.since_ms
    );

    assert_ok(&dispatch(request::Payload::WindowFocus(WindowFocus { focused: true })));
    let seen = until_state(&open, "idle");
    assert_eq!(
        seen.since_ms, finished.since_ms,
        "looking at a finished pane restarted how long it has been resting, so an idle worker \
         reads as having finished the moment somebody glanced at it"
    );
}

/// A watch starts from every pane as it stands, then hears each change once, in order.
///
/// The shape an integrator waiting on several workers wants: one connection, and a line for every
/// agent that starts, stops, or goes. Asserted frame by frame, so a change sent twice or a state
/// skipped fails here rather than reading as a slow agent.
#[test]
fn a_watch_starts_from_every_pane_and_hears_each_change() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let mut watch = watching(&open.socket, WatchPanes::default());

    let first = state_frame(&mut watch);
    assert_eq!(
        (first.pane_id.as_str(), first.since_ms > 0),
        (open.pane.as_str(), true),
        "a watch has to begin with every pane as it stands, time included - otherwise a caller \
         starting one cannot tell what it is waiting on: {first:?}"
    );

    open.report("working");
    let working = state_frame(&mut watch);
    assert_eq!(
        working.state, "working",
        "the first change heard was not the one made: {working:?}"
    );
    assert_eq!(
        working.since_ms,
        agent(&open.socket, &open.pane).since_ms,
        "the watch and a read disagree about when the agent started, so they are two accounts of \
         one pane"
    );

    open.report("idle");
    let finished = state_frame(&mut watch);
    assert_eq!(
        finished.state, "done",
        "after `working`, the next thing a watch says has to be the finish - anything else is a \
         change heard twice or one the caller never made: {finished:?}"
    );

    assert_ok(&dispatch(request::Payload::WindowFocus(WindowFocus { focused: true })));
    let seen = state_frame(&mut watch);
    assert_eq!(
        (seen.state.as_str(), seen.since_ms),
        ("idle", finished.since_ms),
        "somebody looking at a finished pane is a change a watch should hear, and it does not \
         restart the agent's clock: {seen:?}"
    );

    let made = split(&open);
    let appeared = state_frame(&mut watch);
    assert_eq!(appeared.pane_id, made, "a pane made after the watch began went unannounced");
    assert_ok(&dialed(
        &open.socket,
        request::Payload::ClosePane(ClosePane { pane_id: made.clone(), ..ClosePane::default() }),
    ));
    match frame(&mut watch).payload {
        Some(response::Payload::PaneClosed(closed)) if closed.pane_id == made => {}
        other => panic!(
            "a watched pane that closes has to be said to have closed, or a caller waiting on it \
             waits on nothing. Got {other:?}"
        ),
    }
}

/// A wait is a condition: a pane already there answers at once, and otherwise the first change
/// that gets there does - and nothing short of it is sent.
#[test]
fn a_wait_ends_when_a_pane_gets_where_it_was_asked_to() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    let now = agent(&open.socket, &open.pane);
    let mut already = watching(
        &open.socket,
        WatchPanes { pane_ids: vec![open.pane.clone()], until: vec![now.state.clone()] },
    );
    assert_eq!(
        state_frame(&mut already).state,
        now.state,
        "a pane already in the state asked for has to answer at once, or a caller that asked a \
         moment too late waits past something that already happened"
    );
    assert_ended(&mut already);

    open.report("working");
    until_state(&open, "working");
    let mut finish = watching(
        &open.socket,
        WatchPanes { pane_ids: vec![open.pane.clone()], until: vec!["idle".to_string()] },
    );
    // Registered before the agent moves, so what answers is the change and not a later picture.
    until(
        "the window to hold the wait open",
        || muster::testing::watchers() == 1,
        || format!("{} watches are open", muster::testing::watchers()),
    );
    open.report("blocked");
    open.report("idle");
    let finished = state_frame(&mut finish);
    assert_eq!(
        finished.state, "done",
        "a wait for `idle` answered with {finished:?}. Blocked is not finished and must not be \
         sent, and `done` is an idle nobody has looked at - so this window, unfocused, owes `done`"
    );
    assert_ended(&mut finish);
}

/// A wait on a pane that closes is refused, rather than left waiting on something that is gone.
#[test]
fn a_wait_on_a_pane_that_closes_is_refused() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    // Straight after the split, the way `P=$(muster pane new); muster pane wait --pane "$P"`
    // runs: the split answers with the name before the window's mirror has heard of the pane.
    let made = split(&open);

    // Blocked, because a plain shell never is, so nothing but the close can end this.
    let mut waiting = watching(
        &open.socket,
        WatchPanes { pane_ids: vec![made.clone()], until: vec!["blocked".to_string()] },
    );
    until(
        "the window to hold the wait open",
        || muster::testing::watchers() == 1,
        || format!("{} watches are open", muster::testing::watchers()),
    );
    assert_ok(&dialed(
        &open.socket,
        request::Payload::ClosePane(ClosePane { pane_id: made.clone(), ..ClosePane::default() }),
    ));
    match frame(&mut waiting).payload {
        Some(response::Payload::Failure(failure)) if failure.reason.contains(&made) => {}
        other => panic!(
            "a wait on a pane that closed has to be refused, naming the pane, so a caller does not \
             sit out its whole timeout on nothing. Got {other:?}"
        ),
    }
}

/// A watch that could never answer is refused before anything is watched.
#[test]
fn a_watch_on_nothing_real_is_refused() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    let refusals = [
        (WatchPanes { pane_ids: vec!["p1nobody00".to_string()], until: vec![] }, "p1nobody00"),
        (WatchPanes { pane_ids: vec![open.pane.clone()], until: vec!["idel".to_string()] }, "idel"),
    ];
    for (asked, named) in refusals {
        let mut refused = watching(&open.socket, asked);
        match frame(&mut refused).payload {
            Some(response::Payload::Failure(failure)) if failure.reason.contains(named) => {}
            other => panic!(
                "a watch naming {named:?} can never be answered and has to be refused, naming it. \
                 A typo in a state would otherwise wait for a shell. Got {other:?}"
            ),
        }
    }

    // Through the C ABI there is one answer to give, so a watch is refused there too.
    let through_the_shell = dispatch(request::Payload::WatchPanes(WatchPanes::default()));
    assert!(
        matches!(through_the_shell.payload, Some(response::Payload::Failure(_))),
        "a dispatched watch answered as though something were being watched: {through_the_shell:?}"
    );
}

/// A caller that hangs up is let go, rather than holding a thread until its pane next changes.
#[test]
fn a_caller_that_hangs_up_is_let_go() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    let mut watch = watching(&open.socket, WatchPanes::default());
    state_frame(&mut watch);
    assert_eq!(muster::testing::watchers(), 1, "a watch that is answering is not registered");

    drop(watch);
    until(
        "the window to let go of a watch whose caller hung up",
        || muster::testing::watchers() == 0,
        || format!("{} watches are still open", muster::testing::watchers()),
    );
}

/// One window, showing one pane, whose herdr and Muster names are both known.
struct Open {
    daemon: Daemon,
    socket: PathBuf,
    /// What Muster calls the pane, which is what every request addresses.
    pane: String,
    /// What the daemon calls it, which is what `pane.report_agent` addresses.
    backend: String,
}

impl Open {
    /// Tells the daemon the pane's agent is in `state`, the way a harness hook would.
    fn report(&self, state: &str) {
        self.daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": self.backend, "agent": "probe", "source": "probe", "state": state }),
        );
    }
}

fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "agents", "focus": true }));
    let backend = only_pane(&daemon);
    // Named before startup so this test can find it again: Muster mints its own name for the
    // pane, and herdr announces a rename to nobody, so the bootstrap snapshot is the only thing
    // that carries one (`observations/herdr-0.8.0.md` section 16).
    daemon.call("pane.rename", &json!({ "pane_id": backend, "label": "agent" }));

    let socket = daemon.root().join("command.sock");
    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    let pane = until_some("the window to list the pane called agent", || {
        read_window(&socket)
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .flat_map(|tab| tab.panes.iter())
            .find(|pane| pane.given_name == "agent")
            .map(|pane| pane.pane_id.clone())
    });
    Open { daemon, socket, pane, backend }
}

/// Opens a watch, and hands back the connection its answers arrive on.
fn watching(socket: &std::path::Path, asked: WatchPanes) -> UnixStream {
    let mut stream = UnixStream::connect(socket)
        .unwrap_or_else(|error| panic!("nothing is listening on {}: {error}", socket.display()));
    // The harness's own deadline, because every frame here is waiting on a condition the test
    // has already made true.
    stream.set_read_timeout(Some(PATIENCE)).expect("a unix socket takes a read timeout");
    write_frame(
        &mut stream,
        &Request { payload: Some(request::Payload::WatchPanes(asked)) }.encode_to_vec(),
    )
    .expect("the endpoint takes a watch");
    stream
}

fn frame(stream: &mut UnixStream) -> Response {
    let bytes = read_frame(stream, LARGEST_MESSAGE)
        .unwrap_or_else(|detail| panic!("the watch sent nothing more: {detail}"));
    Response::decode(bytes.as_slice()).expect("a watch answers with responses this build knows")
}

fn state_frame(stream: &mut UnixStream) -> PaneStateChanged {
    match frame(stream).payload {
        Some(response::Payload::PaneState(state)) => state,
        other => panic!("a watch sent {other:?} where a pane's state was due"),
    }
}

/// The watch said it is finished, and then hung up.
fn assert_ended(stream: &mut UnixStream) {
    let last = frame(stream);
    assert!(
        matches!(last.payload, Some(response::Payload::Ok(_))),
        "a wait that got what it asked for has to say so and stop, and sent {last:?}"
    );
    assert!(
        read_frame(stream, LARGEST_MESSAGE).is_err(),
        "a wait kept its connection open after its last answer"
    );
}

/// Splits the pane, and hands back what Muster calls the one that appears.
fn split(open: &Open) -> String {
    match dialed(
        &open.socket,
        request::Payload::SplitPane(SplitPane {
            pane_id: open.pane.clone(),
            side: "down".to_string(),
            ..SplitPane::default()
        }),
    )
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("a split answered with {other:?} rather than the pane it made"),
    }
}

/// What the window says one pane's agent is doing, asked over the socket a CLI dials.
fn agent(socket: &std::path::Path, pane: &str) -> PaneStateChanged {
    let window = read_window(socket);
    window
        .panes
        .iter()
        .find(|agent| agent.pane_id == pane)
        .cloned()
        .unwrap_or_else(|| panic!("the window lists no agent for {pane}: {window:?}"))
}

fn until_state(open: &Open, state: &str) -> PaneStateChanged {
    until_some(&format!("the window to say the agent is {state}"), || {
        Some(agent(&open.socket, &open.pane)).filter(|agent| agent.state == state)
    })
}

fn read_window(socket: &std::path::Path) -> Window {
    match dialed(socket, request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the endpoint answered a ReadWindow with {other:?}"),
    }
}

/// One request over one connection, and its one answer.
fn dialed(socket: &std::path::Path, payload: request::Payload) -> Response {
    let mut stream = UnixStream::connect(socket)
        .unwrap_or_else(|error| panic!("nothing is listening on {}: {error}", socket.display()));
    write_frame(&mut stream, &Request { payload: Some(payload) }.encode_to_vec())
        .expect("the endpoint takes a request");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the endpoint answers it");
    Response::decode(reply.as_slice()).expect("the answer is a response this build knows")
}

fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes: Vec<String> = snapshot["snapshot"]["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].clone()
}

fn now_ms() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970");
    i64::try_from(since.as_millis()).expect("milliseconds since 1970 fit an i64")
}

fn dispatch(payload: request::Payload) -> Response {
    let reply = muster::dispatch(&Request { payload: Some(payload) }.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
