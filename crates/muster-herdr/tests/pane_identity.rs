//! Whether a pane Muster made can say which pane it is.
//!
//! The one claim the whole naming registry exists to support, and the one no unit test can
//! reach. `pane-names.json` pins what a name is and `backend-intent.json` pins that a request
//! carries the daemon's id - but the thing an agent in a pane actually depends on is a variable
//! in its own process environment, and every step between the two is a step herdr takes.
//!
//! Three things have to line up for this to pass, and they are decided in three different
//! places: the name is minted *before* the request goes out (`HerdrBackend::submit`), it rides
//! the `env` map of the very request that makes the pane (`env::with_pane_name`), and it is
//! bound to the id the answer carries so the registry can translate it back. Getting any one of
//! them wrong leaves a pane holding a name for a pane that is not itself, which is worse than
//! holding none: a command from that pane would act on somebody else's work.

use std::path::PathBuf;

use herdr_harness::{Daemon, until_some};
use muster_core::intent::{BackendChannel, BackendIntent, Side};
use muster_core::mirror::backend::PaneId;
use muster_core::names::{Mint, Names};
use muster_herdr::{HerdrBackend, HerdrClient, PaneEnvironment};
use serde_json::json;

#[test]
fn a_pane_muster_made_is_told_which_pane_it_is() {
    let daemon = Daemon::start();
    // A minting registry rather than the harness's, which spells a name as the daemon's own id.
    // That is the right default for tests about everything else and exactly wrong here: it
    // would pass whether or not a single character of this mechanism worked.
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names.clone());

    // Through the intent rather than through a hand-built `workspace.create`, because what is
    // under test is the request Muster builds. A test that called the daemon itself would prove
    // that herdr honors an `env` map, which herdr's own suite already covers.
    let outcome = backend
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
        .expect("a daemon that answered ping can make a workspace");
    let made = outcome.created.expect("workspace.create answers with the pane it started");

    assert!(
        made.as_str().starts_with('p') && made.as_str().len() == 10,
        "the pane was named {made}, which is not a name this Muster mints.\n  Impact: whatever \
         is in the pane's environment is not something the registry can resolve, so every \
         command from inside it is refused.\n  Check names::spelling in muster-core."
    );

    // Written to a file rather than read off the pane's screen, for the reason
    // daemon_isolation.rs gives: a grid wraps at its width and carries the shell's own echo, so
    // reading one would be asserting on terminal rendering to answer a question about process
    // environment.
    let dump = PathBuf::from(format!("/tmp/muster-test/pane-identity-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&dump);
    let pane = names.backend_pane(&made).expect("the name Muster just minted resolves");
    daemon.call(
        "pane.send_text",
        &json!({
            "pane_id": pane.as_str(),
            "text": format!("printf '%s' \"$MUSTER_PANE\" > {}\n", dump.display()),
        }),
    );

    let written = until_some("the shell in the pane to write out what it calls itself", || {
        std::fs::read_to_string(&dump).ok().filter(|text| !text.is_empty())
    });
    let _ = std::fs::remove_file(&dump);

    assert_eq!(
        written.trim(),
        made.as_str(),
        "the pane calls itself {written:?} and Muster calls it {made}.\n  Impact: a program in \
         that pane cannot say which pane it is, so `muster pane new --below` from inside it \
         either fails or acts on the wrong pane.\n  Check that the name is minted before the \
         request in HerdrBackend::submit, that env::with_pane_name puts it in the `env` map, \
         and that herdr still passes `env` through to a pane's process."
    );
}

// A name is reserved before a pane-making request is built, because it has to ride in that
// request. Everything that can refuse between the reservation and the request going out has to
// give the name back, or each refusal strands one - invisibly, since nothing reads a reservation
// again. Neither case needs a daemon: both are refused before anything is sent.

#[test]
fn a_tab_whose_workspace_could_not_be_asked_for_gives_its_name_back() {
    // The backend mint, so the pane the tab goes beside resolves and the refusal is the one this
    // is about: the daemon could not be asked which workspace holds it.
    let names = Names::alone("local", Mint::Backend);
    let backend = HerdrBackend::new(nobody_listening(), PaneEnvironment::none(), names.clone());

    let refused = backend.submit(&BackendIntent::CreateTab {
        beside: PaneId::new("w1:p1"),
        cwd: None,
        run: None,
        name: None,
    });

    assert!(refused.is_err(), "a tab was made with no daemon to make it: {refused:?}");
    assert_eq!(
        names.reservations(),
        0,
        "a tab refused before it was asked for kept the name minted for its pane.\n  Impact: every \
         ⌘T that cannot find its workspace strands one name for a pane that never existed.\n  \
         Check that HerdrBackend::submit gives the name back on every early return."
    );
}

#[test]
fn a_split_of_a_pane_nothing_answers_to_gives_its_name_back() {
    // A minting registry, under which a name nobody bound resolves to nothing, so `request`
    // refuses to build the split at all.
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(nobody_listening(), PaneEnvironment::none(), names.clone());

    let refused = backend.submit(&BackendIntent::SplitPane {
        pane: PaneId::new("p00000gone"),
        side: Side::Right,
        ratio: None,
        cwd: None,
        run: None,
        name: None,
    });

    assert!(refused.is_err(), "a split of a pane nothing answers to was sent: {refused:?}");
    assert_eq!(
        names.reservations(),
        0,
        "a split refused before it was sent kept the name minted for its pane.\n  Impact: one \
         stranded name per refusal, for a pane that never existed.\n  Check that \
         HerdrBackend::submit gives the name back on every early return."
    );
}

/// A client for a socket nobody is listening on, so every request is unreachable.
fn nobody_listening() -> HerdrClient {
    HerdrClient::new(format!("/tmp/muster-test/nobody-listening-{}.sock", std::process::id()))
}
