//! Muster's own daemon: whether one is listening, and starting one when none is.
//!
//! Muster ships a herdr and runs it rather than asking anybody to install one, because a
//! person using Muster should not have to learn what herdr is. That only means something if
//! the daemon it talks to is the daemon it shipped: an arbitrary one on the default socket is
//! an arbitrary version, and the corpus this project is judged against says nothing about
//! versions it was not recorded from. So Muster runs its own under a herdr session of its own
//! ([`crate::discovery::OWN_SESSION`]) and never meets a stranger.
//!
//! Started, and stopped only when somebody says so. Sessions outliving the app is a founding
//! desideratum, so the daemon is put in its own process group and quitting Muster costs it
//! nothing - the whole point is that the agents keep working. `stop` is the deliberate way out
//! of that, and nothing calls it unless a person asked: until it existed, the only way to end
//! a daemon was to find it with `pgrep` and kill it, which cost somebody a working agent
//! (kan a_28YghIUw2).

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use serde_json::json;

use crate::client::HerdrClient;
use crate::discovery::{OWN_SESSION, config_file};

/// How long a daemon that has just been started gets to answer.
///
/// Generous, because the alternative is worse: a launch that gives up early reports "no
/// daemon" about a daemon that is seconds from being ready, and the window it produces is the
/// empty one this whole path exists to prevent. A healthy start answers in well under a
/// second, so nobody who is not already broken waits this long.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait on a socket that should already have a daemon behind it.
///
/// Short, because this runs on every launch and the answer is nearly always immediate.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Whether something is listening on this socket and answering herdr's protocol.
///
/// A `ping` rather than a file check: a socket file outlives the daemon that made it, so its
/// presence is not an answer. A stale one is the ordinary state after a crash or a reboot.
pub fn answers(socket_path: &str) -> bool {
    HerdrClient::with_timeout(socket_path, PROBE_TIMEOUT).request("ping", &json!({})).is_ok()
}

