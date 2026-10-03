//! One app per install per Muster home (mip/0006-one-process.md, section 5).
//!
//! Every window of an install is a window of one process, so a second process of the same install
//! must never start: it would follow the same daemon, fight the first for the same tabs, and write
//! the same arrangements. The first process to launch holds a lock for as long as it runs, and a
//! launch that finds it held hands what it was asked to do to the app holding it, then exits.
//!
//! `flock` rather than a file whose presence is the lock: the kernel lets go of it when the
//! process ends, by a quit, a crash or `kill -9` alike, so there is never a stale lock to doubt.

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

/// How long a launch keeps trying to reach the app holding the lock.
///
/// The app takes the lock before it starts listening, so a launch moments after it finds the lock
/// held and nothing yet answering. Long enough for a launch to finish starting, and short enough
/// that a launch facing an app that has hung gives up while somebody is still looking.
const PATIENCE: Duration = Duration::from_secs(5);

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
        Ok(Some(file)) => {
            *poison::lock(&HELD, "app-lock") = Some(file);
            let state = state_directory(home, INSTALL);
            // A launch that cannot make its state directory still runs: it writes nothing, as a
            // launch with no home does, and says so where the arrangement fails to save.
            let _ = std::fs::create_dir_all(&state);
            log::info("app.claimed", fields! { "lock" => lock_path(home, INSTALL).display() });
            Ok(Claim::Claimed { state: Some(state) })
        }
        Ok(None) => {
            hand_over(home, INSTALL, AskForWindow { install: INSTALL.to_string(), ..ask })?;
            Ok(Claim::HandedOver)
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

/// Sends `ask` to the app holding the lock, trying until it answers or [`PATIENCE`] runs out.
fn hand_over(home: &Path, install: &str, ask: AskForWindow) -> Result<(), String> {
    let request = Request::new(request::Payload::AskForWindow(ask));
    let deadline = Instant::now() + PATIENCE;
    let mut last = String::from("it has not said where it listens");
    loop {
        let socket = socket_of_holder(home, install);
        if !socket.is_empty() {
            match exchange(&socket, &request) {
                Ok(Response { payload: Some(response::Payload::Ok(_)) }) => {
                    return Ok(());
                }
                // Answered, and refused: trying again would be refused again.
                Ok(Response { payload: Some(response::Payload::Failure(failure)) }) => {
                    return Err(failure.reason);
                }
                Ok(other) => last = format!("it answered {other:?}"),
                Err(failure) => last = format!("{socket} did not answer ({failure})"),
            }
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

fn exchange(socket: &str, request: &Request) -> Result<Response, String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    write_frame(&mut stream, &request.encode_to_vec()).map_err(|error| error.to_string())?;
    let reply = read_frame(&mut stream, LARGEST_MESSAGE)?;
    Response::decode(reply.as_slice()).map_err(|error| error.to_string())
}

/// Moves what every install kept in one place into the release's own state directory, once.
///
/// Only the release: it is the install that wrote there before installs had places of their own,
/// and a development build adopting the release's windows would open them onto a daemon that
/// holds none of their tabs. An arrangement with a claim beside it belongs to a window process
/// from before that is still running, and stays where that process writes it.
pub(crate) fn adopt_old_state(home: &str) {
    if home.is_empty() || INSTALL != "release" {
        return;
    }
    adopt(Path::new(home), INSTALL);
}

/// [`adopt_old_state`] for any install, so a test can stand in for the release.
pub(crate) fn adopt(home: &Path, install: &str) {
    let state = home.join("state");
    let (old_windows, old_record) = (state.join("windows"), state.join("holding/tabs.toml"));
    let own = state_directory(home, install);
    let (windows, record) = (own.join("windows"), own.join("holding/tabs.toml"));
    if record.exists() || windows.exists() {
        return;
    }
    let mut moved = 0usize;
    if let Ok(entries) = std::fs::read_dir(&old_windows) {
        let _ = std::fs::create_dir_all(&windows);
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "toml")
                || path.with_extension("held").exists()
            {
                continue;
            }
            if std::fs::rename(&path, windows.join(entry.file_name())).is_ok() {
                moved += 1;
            }
        }
    }
    let rows = match std::fs::read_to_string(&old_record) {
        Ok(text) => adopt_record(&text, install, &old_windows, &windows),
        Err(_) => None,
    };
    if let Some(text) = &rows {
        let written = record
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&record, text))
            .and_then(|()| std::fs::remove_file(&old_record));
        if let Err(failure) = written {
            log::warn(
                "state.adopt.failed",
                fields! {
                    "record" => old_record.display(),
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
            "arrangements" => moved.to_string(),
            "record" => rows.is_some().to_string(),
        },
    );
}

/// The shared record, as `install`'s own: its rows and the rows from before rows named an
/// install, with every arrangement under `old` now under `new`. `None` for a record this build
/// cannot read, which is left where it is rather than guessed at.
fn adopt_record(text: &str, install: &str, old: &Path, new: &Path) -> Option<String> {
    let mut holders = from_toml(text).ok()?;
    holders.adopt(install, |arrangement| {
        Path::new(arrangement)
            .strip_prefix(old)
            .map_or_else(|_| arrangement.to_string(), |rest| new.join(rest).display().to_string())
    });
    Some(to_toml(&holders))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "muster-app-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        ));
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
        let refused = hand_over(&home, "i1", AskForWindow::default()).expect_err("handed over");
        assert!(refused.contains("app-i1.lock") && refused.contains("did not answer"), "{refused}");
    }

    /// The release takes the arrangements nothing holds and the record, with every row's
    /// arrangement where its file went; one a running window holds stays put.
    #[test]
    fn the_old_state_is_adopted_once() {
        let home = home();
        let old = home.join("state/windows");
        std::fs::create_dir_all(&old).expect("the old windows directory");
        std::fs::create_dir_all(home.join("state/holding")).expect("the old holding directory");
        std::fs::write(old.join("window-1.toml"), "one").expect("written");
        std::fs::write(old.join("window-2.toml"), "two").expect("written");
        std::fs::write(old.join("window-2.held"), "4321").expect("written");
        let record = format!(
            "version = 1\n\n[[window]]\nname = \"window-1\"\narrangement = \"{}\"\nsocket = \"\"\n\
             pid = 0\nfocused = 0\ndaemons = []\n",
            old.join("window-1.toml").display()
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
        let adopted = std::fs::read_to_string(own.join("holding/tabs.toml")).expect("adopted");
        assert!(
            adopted.contains(&own.join("windows/window-1.toml").display().to_string())
                && adopted.contains("install = \"release\""),
            "{adopted}"
        );
        assert!(!home.join("state/holding/tabs.toml").exists(), "the old record was left behind");

        std::fs::write(old.join("window-3.toml"), "three").expect("written");
        adopt(&home, "release");
        assert!(!own.join("windows/window-3.toml").exists(), "adopted a second time");
    }
}
