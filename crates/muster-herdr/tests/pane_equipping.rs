//! Whether a pane asked for with a name and a program comes up with both.
//!
//! `backend-intent.json` pins that neither travels in the split, because herdr's `pane.split`
//! takes neither. What it cannot say is that they arrive at all: the name is a second request,
//! and the program is a wait plus two more. That sequence is the whole feature - "make me a pane
//! running this" is what a script and an agent send - and it exists nowhere a corpus case can
//! see it.
//!
//! **What is asserted here is the outcome, not the wait.** A pty buffers, so a plain shell handed
//! input before its prompt appears runs it anyway - these tests pass with the wait taken out, and
//! that was checked rather than assumed. The wait is for a program that resets the terminal as it
//! starts and discards what is pending, which `sh` will not do on demand. Its mechanism is pinned
//! in `client_connection.rs`; that it is *needed* is a precaution nothing here demonstrates.
//!
//! What does need a real daemon is everything else: a second and third request going out at all,
//! the rename coming back on the outcome, and a plain split still costing one round trip.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use herdr_harness::{Daemon, until_some};
use muster_core::intent::{BackendChannel, BackendIntent, Side};
use muster_core::mirror::backend::PaneId;
use muster_core::names::{Mint, Names};
use muster_herdr::{HerdrBackend, PaneEnvironment};
use serde_json::json;