/// Starts a daemon on this socket and waits until it answers.
///
/// The environment is built from an allowlist rather than inherited, and that is the whole of
/// [`carried`]. Everything the launching shell held would otherwise become permanent state in
/// a process that outlives the app and hands it to every pane it ever spawns.
///
/// What launchd did not give a GUI process is put back, and that is the whole of [`supplied`].
///
/// `HERDR_SESSION` rather than `--session`, so it reaches the panes the daemon spawns too.
/// That is what makes `herdr pane list` inside a Muster pane talk to the daemon that owns it
/// rather than to whatever the user's own herdr would find.
pub fn start(
    binary: &str,
    socket_path: &str,
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    config_path: Option<&str>,
    commands: Option<&str>,
) -> Result<(), String> {
    log::info(
        "daemon.starting",
        fields! {
            "binary" => binary,
            "socket" => socket_path,
            "session" => OWN_SESSION,
            // Whose config decides what a pane runs, which is the first question when a pane
            // opens the wrong shell. Muster's own derived file where there is one, and the
            // user's herdr config where the shell named nowhere to write one.
            "config" => config_path
                .map(ToString::to_string)
                .or_else(|| config_file(environment))
                .unwrap_or_default(),
            // Which of the two launches this is, because it decides what macOS charges every
            // pane's protected request to for as long as this daemon lives, and nothing else
            // on screen tells the two apart (`observations/macos-26.4.1.md`, section 8).
            "launch" => match launch(binary) {
                Launch::Directly => "spawned - panes are charged to Muster until it exits",
                Launch::ThroughLaunchServices => "opened - panes are charged to the daemon bundle",
            },
        },
    );

    let carried = carried(environment);
    let supplied = supplied(environment, locale, config_path, commands);
    let output = output_beside(config_path);
    let dropped: Vec<&str> = environment
        .keys()
        .filter(|name| !carried.contains_key(*name))
        .map(String::as_str)
        .collect();
    // Names, never values: this log is meant to be attachable to a bug report, and an
    // environment holds tokens (`architecture.md`, the diagnostic log). The names are what
    // somebody asking "why does my pane not see FOO" needs, and they are not secrets.
    //
    // Three lists rather than two, because a supplied variable is neither carried nor
    // dropped - it was never in the environment to be either. Reading a Dock launch's log
    // without it, the honest question "where did LANG come from" has no answer in the file.
    log::info(
        "daemon.environment",
        fields! {
            "carried" => carried.keys().cloned().collect::<Vec<String>>().join(","),
            "supplied" => supplied.keys().cloned().collect::<Vec<String>>().join(","),
            "dropped_count" => dropped.len().to_string(),
            "dropped" => dropped.join(","),
        },
    );

    let how = launch(binary);
    let mut child =
        spawn(how, binary, output.as_deref(), &carried, &supplied).map_err(|error| {
            format!(
                "the daemon at {binary} could not be started ({error}). Muster ships this \
             binary inside its own bundle, so a missing or unrunnable one usually means a \
             build that never staged it - `./dev -b` puts it beside the app - or a \
             MUSTER_HERDR pointing somewhere stale."
            )
        })?;

    // Launch Services hands the daemon back to launchd rather than to us, so `open` is the
    // only process we hold and it exits as soon as the app is on its way. Its status is
    // therefore about the launch and not about the daemon: a bundle that is missing, damaged
    // or refused by Gatekeeper fails here, and everything after that is the daemon's own and
    // says so in the file named below.
    if how == Launch::ThroughLaunchServices {
        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                return Err(format!(
                    "Launch Services refused to start the daemon bundle at {binary} \
                     ({status}), so this window has no session behind it. A bundle that was \
                     never assembled, one whose signature does not verify, and one macOS has \
                     quarantined all look like this; `open -n -a {binary}` by hand says \
                     which."
                ));
            }
            Err(error) => return Err(format!("the daemon launch could not be waited on: {error}")),
        }
    }

    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if answers(socket_path) {
            log::info("daemon.started", fields! { "socket" => socket_path });
            return Ok(());
        }
        // A daemon that exited says so now rather than at the end of the timeout, and its
        // status is the only clue there is - it has written no log if it never bound. Only on
        // the direct path: on the other, `child` was `open` and has already been waited on.
        if how == Launch::Directly
            && let Ok(Some(status)) = child.try_wait()
        {
            return Err(format!(
                "the daemon exited with {status} before it accepted a connection, so this \
                 window has no session behind it. A socket path over the ~104 bytes a Unix \
                 socket allows and a binary for the wrong architecture both look like this; \
                 {binary} run by hand says which."
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(match output {
        Some(path) => format!(
            "the daemon started but did not answer on {socket_path} within {}s, so this \
             window has no session behind it. It may still be coming up, in which case \
             relaunching Muster will find it; {path} is where it would have said otherwise.",
            START_TIMEOUT.as_secs()
        ),
        None => format!(
            "the daemon started but did not answer on {socket_path} within {}s, so this \
             window has no session behind it. It may still be coming up, in which case \
             relaunching Muster will find it.",
            START_TIMEOUT.as_secs()
        ),
    })
}

/// Starts the daemon, by whichever of the two routes its path calls for.
///
/// Split out of [`start`] because the two recipes are the interesting part and the rest of that
/// function is logging and waiting.
fn spawn(
    how: Launch,
    binary: &str,
    output: Option<&str>,
    carried: &BTreeMap<String, String>,
    supplied: &BTreeMap<String, String>,
) -> std::io::Result<Child> {
    match how {
        Launch::Directly => Command::new(binary)
            .arg("server")
            .env_clear()
            .envs(carried)
            .envs(supplied)
            .env("HERDR_SESSION", OWN_SESSION)
            // Its own process group, so that Muster quitting - or being killed with the
            // terminal it was launched from - does not take the agents with it.
            .process_group(0)
            .stdin(Stdio::null())
            // Its startup banner names the socket and the log it just opened, which is
            // Muster's job to report rather than herdr's. Standard error is left inherited:
            // it carries the failures that happen before herdr can open a log of its own - a
            // socket path over the `sockaddr_un` limit, a binary for the wrong architecture -
            // and a daemon that never started has nowhere else to say so.
            .stdout(Stdio::null())
            .spawn(),
        // `env_clear` on `open` rather than on the daemon, and it is not cosmetic. `open`
        // hands the app its own environment and then applies `--env` on top, so without this
        // every variable the launching process held reaches the daemon and outranks the
        // allowlist - measured on 2026-08-30, where a Muster started from an agent's pane gave
        // its daemon 97 variables including that agent's `CLAUDECODE` and a `HERDR_SOCKET_PATH`
        // pointing at somebody else's daemon, which herdr obeys over everything else. That is
        // the bug a_28YgGqYq7 fixed, arriving again by a different door.
        //
        // Cleared, the daemon gets what launchd gives any GUI process - `HOME`, `PATH`,
        // `SHELL`, `USER`, `LOGNAME`, `TMPDIR`, `SSH_AUTH_SOCK` - with Muster's own answers
        // laid over the top, which is the same environment a Dock-launched Muster is given.
        Launch::ThroughLaunchServices => Command::new(OPEN)
            .env_clear()
            .args(open_arguments(binary, output, carried, supplied))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn(),
    }
}

/// How a daemon gets started, which is decided by what its path names.
///
/// A bundle is launched through Launch Services and a bare binary is spawned, and the
/// difference is the whole of `observations/macos-26.4.1.md` section 8: macOS charges a pane's
/// protected request to the *responsible* process, a spawned child inherits the spawner's, and
/// only a process Launch Services started is its own. So a daemon Muster spawns is charged to
/// Muster until Muster exits and to nothing nameable afterwards, and a daemon Muster opens is
/// charged to the daemon's own bundle for as long as it lives - which is across every relaunch,
/// because the daemon is started and never stopped.
///
/// Read off the path rather than configured, because the two are the same question: only a
/// bundle can be opened, and a bundle is what the app ships. A plain build and every test stage
/// a bare binary and keep the spawn they always had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    Directly,
    ThroughLaunchServices,
}

pub fn launch(binary: &str) -> Launch {
    // Case-insensitively, because the filesystem this runs on is: `MusterSessions.APP` names
    // the same directory, and treating it as a bare binary would try to execute a folder.
    let bundle = std::path::Path::new(binary.trim_end_matches('/'))
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"));
    if bundle { Launch::ThroughLaunchServices } else { Launch::Directly }
}

/// Launch Services, which is a program rather than a call because `open` is what knows how to
/// hand an app an environment and a standard error.
const OPEN: &str = "/usr/bin/open";

/// What `open` is asked for, in one place so the corpus can hold it.
///
/// `-n` because Launch Services would otherwise activate a daemon that is already running
/// instead of starting one, and this path is only reached when nothing answered the socket.
///
/// `--env` is how a daemon started this way is handed the environment [`carried`] and
/// [`supplied`] built, one flag per entry. It only ever adds, so what it lands on top of is
/// decided by the caller clearing `open`'s own environment - see [`spawn`], where getting that
/// wrong is the failure worth knowing about.
///
/// `HERDR_SESSION` goes here rather than after `--args` so that it reaches the panes the daemon
/// spawns too, which is what makes `herdr pane list` inside a Muster pane talk to the daemon
/// that owns it.
pub fn open_arguments(
    bundle: &str,
    output: Option<&str>,
    carried: &BTreeMap<String, String>,
    supplied: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut arguments = vec!["-n".to_string(), "-a".to_string(), bundle.to_string()];
    if let Some(path) = output {
        arguments.push("--stderr".to_string());
        arguments.push(path.to_string());
    }
    for (name, value) in carried.iter().chain(supplied.iter()) {
        arguments.push("--env".to_string());
        arguments.push(format!("{name}={value}"));
    }
    arguments.push("--env".to_string());
    arguments.push(format!("HERDR_SESSION={OWN_SESSION}"));
    arguments.push("--args".to_string());
    arguments.push("server".to_string());
    arguments
}

/// Where a daemon Launch Services started writes what it could not put in its own log.
///
/// Beside the config file Muster wrote it, which is `~/.muster/state/herdr.toml`, so this is
/// `~/.muster/state/herdr.out` - the name `remote::start` already gives the same file on a far
/// machine. Derived rather than passed, because the two are the same directory by construction
/// and a second path across the seam would be a second thing to keep in step.
///
/// None when Muster wrote no config file. The launch still happens; what is lost is where a
/// daemon that died before opening its own log would have said so.
pub fn output_beside(config_path: Option<&str>) -> Option<String> {
    let path = config_path?;
    let directory = path.rsplit_once('/')?.0;
    Some(format!("{directory}/herdr.out"))
}

/// Ensures a daemon is listening on this socket, starting Muster's own if none is.
///
/// The order is deliberate: ask first, start second. A daemon left running by an earlier
/// Muster is exactly what should be reused - that is the whole of "sessions outlive the app" -
/// and starting a second one would bind nothing and lose the first one's panes.
pub fn ensure_running(
    socket_path: &str,
    binary: Option<&str>,
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    config_path: Option<&str>,
    commands: Option<&str>,
) -> Result<Reached, String> {
    if answers(socket_path) {
        return Ok(Reached::Adopted);
    }
    let Some(binary) = binary else {
        return Err(format!(
            "nothing is listening on {socket_path} and Muster has no daemon to start: no \
             binary was found beside the app and MUSTER_HERDR names none. This window will \
             render nothing. A bundle carries one (`./dev --bundle`), and an ordinary build \
             stages one beside the binary."
        ));
    };
    start(binary, socket_path, environment, locale, config_path, commands)?;
    Ok(Reached::Started)
}

/// Whether this daemon is one Muster just started or one it found already running.
///
/// The difference matters for exactly one thing, and it is not cosmetic: a daemon reads its
/// config when it starts. One Muster started is running the settings in the file; one it
/// adopted is running whatever it was started with, however long ago, and has to be asked to
/// read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reached {
    Started,
    Adopted,
}

