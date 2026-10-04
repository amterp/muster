//! One app per install per Muster home (mip/0006-one-process.md, section 5).
//!
//! Every window of an install is a window of one process, so a second process of the same install
//! must never start: it would follow the same daemon, fight the first for the same tabs, and write
//! the same arrangements. The first process to launch holds a lock for as long as it runs, and a
//! launch that finds it held hands what it was asked to do to the app holding it, then exits.
//!
//! `flock` rather than a file whose presence is the lock: the kernel lets go of it when the
//! process ends, by a quit, a crash or `kill -9` alike, so there is never a stale lock to doubt.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto::install::INSTALL;
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{AskForWindow, Request, Response, request, response};
use prost::Message;

/// The lock this process holds, kept open so that it stays held.
static HELD: Mutex<Option<File>> = Mutex::new(None);

/// How long one exchange with the app holding the lock may take.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a launch keeps trying to reach the app holding the lock, or to take the lock itself.
///
/// The app takes the lock before it starts listening, and in between it may spend up to five
/// seconds asking window processes from before to quit (`Retiring.patience` in the shell), so this
/// outlasts that with room for the rest of a launch. Short enough that a launch facing an app that
/// has hung gives up while somebody is still looking.
const HANDOVER_PATIENCE: Duration = Duration::from_secs(15);

/// What a claim came to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// This process is the app, and keeps its state here.
    Claimed { state: Option<PathBuf> },
    /// Another process is, and has been handed the request.
    HandedOver,
}

/// Takes the lock for this install under `home`, or hands `ask` to the app that holds it.
pub(crate) fn claim(home: &str, socket: &str, ask: AskForWindow) -> Result<Claim, String> {
    if home.is_empty() {
        return Ok(Claim::Claimed { state: None });
    }
    let home = Path::new(home);
    match lock(home, INSTALL, socket) {
        Ok(Some(file)) => Ok(claimed(home, file)),
        Ok(None) => {
            let ask = AskForWindow { install: INSTALL.to_string(), ..ask };
            match hand_over(home, INSTALL, socket, ask, HANDOVER_PATIENCE)? {
                Handed::Over => Ok(Claim::HandedOver),
                Handed::Back(file) => Ok(claimed(home, file)),
            }
        }
        // Not knowing whether another app runs is not a reason to refuse to be one: a home
        // somebody pointed at a read-only place still gets a window, as it always has.
        Err(failure) => {
            log::warn(
                "app.lock.failed",
                fields! {
                    "lock" => lock_path(home, INSTALL).display(),
                    "detail" => failure.to_string(),
                    "impact" => "this launch runs without knowing whether another Muster of this \
                                 install is running, so a second one could start beside it and \
                                 both would write the same windows' arrangements",
                    "check" => "whether the Muster home's state directory exists and is writable",
                },
            );
            Ok(Claim::Claimed { state: Some(state_directory(home, INSTALL)) })
        }
    }
}

/// This process as the app, holding `file`.
fn claimed(home: &Path, file: File) -> Claim {
    *poison::lock(&HELD, "app-lock") = Some(file);
    let state = state_directory(home, INSTALL);
    // A launch that cannot make its state directory still runs: it writes nothing, as a launch
    // with no home does, and says so where the arrangement fails to save.
    let _ = std::fs::create_dir_all(&state);
    log::info("app.claimed", fields! { "lock" => lock_path(home, INSTALL).display() });
    Claim::Claimed { state: Some(state) }
}

/// Where an install keeps its own state under a home: its window arrangements and its record of
/// which window holds each tab.
pub(crate) fn state_directory(home: &Path, install: &str) -> PathBuf {
    home.join("state").join(install)
}

fn lock_path(home: &Path, install: &str) -> PathBuf {
    home.join("state").join(format!("app-{install}.lock"))
}

