//! What a caller outside the window can learn about what its agents are doing, and when.
//!
//! A snapshot answering `working` twice cannot say whether that was one turn or two with a finish
//! between them, and a caller that wants to know when an agent finishes has had nothing to do but
//! poll (kan a_2M9T8O6dL). These tests are against a real daemon, which is where agent states
//! come from, and they drive states the way a real agent does: a fake agent in the pane paints
//! them, and the daemon's detection reads them off its screen.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    BackendHealth, ClosePane, OpenWindow, PaneStateChanged, ReadWindow, Request, Response,
    SplitPane, Startup, WatchPanes, Window, WindowFocus, request, response,
};
use muster_daemon_proto::AgentState;
use muster_harness::requests::{create, expect, in_new_tab, make, pane, snapshot};
use muster_harness::{Daemon, PATIENCE, until, until_some};
use prost::Message;

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
    open.report(AgentState::Working);
    let working = until_state(&open, "working");
    let after = now_ms();
    assert!(
        (before..=after).contains(&working.since_ms),
        "the agent started working between {before} and {after}, and the window says it has been \
         working since {} - a caller subtracting that from now gets a turn of the wrong length",
        working.since_ms
    );

    // Nothing has told the window it has focus, so a finish nobody saw is `done`.
    open.report(AgentState::Idle);
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

/// A finish nobody saw outlives the window, and a window that shows it clears it for every window.
///
/// The daemon keeps the finish on the pane's record, because it is the one that outlives the app,
/// and quitting and coming back is the ordinary case: a window opened after the agent finished
/// still says `done`. Only a window can say somebody looked, so it tells the daemon, and the
/// daemon's record is what every other window reads.
#[test]
fn a_finish_waits_in_the_daemon_until_a_window_with_the_keyboard_shows_it() {
    let turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    open.report(AgentState::Working);
    until_state(&open, "working");
    open.report(AgentState::Idle);
    until_state(&open, "done");
    assert!(
        finished_unseen(&open),
        "the window says `done` and the daemon holds no finish, so a window opened after this one \
         would say `idle`"
    );

    turn.relaunch();
    open_the_window(&open.daemon, &open.socket);
    until_state(&open, "done");
    assert!(finished_unseen(&open), "a window that has not had the keyboard cleared the finish");

    assert_ok(&dispatch(request::Payload::WindowFocus(WindowFocus { focused: true })));
    until_state(&open, "idle");
    until(
        "the daemon to clear the finish the window saw",
        || !finished_unseen(&open),
        || "the daemon still holds the finish, so every other window still says `done`".to_string(),
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

    open.report(AgentState::Working);
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

    open.report(AgentState::Idle);
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

    open.report(AgentState::Working);
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
    open.report(AgentState::Blocked);
    open.report(AgentState::Idle);
    let finished = state_frame(&mut finish);
    assert_eq!(
        finished.state, "done",
        "a wait for `idle` answered with {finished:?}. Blocked is not finished and must not be \
         sent, and `done` is an idle nobody has looked at - so this window, unfocused, owes `done`"
    );
    assert_ended(&mut finish);
}

/// What an agent says about itself reaches the window with its state, and an idle agent that said
/// it is waiting on its own work reads `waiting`. A wait for `idle` does not end there, since the
/// agent has not finished: it ends when a later turn finishes without the wait.
#[test]
fn an_agents_own_word_reaches_the_window_and_a_wait_for_idle_outlasts_waiting() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    open.report(AgentState::Working);
    until_state(&open, "working");
    open.say(muster_daemon_proto::pane_request::Report {
        context_used: Some(64.0),
        waiting: Some("the full gate".to_string()),
        ..Default::default()
    });
    open.report(AgentState::Idle);

    let waiting = until_state(&open, "waiting");
    let facts = waiting.facts.expect("the agent's facts");
    assert_eq!(facts.context_used, Some(64.0));
    assert_eq!(facts.waiting, "the full gate");
    assert!(!waiting.reported, "read off the screen, not reported");
    assert!(!waiting.unreadable);

    let mut finish = watching(
        &open.socket,
        WatchPanes { pane_ids: vec![open.pane.clone()], until: vec!["idle".to_string()] },
    );
    until(
        "the window to hold the wait open",
        || muster::testing::watchers() == 1,
        || format!("{} watches are open", muster::testing::watchers()),
    );
    open.report(AgentState::Working);
    open.report(AgentState::Idle);
    let finished = state_frame(&mut finish);
    assert_eq!(finished.state, "done", "answered with {finished:?}, not the finish after the wait");
    assert_eq!(finished.facts.map(|facts| facts.waiting), Some(String::new()));
    assert_ended(&mut finish);
}

