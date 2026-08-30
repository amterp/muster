//! Whether a pane asked for with a name and a program comes up with both.
//!
//! `backend-intent.json` pins that neither travels in the split, because herdr's `pane.split`
//! takes neither. What it cannot say is that they arrive at all: the name is a second request,
//! and the program is a wait plus two more. That sequence is the whole feature - "make me a pane
//! running this" is what a script and an agent send - and it exists nowhere a corpus case can
//! see it.
//!
//! **The wait before the text is asserted separately, and against a different program.** A pty
//! buffers, so a plain shell handed input before its prompt appears runs it anyway - the three
//! tests below pass with the wait taken out, which is why nothing showed it mattered for a
//! release. What it is for is a program that resets the terminal as it starts and discards what
//! is pending, and `sh` will not do that on demand. The last test runs its panes on a program
//! that does, and it is the one that fails when the wait comes out.
//!
//! What the rest need a real daemon for is everything else: a second and third request going out
//! at all, the rename coming back on the outcome, and a plain split still costing one round trip.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use herdr_harness::{Daemon, until, until_file, until_some};
use muster_core::intent::{BackendChannel, BackendIntent, Side};
use muster_core::mirror::backend::PaneId;
use muster_core::names::{Mint, Names};
use muster_herdr::{HerdrBackend, HerdrClient, PaneEnvironment};
use serde_json::json;

#[test]
fn a_pane_asked_for_with_a_name_and_a_program_gets_both() {
    let daemon = Daemon::start();
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names.clone());

    let outcome = backend
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
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
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
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
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
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

/// What the fixture prints once it is reading, and the only reliable sign that it is.
const PROMPT: &str = "fixture-is-ready>";