/// The lock, held, with this process's socket written into it; or `None` when another process
/// holds it.
///
/// The socket is written after the lock is taken, so a launch reading the file while it is empty
/// is one that came between the two, and tries again.
fn lock(home: &Path, install: &str, socket: &str) -> std::io::Result<Option<File>> {
    let path = lock_path(home, install);
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let mut file =
        OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&path)?;
    // SAFETY: the descriptor is open for the life of `file`, which this owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let refused = std::io::Error::last_os_error();
        return match refused.raw_os_error() {
            Some(libc::EWOULDBLOCK) => Ok(None),
            _ => Err(refused),
        };
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(socket.as_bytes())?;
    Ok(Some(file))
}

/// The socket the app holding the lock wrote into it, or empty before it has.
fn socket_of_holder(home: &Path, install: &str) -> String {
    let mut text = String::new();
    let _ =
        File::open(lock_path(home, install)).and_then(|mut file| file.read_to_string(&mut text));
    text.trim().to_string()
}

/// What a handover came to.
#[derive(Debug)]
enum Handed {
    /// The app holding the lock took the request.
    Over,
    /// The app holding the lock ended before it answered, and this process took the lock.
    Back(File),
}

/// Sends `ask` to the app holding the lock, trying until it answers or `patience` runs out.
///
/// Between tries the lock is tried again, because the app holding it may be quitting: its socket
/// is gone while it still holds the lock, and once it ends there is nobody left to hand to and
/// this launch is the app.
fn hand_over(
    home: &Path,
    install: &str,
    socket: &str,
    ask: AskForWindow,
    patience: Duration,
) -> Result<Handed, String> {
    let request = Request::new(request::Payload::AskForWindow(ask));
    let deadline = Instant::now() + patience;
    let mut last = String::from("it has not said where it listens");
    let own = socket;
    loop {
        let socket = socket_of_holder(home, install);
        if !socket.is_empty() {
            match exchange(&socket, &request) {
                Ok(Response { payload: Some(response::Payload::Ok(_)) }) => {
                    return Ok(Handed::Over);
                }
                // Answered, and refused: trying again would be refused again.
                Ok(Response { payload: Some(response::Payload::Failure(failure)) }) => {
                    return Err(failure.reason);
                }
                Ok(other) => last = format!("it answered {other:?}"),
                Err(Unsent(failure)) => last = format!("{socket} did not answer ({failure})"),
                // Sent, and the answer lost: the app may have acted on it, and asking again could
                // open a second window.
                Err(Sent(failure)) => {
                    return Err(format!(
                        "the Muster already running was asked, at {socket}, and did not answer \
                         ({failure}). It may still do what was asked; if nothing appears, quit it \
                         and launch again."
                    ));
                }
            }
        }
        if let Ok(Some(file)) = lock(home, install, own) {
            return Ok(Handed::Back(file));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "another Muster of this install is running - it holds {} - but {last}, so \
                 nothing was opened. If it has hung, quit it (Activity Monitor, or `kill` the pid \
                 `lsof -t {}` names) and launch again.",
                lock_path(home, install).display(),
                lock_path(home, install).display(),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Why an exchange failed: before the request could have reached the app, so it is safe to send
/// again, or after, when the app may have acted on it.
enum Failed {
    Unsent(String),
    Sent(String),
}
use Failed::{Sent, Unsent};

/// A hang-up before the app said anything counts as unsent. The app answers every request it
/// reads (`command.rs`), so one that hung up without a byte never read it: it quit with the
/// connection still in its backlog, or died while answering, and either way its lock comes back
/// to this launch. A write the app stopped reading half way is the same. A timeout is not: an
/// app slow to answer may still act.
fn exchange(socket: &str, request: &Request) -> Result<Response, Failed> {
    let mut stream = UnixStream::connect(socket).map_err(|error| Unsent(error.to_string()))?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    write_frame(&mut stream, &request.encode_to_vec()).map_err(|error| {
        if hung_up(error.kind()) { Unsent(error.to_string()) } else { Sent(error.to_string()) }
    })?;
    let mut reading = Reading { stream, read: 0, failed: None };
    let reply = read_frame(&mut reading, LARGEST_MESSAGE).map_err(|failure| {
        let silent = reading.read == 0 && reading.failed.is_none_or(hung_up);
        if silent { Unsent(failure) } else { Sent(failure) }
    })?;
    Response::decode(reply.as_slice()).map_err(|error| Sent(error.to_string()))
}

/// The ways a peer that closed its end shows up. macOS reports a closed unix socket as any of
/// the three, depending on whether the app had read anything and how far the write got.
fn hung_up(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionReset, NotConnected};
    matches!(kind, BrokenPipe | ConnectionReset | NotConnected)
}

/// The reply's stream, counting what arrived and keeping why it stopped, which is how a hang-up
/// before any answer is told from one part way through, or from a timeout.
struct Reading {
    stream: UnixStream,
    read: usize,
    failed: Option<std::io::ErrorKind>,
}

impl Read for Reading {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        let read = self.stream.read(into);
        match &read {
            Ok(count) => self.read += count,
            // `read_frame` reads again after an interruption, so that is not why it stopped.
            Err(error) if error.kind() != std::io::ErrorKind::Interrupted => {
                self.failed = Some(error.kind());
            }
            Err(_) => {}
        }
        read
    }
}

/// Moves what every install kept in one place into the release's own state directory, once.
///
/// Only the release: it is the install that wrote there before installs had places of their own,
/// and a development build adopting the release's windows would open them onto a daemon that
/// holds none of their tabs. An arrangement stays where it is while a window process from before
/// is still running with a claim on it, and so does one the record says another install wrote.
pub(crate) fn adopt_old_state(home: &str) {
    if home.is_empty() || INSTALL != "release" {
        return;
    }
    adopt(Path::new(home), INSTALL);
}

/// [`adopt_old_state`] for any install, so a test can stand in for the release.
///
/// Runs at every launch rather than once: an arrangement held by a window process from before
/// that would not quit stays behind, and is adopted at a later launch once its claim has gone.
/// Paths are matched by file name, since every arrangement it moves came out of one directory and
/// the record may spell that directory differently - through a symlinked home, say.
pub(crate) fn adopt(home: &Path, install: &str) {
    let state = home.join("state");
    let (old_windows, old_record) = (state.join("windows"), state.join("holding/tabs.toml"));
    let own = state_directory(home, install);
    let (windows, record) = (own.join("windows"), own.join("holding/tabs.toml"));
    // The first time, the shared record; after that, this install's own, whose rows for an
    // arrangement left behind still name the old directory.
    let first = !record.exists();
    let from = if first { &old_record } else { &record };
    let text = std::fs::read_to_string(from).ok();
    let others: BTreeSet<std::ffi::OsString> = text
        .as_deref()
        .and_then(|text| from_toml(text).ok())
        .map(|holders| {
            holders
                .windows()
                .filter(|window| !window.install.is_empty() && window.install != install)
                .filter_map(|window| Path::new(&window.arrangement).file_name().map(Into::into))
                .collect()
        })
        .unwrap_or_default();
    // After the first adoption only what this install's own record left in the old directory is
    // its to take: the shared record, which said whose each arrangement was, has gone.
    let left_behind: Option<BTreeSet<std::ffi::OsString>> = (!first).then(|| {
        text.as_deref()
            .and_then(|text| from_toml(text).ok())
            .map(|holders| {
                holders
                    .windows()
                    .map(|window| Path::new(&window.arrangement))
                    .filter(|arrangement| !arrangement.starts_with(&windows))
                    .filter_map(|arrangement| arrangement.file_name().map(Into::into))
                    .collect()
            })
            .unwrap_or_default()
    });
    let mut moved: BTreeSet<std::ffi::OsString> = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(&old_windows) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "toml")
                || others.contains(&entry.file_name())
                || left_behind.as_ref().is_some_and(|ours| !ours.contains(&entry.file_name()))
                || claimed_by_a_running_process(&path.with_extension("held"))
            {
                continue;
            }
            let _ = std::fs::create_dir_all(&windows);
            if std::fs::rename(&path, windows.join(entry.file_name())).is_ok() {
                let _ = std::fs::remove_file(path.with_extension("held"));
                moved.insert(entry.file_name());
            }
        }
    }
    if moved.is_empty() && !(first && old_record.exists()) {
        return;
    }
    let rows = text.as_deref().and_then(|text| adopt_record(text, install, &moved, &windows));
    if let Some(text) = &rows {
        let written = record
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&record, text))
            .and_then(|()| if first { std::fs::remove_file(&old_record) } else { Ok(()) });
        if let Err(failure) = written {
            log::warn(
                "state.adopt.failed",
                fields! {
                    "record" => from.display(),
                    "detail" => failure.to_string(),
                    "impact" => "the windows open when Muster last ended may not all reopen, and \
                                 a closed window's tabs may join an open window",
                    "check" => "whether the state directory is writable",
                },
            );
        }
    }
    log::info(
        "state.adopted",
        fields! {
            "into" => own.display(),
            "arrangements" => moved.len().to_string(),
            "record" => rows.is_some().to_string(),
        },
    );
}