/// A wait on a pane that closes is refused, rather than left waiting on something that is gone.
#[test]
fn a_wait_on_a_pane_that_closes_is_refused() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    // Straight after the split, the way `P=$(muster pane new); muster pane wait --pane "$P"`
    // runs.
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

/// A watch hears a daemon stop answering, and hears it come back, once each.
///
/// While a daemon is stale nothing about its panes reaches the window, so a watch that said
/// nothing would read as agents that had all gone quiet at once. Restarted rather than only killed
/// so that it does come back - and the follower says a daemon is stale on every reconnect it
/// tries, which a caller should hear as one change each way.
#[test]
fn a_watch_hears_a_daemon_stop_answering_and_come_back() {
    let _turn = muster::testing::fresh_session();
    let mut open = a_window_onto_one_pane();
    let mut watch = watching(&open.socket, WatchPanes::default());
    let daemon = state_frame(&mut watch).daemon_id;

    open.daemon.kill();
    open.daemon.restart();
    let gone = health_frame(&mut watch);
    assert_eq!(
        (gone.daemon_id.as_str(), gone.state.as_str()),
        (daemon.as_str(), "stale"),
        "a daemon that stopped answering has to be said to have, by name, or a watch on its panes \
         reads as agents that went quiet: {gone:?}"
    );
    let back = health_frame(&mut watch);
    assert_eq!(
        (back.daemon_id.as_str(), back.state.as_str()),
        (daemon.as_str(), "connected"),
        "after `stale`, the next thing a watch says about the daemon has to be that it is back - \
         anything else is one change heard twice: {back:?}"
    );
}