#[test]
fn a_pane_asked_for_with_a_name_and_a_program_gets_both() {
    let daemon = Daemon::start();
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names.clone());

    let outcome = backend
        .submit(&BackendIntent::CreateWorkspace { cwd: Some("/tmp".to_string()) })
        .expect("a daemon that answered ping can make a workspace");
    let first = outcome.created.expect("workspace.create answers with the pane it started");

    // A command whose having-run is a fact on disk rather than something read off a grid. A
    // screen wraps at its width and carries the shell's own echo of what was typed, so reading
    // one cannot distinguish "the command ran" from "the command is sitting at the prompt".
    let ran = PathBuf::from(format!("/tmp/muster-test/pane-equipping-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&ran);
    let outcome = backend
        .submit(&BackendIntent::SplitPane {
            pane: first.clone(),
            side: Side::Down,
            ratio: None,
            cwd: Some("/tmp".to_string()),
            run: Some(format!("printf 'ran' > {}", ran.display())),
            name: Some("🤖 A".to_string()),
        })
        .expect("a real daemon refused a split it can do");
    let made = outcome.created.clone().expect("pane.split answers with the pane it made");

    // The rename comes back on the outcome, and that is not a nicety: herdr announces a rename
    // to nobody (section 16), so a window that is not told here shows the pane unnamed for as
    // long as the connection lasts.
    assert_eq!(
        outcome.renamed,
        Some((made.clone(), Some("🤖 A".to_string()))),
        "the split reported no rename, so nothing above the adapter will ever learn the name it \
         asked for - herdr emits no event for one. The pane may well be named on the daemon; the \
         window would show it nameless."
    );
    assert_eq!(
        label(&daemon, &names, &made),
        Some("🤖 A".to_string()),
        "the daemon does not hold the name the split asked for, so a person running several \
         agents cannot tell this pane from the others"
    );

    // Polled rather than assumed-immediate: the split's answer comes back before the shell has
    // finished running what it was typed, and how long that takes is the machine's business.
    let written = until_some("the command to have run in the pane the split made", || {
        std::fs::read_to_string(&ran).ok().filter(|text| !text.is_empty())
    });
    let _ = std::fs::remove_file(&ran);
    assert_eq!(written, "ran", "something ran in that pane and it was not what was asked for");
}

/// A split that asks for neither leaves the pane as a split has always left it.
///
/// The other half, because the sequence above is three extra requests and a wait: a keybinding
/// sends no name and no program, and must not pay for either. A rename reported here would also
/// be a rename the mirror applies, clearing a label somebody else set.
#[test]
fn a_split_asking_for_nothing_sends_nothing_extra() {
    let daemon = Daemon::start();
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names.clone());

    let first = backend
        .submit(&BackendIntent::CreateWorkspace { cwd: Some("/tmp".to_string()) })
        .expect("a daemon that answered ping can make a workspace")
        .created
        .expect("workspace.create answers with the pane it started");

    let began = Instant::now();
    let outcome = backend
        .submit(&BackendIntent::SplitPane {
            pane: first,
            side: Side::Right,
            ratio: None,
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
        .expect("a real daemon refused a split it can do");
    let made = outcome.created.clone().expect("pane.split answers with the pane it made");

    assert_eq!(
        outcome.renamed, None,
        "a split that asked for no name reported one anyway, which the mirror would apply - so \
         this would clear whatever the pane was already called"
    );
    assert_eq!(
        label(&daemon, &names, &made),
        None,
        "the daemon holds a label for a pane nobody named it"
    );
    // Well under the readiness allowance, which is what says the wait was skipped rather than
    // satisfied quickly. A prompt on this machine appears in tens of milliseconds, so a second is
    // slack against a loaded runner and still a tenth of the allowance.
    assert!(
        began.elapsed() < Duration::from_secs(1),
        "a plain split took {:?}, which is long enough that it waited for something. A \
         keybinding sends no program and must not pay for the wait one needs.",
        began.elapsed()
    );
}

/// Whether Return is pressed is whether the text runs.
///
/// The corpus pins that both spellings build the identical `pane.send_text`, which is the whole of
/// what a request can say. What the flag does is a second request, and the difference it makes is
/// the difference between an agent being told something and an agent staring at a line somebody
/// typed for it - so it is only observable against a real shell.
#[test]
fn text_runs_when_it_asked_for_a_return_and_waits_when_it_did_not() {
    let daemon = Daemon::start();
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names.clone());

    let pane = backend
        .submit(&BackendIntent::CreateWorkspace { cwd: Some("/tmp".to_string()) })
        .expect("a daemon that answered ping can make a workspace")
        .created
        .expect("workspace.create answers with the pane it started");

    let held = PathBuf::from(format!("/tmp/muster-test/pane-held-{}.txt", std::process::id()));
    let sent = PathBuf::from(format!("/tmp/muster-test/pane-sent-{}.txt", std::process::id()));
    for path in [&held, &sent] {
        let _ = std::fs::remove_file(path);
    }

    backend
        .submit(&BackendIntent::SendText {
            pane: pane.clone(),
            text: format!("printf 'held' > {}", held.display()),
            enter: false,
        })
        .expect("a real daemon takes text for a pane it holds");
    // Then a second one that does submit. Its arrival is what makes the first one's absence
    // meaningful: without it this would be a race against a shell that had not got round to it.
    backend
        .submit(&BackendIntent::SendText {
            pane,
            text: format!("; printf 'sent' > {}", sent.display()),
            enter: true,
        })
        .expect("a real daemon takes text for a pane it holds");

    until_some("the submitted text to have run", || {
        std::fs::read_to_string(&sent).ok().filter(|text| !text.is_empty())
    });
    // Both lines ran, on one Return - which is the point. The first was still sitting on the
    // prompt when the second was typed after it, so a `held` written by its own Return would mean
    // the flag did nothing.
    assert_eq!(
        std::fs::read_to_string(&held).ok().as_deref(),
        Some("held"),
        "the two lines should have run together as one command line, and did not - so the first \
         was submitted on its own despite asking for no Return"
    );
    for path in [&held, &sent] {
        let _ = std::fs::remove_file(path);
    }
}

/// What the daemon itself calls a pane, asked of the daemon rather than of Muster's mirror.
///
/// The oracle has to be the daemon: what this is checking is that a request went out and took
/// effect, and Muster's own record of it comes from the same reply the assertion would be
/// reading.
fn label(daemon: &Daemon, names: &Names, pane: &PaneId) -> Option<String> {
    let backend = names.backend(pane).expect("a name Muster minted resolves");
    let got = daemon.call("pane.get", &json!({ "pane_id": backend.as_str() }));
    got["pane"]["label"].as_str().filter(|label| !label.is_empty()).map(str::to_string)
}
