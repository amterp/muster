//! A pane nobody can see costs its daemon nothing to render, and loses nothing for it.
//!
//! herdr renders every attached client on each pass, on the thread that also answers requests,
//! so a window that keeps a client on every pane it has ever shown pays for panes behind other
//! tabs: measured at 0.135 of a core with fourteen panes attached against 0.030 with the two on
//! screen, twelve of them printing (`tools/latency.py`). So a hidden pane's bridge lets go of its
//! herdr client and keeps everything else - its surface, its socket to the app, and the pane's
//! size, which herdr holds after a client leaves (`observations/herdr-0.8.0.md` section 4).
//!
//! What must not change is what the pane can still be asked for. Showing it again brings the
//! stream back, and sending to it while it is hidden - `muster pane send` to a pane behind
//! another tab is ordinary - still reaches the program, without bringing the stream back: that
//! goes to the daemon by name rather than through the bridge.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use muster::proto::{CreateTab, FocusTabRelative, SendToPane, request};
use support::{Press, Typing, answer, assert_ok, named_pane, until};

#[test]
fn a_hidden_pane_lets_go_of_its_stream_and_takes_it_back() {
    let typing = Typing::start("");
    typing.run("cat", "cat");
    assert_eq!(typing.bridge.herdr_clients(), 1, "a pane on screen streams from one client");

    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the pane behind the new tab to let go of its herdr client",
        || typing.bridge.herdr_clients() == 0,
        || typing.bridge.diagnosis("herdr goes on rendering a pane nobody can see"),
    );

    assert_ok(&answer(request::Payload::FocusTabRelative(FocusTabRelative {
        direction: "previous".to_string(),
    })));
    until(
        "the pane shown again to stream again",
        || typing.bridge.herdr_clients() == 1,
        || typing.bridge.diagnosis("a pane switched back to shows its last screen forever"),
    );
    Press::new("KeyB", "b").send();
    typing.expect_on_screen("b", "a pane shown again does not take what is typed into it");

    // Hidden again, and sent to while it is.
    let name = named_pane(&typing.pane);
    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the pane to let go of its client once it is hidden again",
        || typing.bridge.herdr_clients() == 0,
        || typing.bridge.diagnosis("herdr goes on rendering a pane nobody can see"),
    );
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: name,
        text: "c".to_string(),
        ..SendToPane::default()
    })));
    assert_eq!(
        typing.bridge.herdr_clients(),
        0,
        "a send by name goes to the daemon, so it has no reason to bring the stream back"
    );

    // And it was there all along, which showing the pane again paints. Two steps back, past the
    // tab made first.
    for _ in 0..2 {
        assert_ok(&answer(request::Payload::FocusTabRelative(FocusTabRelative {
            direction: "previous".to_string(),
        })));
    }
    typing.expect_on_screen("bc", "what was sent to a hidden pane never reached its program");
}