/// A wait on a pane whose daemon stops answering ends unanswered, rather than waiting on.
///
/// Nothing about the pane can reach the window while its daemon is stale, so the wait could only
/// run out its timeout and exit as though the agent were still busy. Whether the pane got there
/// is unknown, and that is what a wait started after the daemon went answers at once too. A watch
/// started then says so before any pane, because every state it sends for that daemon is a guess.
#[test]
fn a_wait_on_a_pane_whose_daemon_stops_answering_is_unanswered() {
    let _turn = muster::testing::fresh_session();
    let mut open = a_window_onto_one_pane();
    let blocked =
        || WatchPanes { pane_ids: vec![open.pane.clone()], until: vec!["blocked".into()] };

    // Blocked, because a plain shell never is, so nothing but the daemon going can end this.
    let mut waiting = watching(&open.socket, blocked());
    until(
        "the window to hold the wait open",
        || muster::testing::watchers() == 1,
        || format!("{} watches are open", muster::testing::watchers()),
    );
    open.daemon.kill();
    match frame(&mut waiting).payload {
        Some(response::Payload::Unanswered(unanswered))
            if unanswered.reason.contains(&open.pane) => {}
        other => panic!(
            "a wait on a pane whose daemon stopped answering has to end as unanswered, naming the \
             pane, rather than sit out its timeout and read as an agent still working. Got {other:?}"
        ),
    }
    assert!(
        read_frame(&mut waiting, LARGEST_MESSAGE).is_err(),
        "a wait kept its connection open after its last answer"
    );

    let mut late = watching(&open.socket, blocked());
    match frame(&mut late).payload {
        Some(response::Payload::Unanswered(unanswered))
            if unanswered.reason.contains(&open.pane) => {}
        other => panic!(
            "a wait started while its pane's daemon is stale has nothing to hear, so it has to end \
             at once as unanswered. Got {other:?}"
        ),
    }

    let mut watch = watching(&open.socket, WatchPanes::default());
    match frame(&mut watch).payload {
        // Stale or disconnected: a daemon killed before the window's subscription took its own
        // first snapshot is written down as never having been reached.
        Some(response::Payload::BackendHealth(health)) if health.state != "connected" => {}
        other => panic!(
            "a watch started while a daemon is stale has to say so before any pane, since every \
             state it sends for that daemon's panes is a guess. Got {other:?}"
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

    // The refusal for a typo lists the states there are, so it has to list every one of them.
    let typo = WatchPanes { pane_ids: vec![open.pane.clone()], until: vec!["idel".to_string()] };
    let Some(response::Payload::Failure(failure)) =
        frame(&mut watching(&open.socket, typo)).payload
    else {
        panic!("a watch until `idel` was not refused");
    };
    for state in ["working", "blocked", "waiting", "idle", "done", "unknown"] {
        assert!(
            failure.reason.contains(state),
            "the refusal of a mistyped state leaves out `{state}`: {}",
            failure.reason
        );
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

/// One window, showing one pane with an agent in it.
struct Open {
    daemon: Daemon,
    socket: PathBuf,
    /// The pane's name, which the daemon and the window both know it by.
    pane: String,
}

impl Open {
    /// Has the pane's agent paint `state`, and waits until the daemon has read it.
    fn report(&self, state: AgentState) {
        self.daemon.set_agent_state(&self.pane, state);
    }

    /// Has the pane's agent say something about itself, as its hooks and statusline do.
    fn say(&self, mut report: muster_daemon_proto::pane_request::Report) {
        report.pane.clone_from(&self.pane);
        let request = pane(muster_daemon_proto::pane_request::Request::Report(report));
        expect(&mut self.daemon.connect(), request, muster_daemon_proto::Outcome::Done);
    }
}

fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start_detecting();
    let pane = "p1".to_string();
    make(&mut daemon.connect(), create(&pane, in_new_tab("t1")));
    // Before the window opens, so the agent is already idle when it is first seen and the only
    // state changes a test hears are the ones it makes.
    daemon.run_agent(&pane);

    let socket = daemon.root().join("command.sock");
    open_the_window(&daemon, &socket);

    until(
        "the window to say the pane's agent is idle",
        || {
            read_window(&socket)
                .panes
                .iter()
                .any(|agent| agent.pane_id == pane && agent.state == "idle")
        },
        || format!("the window reads {:?}", read_window(&socket)),
    );
    Open { daemon, socket, pane }
}

fn open_the_window(daemon: &Daemon, socket: &std::path::Path) {
    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow {})));
}

/// Whether the pane's daemon holds a finish nobody has seen, asked of the daemon itself.
fn finished_unseen(open: &Open) -> bool {
    snapshot(&mut open.daemon.connect())
        .panes
        .into_iter()
        .find(|record| record.pane == open.pane)
        .is_some_and(|record| record.finished_unseen)
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

/// The next thing a watch says about a daemon, passing over what it says about panes meanwhile.
///
/// A daemon that restarts may announce its panes again on the way back, and that is not what a
/// test about its health is asserting.
fn health_frame(stream: &mut UnixStream) -> BackendHealth {
    loop {
        match frame(stream).payload {
            Some(response::Payload::BackendHealth(health)) => return health,
            Some(response::Payload::PaneState(_) | response::Payload::PaneClosed(_)) => {}
            other => panic!("a watch sent {other:?} where a daemon's health was due"),
        }
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
