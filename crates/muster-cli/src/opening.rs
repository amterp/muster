//! Opening another window: asked of the app that is running, or, with none, by starting it. And
//! closing one, which is only ever asked of the app running.
//!
//! Every window of an install is a window of one process (mip/0006-one-process.md), so a running
//! app is asked for a window (`AskForWindow`) and opens it beside the ones it has. Only when no
//! app of this install answers - none is running, or the one running predates being asked - does
//! this start one, and then wait for its endpoint. That half is a deliberate exception to "every
//! command is a request the keyboard also sends" rather than an oversight: a request has to
//! reach a running core, and the point of it is that there may not be one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use muster_daemon_proto::install;
use muster_proto::{AskForWindow, AskToCloseWindow, Request, Response, request, response};

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

/// What tells the new window that somebody asked for it.
///
/// Read on the other side by `launchIsFresh`. A flag rather than an environment variable for the
/// same reason `--home` is one: `open` hands the app its own environment, and the app is started
/// through Launch Services, which hands over launchd's.
pub const FRESH: &str = "--fresh";

/// A window that opened: its name, and the socket of the app it is in.
///
/// The name is what says which window; the socket reaches the app, whichever of its windows is
/// meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub window: String,
    pub socket: String,
}

/// Opens another window onto tabs of its own, and hands back what it is called once it is open.
///
/// Waiting for it rather than answering at once is the difference between a command a script can
/// use and one it has to poll after.
pub fn another_window(environment: &BTreeMap<String, String>) -> Result<Opened, Trouble> {
    open_a_window(environment, true, None)
}

/// Brings back a closed window - `name`, or the one closed last - onto the tabs it kept.
///
/// The same act as [`another_window`] with the arrangement chosen differently: a window somebody
/// asked for takes one nothing has ever held, and this takes the closed window's own.
pub fn the_closed_window(
    environment: &BTreeMap<String, String>,
    name: Option<&str>,
) -> Result<Opened, Trouble> {
    open_a_window(environment, false, name)
}

fn open_a_window(
    environment: &BTreeMap<String, String>,
    fresh: bool,
    name: Option<&str>,
) -> Result<Opened, Trouble> {
    if let Some(opened) = ask_a_running_app(environment, fresh, name)? {
        return Ok(opened);
    }
    let socket = launch(environment, fresh, name)?;
    let window = name.map(str::to_string).or_else(|| windows_open(&socket, environment).ok()?.0);
    Ok(Opened { window: window.unwrap_or_default(), socket })
}

/// Asks a running app of this install for the window, and waits for it to open. Nothing when no
/// app here takes the request: none answers, or every one belongs to another install or
/// predates being asked.
fn ask_a_running_app(
    environment: &BTreeMap<String, String>,
    fresh: bool,
    name: Option<&str>,
) -> Result<Option<Opened>, Trouble> {
    for socket in apps(environment) {
        let Ok((_, before)) = windows_open(&socket, environment) else { continue };
        if let Some(name) = name
            && before.contains(name)
        {
            return Ok(Some(Opened { window: name.to_string(), socket }));
        }
        let asked = Request::new(request::Payload::AskForWindow(AskForWindow {
            install: install::INSTALL.to_string(),
            fresh,
            name: name.unwrap_or_default().to_string(),
            ..AskForWindow::default()
        }));
        match dial::ask(&asked, Some(&socket), environment) {
            Ok(Response { payload: Some(response::Payload::Ok(_)) }) => {}
            // Another install's app, or one from before windows shared a process: it keeps its
            // windows to itself, so the next is asked, and failing every one the app is started.
            _ => continue,
        }
        return opened_in(&socket, environment, &before, name).map(Some).ok_or_else(|| {
            Trouble::Unreachable(format!(
                "the Muster at {socket} was asked for a window and none opened within \
                 {PATIENCE:?}. It may still be opening - `muster window` lists the windows there \
                 are - and if not, the app's run log says why."
            ))
        });
    }
    Ok(None)
}

/// The apps to ask, the one this command is running in first.
fn apps(environment: &BTreeMap<String, String>) -> Vec<String> {
    let mut apps: Vec<String> = environment
        .get(environment::WINDOW_SOCKET)
        .filter(|socket| !socket.is_empty())
        .cloned()
        .into_iter()
        .collect();
    for socket in dial::candidates(environment) {
        if !apps.contains(&socket) {
            apps.push(socket);
        }
    }
    apps
}

/// The window answering at `socket`, and every window open in its app: the one answering, and
/// the others it lists as open. Every window of an app is a window of one process, so the names
/// are enough, whatever the socket is called - a devenv pane's is not named for a pid.
fn windows_open(
    socket: &str,
    environment: &BTreeMap<String, String>,
) -> Result<(Option<String>, BTreeSet<String>), Trouble> {
    let answer = dial::ask(&crate::read_window(), Some(socket), environment)?;
    let Some(response::Payload::Window(window)) = answer.payload else {
        return Err(Trouble::Refused(format!("the Muster at {socket} did not say what it holds")));
    };
    let mut open: BTreeSet<String> = window
        .windows
        .iter()
        .filter(|other| other.pid != 0)
        .map(|other| other.name.clone())
        .collect();
    let answering = (!window.name.is_empty()).then(|| window.name.clone());
    open.extend(answering.clone());
    Ok((answering, open))
}