/// Whether a claim beside an arrangement names a process that is still running.
///
/// A window process from before removed its claim only on a clean quit, so one that crashed, was
/// killed or went down with the machine left a claim naming nobody. Adopting past such a claim is
/// the case this exists for: skipping it would leave that window's arrangement where nothing reads
/// it, and its tabs would join whichever window is open.
fn claimed_by_a_running_process(claim: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(claim) else { return false };
    let Ok(pid) = text.trim().parse::<libc::pid_t>() else { return false };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only asks whether the pid exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // Running as somebody else is still running.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The shared record, as `install`'s own: its rows and the rows from before rows named an
/// install, with every arrangement named in `moved` now under `new`. A row whose arrangement did
/// not move keeps its path, since that is still where its file is. `None` for a record this build
/// cannot read, which is left where it is rather than guessed at.
fn adopt_record(
    text: &str,
    install: &str,
    moved: &BTreeSet<std::ffi::OsString>,
    new: &Path,
) -> Option<String> {
    let mut holders = from_toml(text).ok()?;
    holders.adopt(install, |arrangement| match Path::new(arrangement).file_name() {
        Some(file) if moved.contains(file) => new.join(file).display().to_string(),
        _ => arrangement.to_string(),
    });
    Some(to_toml(&holders))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home of the test's own. Counted rather than timed: macOS keeps the clock to the
    /// microsecond, so two tests starting together were handed one home and took each other's
    /// lock.
    fn home() -> PathBuf {
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let home = std::env::temp_dir().join(format!(
            "muster-app-lock-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("a scratch home");
        home
    }

    /// A second claim on the same home is refused while the first is held, even from the same
    /// process - which is what lets this be tested here - and succeeds once it is let go.
    #[test]
    fn one_claim_per_install_per_home() {
        let home = home();
        let first = lock(&home, "i1", "/s/command-1.sock").expect("the lock opens");
        assert!(first.is_some(), "the first claim was refused");
        assert!(lock(&home, "i1", "/s/command-2.sock").expect("the lock opens").is_none());
        assert_eq!(socket_of_holder(&home, "i1"), "/s/command-1.sock");

        assert!(lock(&home, "i2", "/s/command-3.sock").expect("the lock opens").is_some());
        let elsewhere = self::home();
        assert!(lock(&elsewhere, "i1", "/s/command-4.sock").expect("the lock opens").is_some());

        drop(first);
        assert!(lock(&home, "i1", "/s/command-5.sock").expect("the lock opens").is_some());
        assert_eq!(socket_of_holder(&home, "i1"), "/s/command-5.sock");
    }

    /// A holder whose socket never answers is given up on with what to do about it, rather than
    /// waited on forever or answered with a second app.
    #[test]
    fn a_holder_that_never_answers_is_named() {
        let home = home();
        let _held = lock(&home, "i1", &home.join("nobody.sock").display().to_string())
            .expect("the lock opens");
        let refused = hand_over(
            &home,
            "i1",
            "/s/command-2.sock",
            AskForWindow::default(),
            Duration::from_millis(300),
        )
        .expect_err("handed over");
        assert!(refused.contains("app-i1.lock") && refused.contains("did not answer"), "{refused}");
    }

    /// An app that took the request and is slow to answer may act on it yet, so the launch says
    /// so rather than asking again - which could open a second window. Costs `PATIENCE`, the
    /// read timeout this is about.
    #[test]
    fn a_request_taken_and_not_answered_is_not_sent_again() {
        let home = home();
        let socket = home.join("silent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");
        let _held = lock(&home, "i1", &socket.display().to_string()).expect("the lock opens");
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counting = std::sync::Arc::clone(&asked);
        std::thread::spawn(move || {
            // Held open, unanswered: a hang-up would be an app that never read the request.
            let mut taken = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                if read_frame(&mut stream, LARGEST_MESSAGE).is_ok() {
                    counting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                taken.push(stream);
            }
        });

        let refused = hand_over(
            &home,
            "i1",
            "/s/command-2.sock",
            AskForWindow::default(),
            Duration::from_secs(2),
        )
        .expect_err("handed over");

        assert!(refused.contains("may still do what was asked"), "{refused}");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1, "asked more than once");
    }

    /// An app that took the connection and quit without reading it - the request sat in its
    /// backlog - never saw the request, so the launch takes the lock once it is let go rather
    /// than giving up on an app that may have acted.
    #[test]
    fn a_holder_that_quits_with_the_request_unread_hands_the_lock_on() {
        let home = home();
        let socket = home.join("quitting.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");
        let held = lock(&home, "i1", &socket.display().to_string()).expect("the lock opens");
        let quitting = std::thread::spawn(move || {
            let accepted = listener.accept().map(|(stream, _)| stream);
            drop(accepted);
            drop(listener);
            drop(held);
        });

        let handed = hand_over(
            &home,
            "i1",
            "/s/command-2.sock",
            AskForWindow::default(),
            Duration::from_secs(5),
        );

        quitting.join().expect("the holder quit");
        assert!(matches!(handed, Ok(Handed::Back(_))), "{handed:?}");
    }

    /// An app that hangs up without a word was never in a position to have answered, so the
    /// request is sent again until it answers or patience runs out - and the launch then says
    /// the app did not answer, not that it may have acted.
    #[test]
    fn a_holder_that_hangs_up_unanswered_is_asked_again() {
        let home = home();
        let socket = home.join("hanging-up.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");
        let _held = lock(&home, "i1", &socket.display().to_string()).expect("the lock opens");
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counting = std::sync::Arc::clone(&asked);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                if read_frame(&mut stream, LARGEST_MESSAGE).is_ok() {
                    counting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });

        let refused = hand_over(
            &home,
            "i1",
            "/s/command-2.sock",
            AskForWindow::default(),
            Duration::from_secs(1),
        )
        .expect_err("handed over");

        assert!(refused.contains("did not answer") && refused.contains("app-i1.lock"), "{refused}");
        assert!(asked.load(std::sync::atomic::Ordering::SeqCst) > 1, "asked only once");
    }

    /// An app that quits while a launch is handing to it leaves the lock to that launch, rather
    /// than the launch reporting an app that never answered.
    #[test]
    fn a_holder_that_quits_hands_the_lock_on() {
        let home = home();
        let held = lock(&home, "i1", &home.join("quitting.sock").display().to_string())
            .expect("the lock opens");
        let quitting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let handed = hand_over(
            &home,
            "i1",
            "/s/command-2.sock",
            AskForWindow::default(),
            Duration::from_secs(5),
        );
        quitting.join().expect("the holder let go");
        assert!(matches!(handed, Ok(Handed::Back(_))), "{handed:?}");
        assert_eq!(socket_of_holder(&home, "i1"), "/s/command-2.sock");
    }

    /// The release takes the arrangements nothing holds and the record, with every row's
    /// arrangement where its file is; one a running window holds stays put, and so does one
    /// another install wrote.
    #[test]
    fn the_old_state_is_adopted_once() {
        let home = home();
        let old = home.join("state/windows");
        std::fs::create_dir_all(&old).expect("the old windows directory");
        std::fs::create_dir_all(home.join("state/holding")).expect("the old holding directory");
        std::fs::write(old.join("window-1.toml"), "one").expect("written");
        std::fs::write(old.join("window-2.toml"), "two").expect("written");
        std::fs::write(old.join("window-2.held"), std::process::id().to_string()).expect("written");
        std::fs::write(old.join("window-4.toml"), "four").expect("written");
        let row = |name: &str, install: &str| {
            format!(
                "[[window]]\nname = \"{name}\"\narrangement = \"{}\"\nsocket = \"\"\npid = 7\n\
                 install = \"{install}\"\nfocused = 0\ndaemons = []\n\n",
                old.join(format!("{name}.toml")).display()
            )
        };
        let record = format!(
            "version = 1\n\n{}{}{}",
            row("window-1", ""),
            row("window-2", "release"),
            row("window-4", "dev-1234")
        );
        std::fs::write(home.join("state/holding/tabs.toml"), &record).expect("written");

        adopt(&home, "release");

        let own = home.join("state/release");
        assert_eq!(
            std::fs::read_to_string(own.join("windows/window-1.toml")).ok().as_deref(),
            Some("one")
        );
        assert!(old.join("window-2.toml").exists(), "a held arrangement was moved");
        assert!(!own.join("windows/window-2.toml").exists());
        assert!(old.join("window-4.toml").exists(), "another install's arrangement was taken");
        let adopted = std::fs::read_to_string(own.join("holding/tabs.toml")).expect("adopted");
        assert!(
            adopted.contains(&own.join("windows/window-1.toml").display().to_string())
                && adopted.contains(&old.join("window-2.toml").display().to_string())
                && adopted.contains("install = \"release\"")
                && !adopted.contains("window-4"),
            "{adopted}"
        );
        assert!(!home.join("state/holding/tabs.toml").exists(), "the old record was left behind");

        // The window a running process held is adopted at a later launch, once that process has
        // gone, with its row moved after it.
        std::fs::remove_file(old.join("window-2.held")).expect("the claim is there");
        adopt(&home, "release");
        assert_eq!(
            std::fs::read_to_string(own.join("windows/window-2.toml")).ok().as_deref(),
            Some("two")
        );
        let adopted = std::fs::read_to_string(own.join("holding/tabs.toml")).expect("adopted");
        assert!(
            adopted.contains(&own.join("windows/window-2.toml").display().to_string()),
            "{adopted}"
        );
        assert!(old.join("window-4.toml").exists(), "another install's arrangement was taken");
    }

    /// A claim whose process has gone - a crash, a kill, a reboot under a build from before - is
    /// no claim: its window is adopted like any other, so it reopens rather than being lost.
    #[test]
    fn a_claim_nobody_holds_any_more_is_adopted_past() {
        let home = home();
        let old = home.join("state/windows");
        std::fs::create_dir_all(&old).expect("the old windows directory");
        // Above any pid macOS or Linux hands out, so no process has it. Not a child started and
        // reaped for its pid: forking here would carry another test's lock into the child until
        // it execs, and that test would find its own lock still held.
        let gone = 4_000_000_u32;
        std::fs::write(old.join("window-1.toml"), "one").expect("written");
        std::fs::write(old.join("window-1.held"), gone.to_string()).expect("written");

        adopt(&home, "release");

        let own = home.join("state/release/windows");
        assert_eq!(std::fs::read_to_string(own.join("window-1.toml")).ok().as_deref(), Some("one"));
        assert!(!old.join("window-1.held").exists(), "the stale claim was left behind");
    }
}