/// Asks a daemon to read its config file again.
///
/// The lever that makes saving Muster's config file mean something. herdr reads its config at
/// startup and on request and watches no file, and Muster's daemon is started and never
/// stopped - so without this a changed setting would wait for a machine to be rebooted.
///
/// **What it cannot do is reach a pane that already exists.** herdr takes the shell and the
/// scrollback limit as arguments when it builds a pane's terminal, so both reach panes opened
/// afterwards and no others. The update checks are the exception and are the reason to call
/// this on a daemon Muster adopted: those are cancelled the moment the config is applied.
///
/// Ends a daemon, and the session in it.
///
/// The other half of a lifecycle that had only one, and the module comment above says why it
/// had only one: sessions outliving the app is a founding desideratum, so a daemon Muster
/// starts goes into a process group of its own and quitting cannot touch it. That stays the
/// default. What this is for is somebody saying outright that they are finished.
///
/// `server.stop` and not a signal, and not only because there is no pid to signal a daemon
/// opened through Launch Services. It is a *clean* stop, measured: a pane's process gets a
/// catchable SIGHUP and a window to act in, and the shell exits rather than being shot
/// (kan a_28YghIUw2).
///
/// The timeout is the caller's, because this is a daemon tearing down every pane it holds and
/// what counts as too long depends on whether somebody is waiting for a window to close.
///
/// Returned rather than reported, unlike the reload below: whether a session somebody asked to
/// end is still running is something the caller has to be able to say out loud.
pub fn stop(socket_path: &str, patience: Duration) -> Result<(), String> {
    HerdrClient::with_timeout(socket_path, patience)
        .request("server.stop", &json!({}))
        .map(|_| ())
        .map_err(|failure| failure.to_string())
}

