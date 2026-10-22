//! Saying so when another install's daemon holds panes this window cannot show.
//!
//! Each install has a daemon of its own, so a build put in place of the installed Muster without
//! being built for its install (`MUSTER_INSTALL`) starts a second, empty daemon beside the one
//! holding every agent. The window opened on one blank pane and said nothing, and the agents went
//! on running out of sight (kan a_2gNXMhi6k). Muster's own records of the daemons it started are
//! what can tell: one in this home, of another install, answering with panes, is that case.
//!
//! A home of its own sees no other install's records, so `tools/run-checkout` and the contract
//! tier, which each run a build under a separate `MUSTER_HOME`, never raise this.

use std::path::Path;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::problems::Severity;
use muster_daemon_client::records::{self, Census, State};
use muster_daemon_proto::install;

use super::problems::{clear_problem, raise_problem};
use super::startup::daemon_records_path;

const KEY: &str = "daemon.other-install";

/// Asks every daemon in the records, on a thread of its own, and raises or clears the problem.
///
/// Off the attach path because the census dials each record and asks each answering daemon for
/// its panes, five seconds at most apiece, and a window should not wait on that to draw.
pub(crate) fn look_beside(ours: String) {
    let Some(directory) = daemon_records_path() else { return };
    let spawned =
        std::thread::Builder::new().name("muster-other-installs".to_string()).spawn(move || {
            let found = records::census(&directory);
            let others = beside(&ours, &found);
            if others.is_empty() {
                clear_problem(KEY, "no other install's daemon in this home holds panes");
                return;
            }
            let detail = sentence(install::INSTALL, &others);
            let sockets: Vec<&str> = others.iter().map(|other| other.socket.as_str()).collect();
            log::warn(
                "daemon.other_install",
                fields! {
                    "ours" => &ours,
                    "others" => sockets.join(", "),
                    "impact" => "the agents in those panes keep running, and this window cannot \
                                 show them",
                    "check" => "whether this build was meant to replace the installed Muster: \
                                if so it needed MUSTER_INSTALL set to that install's name; \
                                `muster daemons` lists every daemon in this home",
                },
            );
            raise_problem(KEY, Severity::Error, &detail);
        });
    if let Err(error) = spawned {
        log::warn(
            "daemon.other_install.unchecked",
            fields! {
                "error" => error.to_string(),
                "impact" => "a daemon of another install holding panes goes unmentioned",
            },
        );
    }
}

/// The daemons in `found` that answer and hold panes, beside `ours` in the same directory but on
/// another socket: another install in this home.
fn beside<'a>(ours: &str, found: &'a [Census]) -> Vec<&'a Census> {
    let directory = Path::new(ours).parent();
    found
        .iter()
        .filter(|other| {
            other.state == State::Answering
                && other.panes > 0
                && other.socket != ours
                && Path::new(&other.socket).parent() == directory
        })
        .collect()
}

/// What the person is told, naming each install by its socket's name as `muster daemons` does.
fn sentence(ours: &str, others: &[&Census]) -> String {
    let named: Vec<String> = others
        .iter()
        .map(|other| {
            let install = Path::new(&other.socket)
                .file_stem()
                .map_or_else(|| other.socket.clone(), |stem| stem.to_string_lossy().into_owned());
            let version = if other.version.is_empty() {
                String::new()
            } else {
                format!("{}, ", other.version)
            };
            format!("{install} ({version}{} pane(s)) at {}", other.panes, other.socket)
        })
        .collect();
    let mut detail = format!(
        "Another Muster install's daemon on this machine holds panes this window cannot show: {}. \
         This Muster is install {ours} and runs a daemon of its own, so the agents in those panes \
         keep running out of its sight. Open the Muster they belong to to see them; `muster \
         daemons` lists every daemon here and how to end one.",
        named.join("; ")
    );
    if ours.starts_with("dev-") {
        detail.push_str(
            " A build meant to replace the installed Muster has to be built for its install, \
             with MUSTER_INSTALL: tools/install-main in a Muster checkout does that.",
        );
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon(socket: &str, state: State, panes: u32) -> Census {
        Census {
            socket: socket.to_string(),
            started: 0,
            state,
            panes,
            directories: Vec::new(),
            version: "0.17.0".to_string(),
        }
    }

    #[test]
    fn only_another_install_in_this_home_holding_panes_counts() {
        let ours = "/home/a/.muster/daemon/dev-0a1b2c3d4e5f.sock";
        let found = [
            daemon(ours, State::Answering, 1),
            daemon("/home/a/.muster/daemon/release.sock", State::Answering, 18),
            daemon("/home/a/.muster/daemon/dev-ffffffffffff.sock", State::Answering, 0),
            daemon("/home/a/.muster/daemon/dev-eeeeeeeeeeee.sock", State::Silent, 0),
            daemon("/home/a/.muster-dev/daemon/release.sock", State::Answering, 3),
        ];
        let others: Vec<&str> =
            beside(ours, &found).iter().map(|other| other.socket.as_str()).collect();
        assert_eq!(others, ["/home/a/.muster/daemon/release.sock"]);
    }

    #[test]
    fn a_development_build_is_told_how_to_replace_the_installed_muster() {
        let release = daemon("/home/a/.muster/daemon/release.sock", State::Answering, 18);
        let detail = sentence("dev-0a1b2c3d4e5f", &[&release]);
        assert!(
            detail.contains("release (0.17.0, 18 pane(s)) at /home/a/.muster/daemon/release.sock"),
            "{detail}"
        );
        assert!(detail.contains("install dev-0a1b2c3d4e5f"), "{detail}");
        assert!(detail.contains("MUSTER_INSTALL"), "{detail}");
    }

    #[test]
    fn a_release_finding_a_stray_development_daemon_is_not_told_about_building() {
        let stray = daemon("/home/a/.muster/daemon/dev-0a1b2c3d4e5f.sock", State::Answering, 1);
        let detail = sentence("release", &[&stray]);
        assert!(detail.contains("dev-0a1b2c3d4e5f (0.17.0, 1 pane(s))"), "{detail}");
        assert!(!detail.contains("MUSTER_INSTALL"), "{detail}");
    }
}