/// The pane a program was asked for gets the program, even when the program throws away
/// whatever was typed before it was ready.
///
/// The hazard the readiness wait exists for, and the reason it could not be demonstrated
/// before: `sh` will not discard pending input on demand, so every other test here passes with
/// the wait taken out. A program that takes the terminal into raw mode does discard it, because
/// `tcsetattr` with `TCSAFLUSH` throws away input that arrived and has not been read - and
/// taking the terminal in hand at startup is the first thing a full-screen agent harness
/// does. So this runs the panes on one.
///
/// Two panes and one program, because either half alone proves nothing. The pane Muster
/// equipped gets its command; the pane handed the same text before the program was ready
/// does not, and then gets it once the program is ready - which is what says the pane was
/// working and the text was thrown away, rather than the pane being broken.
///
/// Delete the wait in `HerdrBackend::start` and the first assertion fails.
#[test]
fn a_program_that_discards_what_it_was_typed_early_still_gets_what_muster_asked_for() {
    let handed = scratch("equipping-readiness");
    let daemon = Daemon::start_running(&fixture(&handed).to_string_lossy());
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names.clone());

    let first = backend
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
        .expect("a daemon that answered ping can make a workspace")
        .created
        .expect("workspace.create answers with the pane it started");

    // Muster's own path: the split, the wait, the text, the Return.
    backend
        .submit(&BackendIntent::SplitPane {
            pane: first.clone(),
            side: Side::Down,
            ratio: None,
            cwd: Some("/tmp".to_string()),
            run: Some("typed-after-the-wait".to_string()),
            name: None,
        })
        .expect("a real daemon refused a split it can do");

    // The same program, handed text with no wait in front of it. A plain split asks for no
    // program, so nothing waits, and this is sent by hand the instant the split answers -
    // which is the arrangement `pane new --run` would have if the wait came out.
    let raced = backend
        .submit(&BackendIntent::SplitPane {
            pane: first,
            side: Side::Right,
            ratio: None,
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
        .expect("a real daemon refused a split it can do")
        .created
        .expect("pane.split answers with the pane it made");
    let raced = names.backend_pane(&raced).expect("a name Muster minted resolves");
    type_into(&daemon, raced.as_str(), "typed-before-it-was-ready");

    until(
        "the command Muster was asked to run to reach the pane it made",
        || handed.join("typed-after-the-wait").exists(),
        || {
            "the pane Muster equipped never received its command, against a program that \
             resets its terminal as it starts.\n  Impact: `pane new --run` makes the pane and \
             leaves it running nothing, which looks exactly like a program that started and \
             printed nothing - and an agent nobody can tell is not listening.\n  This is what \
             the readiness wait in HerdrBackend::start is for: it is the one assertion in the \
             suite that fails when the wait comes out."
                .to_string()
        },
    );

    // And the same pane again, this time behind a wait. Its arrival is what makes the absence
    // below meaningful: without it, a missing file would read as a pane that never started
    // rather than as text that was thrown away. It is also the whole claim in one pair - same
    // pane, same program, same text, and a wait is the only difference between them.
    wait_until_ready(&daemon, raced.as_str());
    type_into(&daemon, raced.as_str(), "typed-once-it-was-ready");
    until_file(
        &handed.join("typed-once-it-was-ready"),
        "the same pane to take text once its program was reading",
    );
    assert!(
        !handed.join("typed-before-it-was-ready").exists(),
        "the program was handed text before it was ready and got it anyway, so this program \
         does not reproduce the loss the wait is for and this test is now the thing that is \
         wrong. Either the fixture stopped discarding pending input, or herdr stopped \
         delivering it to the pty before the program read it."
    );
    let _ = std::fs::remove_dir_all(&handed);
}

/// A directory this test owns, beside the daemon roots rather than inside one.
///
/// The fixture's path has to be known before the daemon exists - it is written into the
/// daemon's own config - so it cannot live under `Daemon::root`.
fn scratch(name: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    path
}

/// A program that resets its terminal as it starts, and files away every line it is handed.
///
/// Three things it has to be. It must not exit, because herdr closes a pane whose process
/// ends and then the workspace and then the daemon. It must print something once it is
/// ready, because that is what the wait waits for. And it must be unready for long enough
/// that "before it is ready" is a window a test can aim at rather than a race it might lose.
///
/// A line arrives as a file named after itself, so two panes running one program cannot be
/// confused for each other - the daemon has a single `default_shell` and both panes run it.
fn fixture(handed: &Path) -> PathBuf {
    let script = handed.join("resets-its-terminal.py");
    std::fs::write(
        &script,
        format!(
            r#"#!/usr/bin/env python3
import os, sys, termios, time

HANDED = {handed:?}

# Not ready. Anything typed into this pane now is sitting in the pty, unread.
time.sleep(1.0)

# The terminal taken in hand, the way a full-screen program takes it. TCSAFLUSH is the
# part that matters: it discards input that arrived and has not been read yet.
attributes = termios.tcgetattr(0)
attributes[3] &= ~termios.ECHO
termios.tcsetattr(0, termios.TCSAFLUSH, attributes)

# Now the screen has something on it, which is what the readiness wait is waiting for.
sys.stdout.write({prompt:?} + " ")
sys.stdout.flush()

while True:
    line = sys.stdin.readline()
    if not line:
        time.sleep(3600)
        continue
    name = line.strip() or "empty"
    with open(os.path.join(HANDED, name), "w") as handed:
        handed.write("handed")
"#,
            handed = handed.to_string_lossy(),
            prompt = PROMPT
        ),
    )
    .expect("the scratch directory should be writable");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the fixture should be executable");
    script
}

/// Waits for the fixture's own prompt, which is what says its program is reading.
///
/// The fixture's prompt rather than Muster's "anything on the visible screen", and the
/// difference is not pedantry: a terminal echoes what is typed into it, so text sent early
/// puts non-space on the screen and satisfies that test before the program has run a line.
/// Muster's own wait is not wrong for what it does - a pane it has just made has had nothing
/// typed into it - but this pane has, deliberately.
///
/// Through a client of its own because the harness's `call` is sized for a keystroke, and
/// `pane.wait_for_output` is the one call herdr answers slowly on purpose
/// (`observations/herdr-0.8.0.md` section 18).
fn wait_until_ready(daemon: &Daemon, pane: &str) {
    let client = HerdrClient::new(daemon.socket_path().to_string_lossy().into_owned());
    client
        .request_within(
            "pane.wait_for_output",
            &json!({
                "pane_id": pane,
                "match": { "type": "substring", "value": PROMPT },
                "source": "visible",
                "timeout_ms": 5_000,
            }),
            Duration::from_secs(10),
        )
        .expect("the fixture draws a prompt once it is ready for input");
}

/// Text and a Return, sent straight to the daemon.
///
/// Around Muster rather than through it, because what it is for is the arrangement Muster
/// does not have: a pane handed input with nothing waiting first.
fn type_into(daemon: &Daemon, pane: &str, text: &str) {
    daemon.call("pane.send_text", &json!({ "pane_id": pane, "text": text }));
    daemon.call("pane.send_input", &json!({ "pane_id": pane, "keys": ["enter"] }));
}

/// What the daemon itself calls a pane, asked of the daemon rather than of Muster's mirror.
///
/// The oracle has to be the daemon: what this is checking is that a request went out and took
/// effect, and Muster's own record of it comes from the same reply the assertion would be
/// reading.
fn label(daemon: &Daemon, names: &Names, pane: &PaneId) -> Option<String> {
    let backend = names.backend_pane(pane).expect("a name Muster minted resolves");
    let got = daemon.call("pane.get", &json!({ "pane_id": backend.as_str() }));
    got["pane"]["label"].as_str().filter(|label| !label.is_empty()).map(str::to_string)
}