/// A failure here is Muster's own file being refused, so it is reported rather than returned:
/// there is nothing the caller can do about it and nothing about the window is wrong yet.
pub fn reload_configuration(socket_path: &str) {
    let answer = HerdrClient::new(socket_path).request("server.reload_config", &json!({}));
    match answer {
        Ok(result) => {
            let status = result
                .get("config_reload")
                .and_then(|reload| reload.get("status"))
                .and_then(|status| status.as_str())
                .unwrap_or("unknown");
            log::info(
                "daemon.config.reloaded",
                fields! {
                    "socket" => socket_path,
                    "status" => status,
                    "impact" => "panes opened from now on run these settings; panes already \
                                 open keep the ones they were made with, because the daemon \
                                 takes both when it builds a pane",
                },
            );
        }
        Err(failure) => {
            log::warn(
                "daemon.config.refused",
                fields! {
                    "socket" => socket_path,
                    "detail" => failure.to_string(),
                    "impact" => "the daemon is running the settings it was started with, so a \
                                 setting saved since then reaches no new pane either",
                    "check" => "the file named in daemon.starting - Muster wrote it, so a \
                                daemon refusing it is a bug here rather than in anybody's \
                                config",
                },
            );
        }
    }
}

/// The environment Muster resolves its own socket path from.
///
/// Read here rather than deeper down, so that everything below takes a map and is answerable
/// to the corpus without an environment (`crate::discovery`).
pub fn environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

/// What Muster's daemon is entitled to inherit from whoever launched Muster: the client's
/// allowlist (`muster_daemon_client::environment`).
pub fn carried(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    muster_daemon_client::environment::carried(environment)
}

/// What Muster gives its daemon that nobody handed Muster: the client's locale and command
/// directory, and `HERDR_CONFIG_PATH`, naming the file Muster wrote from its own config so that
/// a `default_shell` set for somebody's own terminal stops deciding what every Muster pane runs.
pub fn supplied(
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    config_path: Option<&str>,
    commands: Option<&str>,
) -> BTreeMap<String, String> {
    let mut supplied = muster_daemon_client::environment::supplied(environment, locale, commands);
    if let Some(path) = config_path.filter(|path| !path.is_empty()) {
        supplied.insert("HERDR_CONFIG_PATH".to_string(), path.to_string());
    }
    supplied
}
