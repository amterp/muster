//! Starting this machine's daemon when a `muster msg` verb finds none running (MIP-4, section 1).
//!
//! Started the way the app starts it, by the same code (`muster-daemon-launch`), on the same
//! socket and written down in the same records, so a window opened later adopts it rather than
//! starting a second.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use muster_daemon_launch::{environment as daemon_environment, launch, records};
use muster_daemon_proto::install;

use crate::Trouble;
use crate::daemon::{MayStart, SOCKET};

/// What the app's own `MUSTER_DAEMON_BINARY` is called, which names a daemon to run in place of
/// the one this build ships (`DaemonLocation.swift`). Honored here too, so the CLI starts what
/// the app would.
const DAEMON_BINARY: &str = "MUSTER_DAEMON_BINARY";

/// Starts this install's daemon on `socket`, where connecting failed with `error`, and returns
/// once it answers.
pub(crate) fn start(socket: &Path, error: &std::io::Error, may: MayStart) -> Result<(), Trouble> {
    let home = crate::environment::muster_home(may.environment).map(PathBuf::from);
    let ours = home.as_deref().map(install::socket_path);
    let Some(home) = home.filter(|_| ours.as_deref() == Some(socket)) else {
        return Err(Trouble::Unreachable(format!(
            "no muster-daemon answered at {} ({error}). ${SOCKET} names that socket, and a daemon \
             somebody named is theirs to start, so none was started there. Unset ${SOCKET} to \
             reach this install's own daemon, which `muster msg` starts when none runs.",
            socket.display()
        )));
    };
    let binary = this_executable()
        .map_err(|why| {
            format!("this muster could not find its own executable ({why}) to look beside it")
        })
        .and_then(|cli| {
            daemon_beside(&cli, may.environment, Path::is_file).map_err(|looked| {
                let looked: Vec<String> =
                    looked.iter().map(|path| path.display().to_string()).collect();
                format!(
                    "this muster has no muster-daemon beside it: it looked at {}",
                    looked.join(" and ")
                )
            })
        })
        .map_err(|why| {
            Trouble::Unreachable(format!(
                "no muster-daemon answered at {} ({error}), and none was started: {why}. A muster \
                 installed apart from the app cannot start one; open Muster, or run the `muster` \
                 the app links at ~/.muster/bin.",
                socket.display()
            ))
        })?;

    crate::say_note(
        &format!(
            "no muster-daemon was running at {}, so this is starting one ({}). It brings back \
             the tabs it saved, each pane as a shell.",
            socket.display(),
            binary.display()
        ),
        may.json,
        &mut std::io::stderr(),
    );
    let commands = home.join("bin");
    let given = daemon_environment::for_daemon_from_a_shell(
        may.environment,
        commands.join("muster").exists().then(|| commands.to_string_lossy()).as_deref(),
    );
    let (reached, _) = launch::ensure_running(&launch::Launch {
        binary: &binary,
        data: None,
        socket,
        environment: &given,
        impact: "`muster msg` has no daemon to ask",
    })
    .map_err(Trouble::Unreachable)?;
    // Written down only when this start's daemon is the one that answered, as the app does: one
    // a rival started belongs to whoever started it.
    if reached == launch::Reached::Started {
        records::started(
            &home.join("state").join("daemons").to_string_lossy(),
            &socket.to_string_lossy(),
        );
    }
    Ok(())
}

/// This CLI, with every link followed: Homebrew's `muster` and `~/.muster/bin/muster` are links
/// into the app, and the daemon is found beside where they lead.
fn this_executable() -> std::io::Result<PathBuf> {
    std::env::current_exe()?.canonicalize()
}

/// The daemon that belongs with the CLI at `cli`, or every place it was looked for.
///
/// The places the app looks for its own daemon from its executable (`DaemonLocation.swift`),
/// which sits beside this CLI in a bundle and in a build: the helper application a bundle carries
/// in `Contents/Library`, then beside the executable, which is a SwiftPM build and an install on
/// another machine (`~/.muster/daemon/<version>/`). Its data directory is the daemon's own to
/// find, beside it or in its bundle's resources.
pub(crate) fn daemon_beside(
    cli: &Path,
    environment: &BTreeMap<String, String>,
    is_file: impl Fn(&Path) -> bool,
) -> Result<PathBuf, Vec<PathBuf>> {
    if let Some(named) = environment.get(DAEMON_BINARY).filter(|named| !named.is_empty()) {
        return Ok(PathBuf::from(named));
    }
    let folder = cli.parent().unwrap_or(Path::new("/"));
    let candidates: Vec<PathBuf> = folder
        .parent()
        .map(|contents| contents.join("Library/MusterSessions.app/Contents/MacOS/muster-daemon"))
        .into_iter()
        .chain([folder.join("muster-daemon")])
        .collect();
    match candidates.iter().find(|candidate| is_file(candidate)) {
        Some(found) => Ok(found.clone()),
        None => Err(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(
        cli: &str,
        files: &[&str],
        environment: &[(&str, &str)],
    ) -> Result<PathBuf, Vec<PathBuf>> {
        let environment: BTreeMap<String, String> = environment
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        daemon_beside(Path::new(cli), &environment, |path| {
            files.iter().any(|file| Path::new(file) == path)
        })
    }

    #[test]
    fn a_bundles_cli_starts_the_helper_application() {
        assert_eq!(
            found(
                "/Applications/Muster.app/Contents/MacOS/muster-cli",
                &[
                    "/Applications/Muster.app/Contents/Library/MusterSessions.app/Contents/MacOS/muster-daemon"
                ],
                &[],
            ),
            Ok(PathBuf::from(
                "/Applications/Muster.app/Contents/Library/MusterSessions.app/Contents/MacOS/muster-daemon"
            ))
        );
    }

    #[test]
    fn a_build_or_another_machines_install_starts_the_daemon_beside_it() {
        assert_eq!(
            found(
                "/home/dev/.muster/daemon/0.13.0/muster",
                &["/home/dev/.muster/daemon/0.13.0/muster-daemon"],
                &[],
            ),
            Ok(PathBuf::from("/home/dev/.muster/daemon/0.13.0/muster-daemon"))
        );
    }

    #[test]
    fn the_apps_override_wins_and_nothing_found_says_where_it_looked() {
        assert_eq!(
            found(
                "/x/bin/muster",
                &["/x/bin/muster-daemon"],
                &[(DAEMON_BINARY, "/mine/muster-daemon")]
            ),
            Ok(PathBuf::from("/mine/muster-daemon"))
        );
        assert_eq!(
            found("/usr/local/bin/muster", &[], &[(DAEMON_BINARY, "")]),
            Err(vec![
                PathBuf::from("/usr/local/Library/MusterSessions.app/Contents/MacOS/muster-daemon"),
                PathBuf::from("/usr/local/bin/muster-daemon"),
            ])
        );
    }
}
