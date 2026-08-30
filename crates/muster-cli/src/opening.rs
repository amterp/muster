//! Opening a second window, which is the one thing here that is not a request.
//!
//! Every other command names a window and asks it something. This one has no window to name -
//! it is asked when there are none, or when the ones there are will not do - so it starts an
//! app rather than dialling one, and then waits for the endpoint that window binds. That is a
//! deliberate exception to "every command is a request the keyboard also sends" rather than an
//! oversight: a request has to reach a running core, and the whole point of this is that there
//! may not be one.
//!
//! **A window is a process.** Muster holds one window per process by construction - the core's
//! session is a global - so a second window is a second copy of the app, which is also why the
//! endpoint socket carries a pid. Nothing here is a workaround for that; it is the shape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::{Trouble, dial, environment};

/// How long to wait for a new window to bind its endpoint.
///
/// A cold launch has to start the app, reach a daemon - starting one if none answers - and open
/// a socket per pane it shows, so this is generous. It is a deadline against a launch that
/// failed rather than a budget a good one spends: a warm second window answers in about a
/// second.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often to look for it, which is a compromise nobody notices either way.
const INTERVAL: Duration = Duration::from_millis(100);

/// Where to look for a window override, for a test that must not launch the developer's app.
pub const APP_PATH: &str = "MUSTER_APP";

/// Starts another Muster and hands back the socket its window binds.
///
/// The socket rather than a pid, because the socket is what every other command takes: the next
/// line of a script is `muster --socket "$W" pane new`. Waiting for it rather than answering
/// immediately is the difference between a command a script can use and one it has to poll
/// after.
pub fn another_window(environment: &BTreeMap<String, String>) -> Result<String, Trouble> {
    let app = bundle(environment)?;

    // Read before launching, so that the new one is the one that was not here. Comparing paths
    // rather than counting: a window quitting while this runs would otherwise make the count
    // land back where it started and this wait forever.
    let before: Vec<String> = dial::candidates(environment);

    // `-n` is the whole request: without it macOS activates the copy that is already running
    // and no second process starts, which would look exactly like a launch that did nothing.
    //
    // Through `open` rather than by running the executable, because a window is a GUI app and
    // LaunchServices is what makes one: activation, the Dock, and which application macOS
    // charges a permission prompt to. The cost is that it hands over no environment, so the
    // one variable the new window cannot work out for itself travels as an argument.
    let mut opening = Command::new("/usr/bin/open");
    opening.arg("-n").arg("-a").arg(&app);
    if let Some(home) = environment::muster_home(environment) {
        opening.arg("--args").arg("--home").arg(home);
    }
    let started = opening.status();
    match started {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return Err(Trouble::Refused(format!(
                "opening {} exited {}, so no window was made. Try it by hand to see what macOS \
                 says about the bundle.",
                app.display(),
                status.code().unwrap_or(-1)
            )));
        }
        Err(error) => {
            return Err(Trouble::Refused(format!(
                "`open` could not be run ({error}), so no window was made. It is part of macOS \
                 at /usr/bin/open."
            )));
        }
    }

    appeared(environment, &before).ok_or_else(|| {
        Trouble::Unreachable(format!(
            "a Muster was started from {} and no new window answered within {PATIENCE:?}. It may \
             still be coming up - `muster window list` says which windows are listening. If none \
             appeared, the app failed to launch and said why on its own stderr.",
            app.display()
        ))
    })
}

/// The first endpoint that is listening and was not there before.
fn appeared(environment: &BTreeMap<String, String>, before: &[String]) -> Option<String> {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        // Answered rather than merely present: the app binds its socket partway through
        // starting up, and a caller handed the path a moment too early would dial nothing.
        for (path, answer) in dial::survey(environment, &crate::read_window()) {
            if answer.is_ok() && !before.contains(&path) {
                return Some(path);
            }
        }
        std::thread::sleep(INTERVAL);
    }
    None
}

/// The application bundle this command came out of.
///
/// The CLI ships inside the bundle, so the app to start is the one this binary is sitting in -
/// which is right whichever copy answered: the app's own, or the Homebrew link into
/// /Applications, since resolving that link lands in the same place. A caller that means a
/// different app says so with `$MUSTER_APP`.
fn bundle(environment: &BTreeMap<String, String>) -> Result<PathBuf, Trouble> {
    if let Some(named) = environment.get(APP_PATH).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(named));
    }

    let executable = std::env::current_exe().map_err(|error| {
        Trouble::Refused(format!(
            "this command cannot find its own path ({error}), so it cannot say which Muster to \
             open. Name one with ${APP_PATH}."
        ))
    })?;
    // Resolved, because the copy on somebody's PATH is usually a link: Homebrew's points into
    // /Applications and the app's own points into the bundle it staged.
    let resolved = std::fs::canonicalize(&executable).unwrap_or(executable);

    enclosing_bundle(&resolved).ok_or_else(|| {
        Trouble::Refused(format!(
            "this `muster` is at {}, which is not inside a muster.app - so there is no app for \
             it to open a second window of. That is what a build tree looks like: run `./dev \
             --bundle` and open that, or name one with ${APP_PATH}.",
            resolved.display()
        ))
    })
}

/// The nearest `.app` this path sits inside.
///
/// Its own function so a test can say what it is testing without a bundle on disk.
pub fn enclosing_bundle(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|ancestor| {
            ancestor.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        })
        .map(Path::to_path_buf)
}