/// The window that opened in the app at `socket`: `name` once it is open, or the first window
/// open there that was not before.
fn opened_in(
    socket: &str,
    environment: &BTreeMap<String, String>,
    before: &BTreeSet<String>,
    name: Option<&str>,
) -> Option<Opened> {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if let Ok((_, open)) = windows_open(socket, environment) {
            let found = match name {
                Some(name) => open.contains(name).then(|| name.to_string()),
                None => open.difference(before).next().cloned(),
            };
            if let Some(window) = found {
                return Some(Opened { window, socket: socket.to_string() });
            }
        }
        std::thread::sleep(INTERVAL);
    }
    None
}

/// Closes a window - `name`, or the one this command is about - and hands back its name once the
/// app lists it closed.
///
/// The app is asked, and closes the window as its close button would. Waiting for it, as opening
/// does, so a script can reopen it on the next line. It is seen closed from another window open
/// beside it, because a read naming a closed window is still answered as that window.
pub fn close_a_window(
    environment: &BTreeMap<String, String>,
    socket: Option<&str>,
    name: Option<&str>,
) -> Result<String, Trouble> {
    let about = |request: Request| match name {
        Some(name) => request.for_window(name),
        None => request,
    };
    let (target, beside) = match dial::ask(&about(crate::read_window()), socket, environment)? {
        Response { payload: Some(response::Payload::Window(window)) } => {
            let beside =
                window.windows.iter().find(|other| other.pid != 0).map(|other| other.name.clone());
            (window.name, beside)
        }
        Response { payload: Some(response::Payload::Failure(failure)) } => {
            return Err(Trouble::Refused(failure.reason));
        }
        _ => return Err(Trouble::Refused("the app did not say which window this is".to_string())),
    };
    let asked =
        Request::new(request::Payload::AskToCloseWindow(AskToCloseWindow {})).for_window(&target);
    if let Response { payload: Some(response::Payload::Failure(failure)) } =
        dial::ask(&asked, socket, environment)?
    {
        return Err(Trouble::Refused(failure.reason));
    }
    let Some(beside) = beside else {
        // Only reachable for a window already closed with none open beside it, which an app
        // that is running does not have.
        return Ok(target);
    };
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        let read = crate::read_window().for_window(&beside);
        if let Ok(Response { payload: Some(response::Payload::Window(window)) }) =
            dial::ask(&read, socket, environment)
            && window.windows.iter().any(|other| other.name == target && other.pid == 0)
        {
            return Ok(target);
        }
        std::thread::sleep(INTERVAL);
    }
    Err(Trouble::Unreachable(format!(
        "the app was asked to close {target} and it was still open after {PATIENCE:?}. A sheet \
         asking something may be holding the window open; `muster window list` says whether it \
         closed since."
    )))
}

/// Starts the app, and hands back the socket it binds.
fn launch(
    environment: &BTreeMap<String, String>,
    fresh: bool,
    name: Option<&str>,
) -> Result<String, Trouble> {
    let app = bundle(environment)?;

    // Read before launching, so that the new one is the one that was not here. Comparing paths
    // rather than counting: a window quitting while this runs would otherwise make the count
    // land back where it started and this wait forever.
    let before: Vec<String> = dial::candidates(environment);

    // `-n` is the whole request: without it macOS activates the copy that is already running
    // and no second process starts, which would look exactly like a launch that did nothing.
    //
    // Through `open` rather than by running the executable, because a window is a GUI app and
    // Launch Services is what makes one: activation, the Dock, and which application macOS
    // charges a permission prompt to.
    //
    // `env_clear` on `open` itself, on the same terms and for the same measured reason as the
    // daemon's launch (`muster-daemon-client`'s `launch`, and `observations/macos-26.4.1.md`
    // section 8): `open` hands the app *its own* environment, so without this every variable
    // the caller held reaches the new window - and the caller is usually a pane, which carries
    // `MUSTER_PANE`, `MUSTER_SOCKET` and a `MUSTER_DAEMON_SOCKET` naming a daemon. That is the
    // bug `a_28YgGqYq7` fixed arriving through a third door, and it is invisible, because the
    // window opens and works.
    //
    // Cleared, the new window gets what launchd gives any GUI process, which is what a window
    // opened from the Dock gets. So the one thing it cannot then work out for itself travels
    // as an argument.
    let mut opening = Command::new("/usr/bin/open");
    opening.env_clear().arg("-n").arg("-a").arg(&app);
    opening.arg("--args");
    // `--fresh` when somebody asked for a window rather than for the one they closed. A window
    // Muster comes back to opens onto the tabs it was left on; a new one has to open onto tabs of
    // its own, since the tabs the window it was asked from is showing are panes the daemon lets
    // one bridge draw at a time. It also takes an arrangement of its own either way - the
    // difference is whether that arrangement is one nothing has ever held.
    if fresh {
        opening.arg(FRESH);
    }
    if let Some(name) = name {
        opening.arg("--window").arg(name);
    }
    if let Some(home) = environment::muster_home(environment) {
        opening.arg("--home").arg(home);
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
