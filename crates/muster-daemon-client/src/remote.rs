//! Starting the daemon on another machine, over an ssh master that forwards its socket here.
//!
//! The same rule as on this machine: whatever answers is adopted, and a daemon is started only
//! when nothing does. It is started from where this version's daemon is installed over there,
//! `~/.muster/daemon/<version>/`, with its data directory beside it, and listens on the install's
//! socket (MIP-3, sections 1 and 12). Nothing here installs it: Muster does not yet copy its
//! daemon to a remote machine, so one that has none is told what to copy where.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::HandshakeError;
use muster_daemon_proto::{ConnectionKind, Welcome, install};
use muster_ssh::{Remote, quoted};

use crate::launch::Reached;

/// How long a daemon started over there may take to answer through the forward. Longer than
/// on this machine, since a devenv is often at the end of a VPN.
const START_PATIENCE: Duration = Duration::from_secs(30);

const DIAL_INTERVAL: Duration = Duration::from_millis(20);

/// Where this version's daemon is on a machine whose environment is `environment`, as
/// `muster_ssh::remote_environment` read it, and where it listens there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub binary: PathBuf,
    pub socket: PathBuf,
}

impl Installed {
    /// None when the far environment names no home at all.
    pub fn on(environment: &BTreeMap<String, String>) -> Option<Installed> {
        let home = install::muster_home(|name| environment.get(name).cloned())?;
        Some(Installed {
            binary: home.join("daemon").join(env!("CARGO_PKG_VERSION")).join("muster-daemon"),
            socket: install::socket_path(&home),
        })
    }

    fn stderr(&self) -> PathBuf {
        self.socket.with_extension("stderr")
    }
}

/// Dials the daemon through `local_socket`, the forward's end here, and starts the one
/// `installed` names over there if nothing answers.
pub fn ensure_running(
    remote: &Remote,
    installed: &Installed,
    local_socket: &Path,
) -> Result<(Reached, Welcome), String> {
    match probe(local_socket) {
        Ok(welcome) => return Ok((Reached::Adopted, welcome)),
        Err(HandshakeError::Unreachable(_)) => {}
        Err(error) => {
            return Err(format!(
                "the daemon on {} would not talk to this app: {error}. Its panes are still \
                 running, and this window cannot show them until a Muster that speaks its \
                 protocol is used.",
                remote.host()
            ));
        }
    }

    let present =
        remote.shell(&format!("test -x {} && echo yes || echo no", path(&installed.binary)))?;
    if present.trim() != "yes" {
        return Err(format!(
            "{host} has no muster-daemon at {binary}, so its panes are absent from the window \
             and nothing else is affected. Muster does not install its daemon on a remote \
             machine yet: copy a muster-daemon built for that machine (`uname -sm` there says \
             which), and the muster-daemon-data directory beside it, into {directory} on {host}.",
            host = remote.host(),
            binary = installed.binary.display(),
            directory = installed.binary.parent().unwrap_or(Path::new("/")).display(),
        ));
    }

    log::info(
        "daemon.remote.starting",
        fields! {
            "host" => remote.host(),
            "binary" => installed.binary.display(),
            "socket" => installed.socket.display(),
        },
    );
    remote.shell(&start_script(installed))?;

    let deadline = Instant::now() + START_PATIENCE;
    loop {
        if let Ok(welcome) = probe(local_socket) {
            log::info(
                "daemon.remote.started",
                fields! { "host" => remote.host(), "pid" => welcome.pid, "instance" => welcome.instance },
            );
            return Ok((Reached::Started, welcome));
        }
        if Instant::now() >= deadline {
            let said = remote
                .shell(&format!("cat {} 2>/dev/null", path(&installed.stderr())))
                .unwrap_or_default();
            return Err(format!(
                "the daemon on {} was started and did not answer within {}s, so its panes are \
                 absent from the window. It said: {}",
                remote.host(),
                START_PATIENCE.as_secs(),
                if said.trim().is_empty() { "nothing" } else { said.trim() }
            ));
        }
        std::thread::sleep(DIAL_INTERVAL);
    }
}

/// The command that starts the daemon over there and returns at once.
///
/// In a session of its own where the machine has `setsid`, and under `nohup` where it does not
/// (a remote Mac ships no `setsid`), so the daemon outlives the ssh connection that started it.
/// The brace group keeps the redirects on the daemon itself: dash execs the last command of a
/// backgrounded list, and without the group ssh would wait on pipes the daemon holds open.
pub fn start_script(installed: &Installed) -> String {
    let binary = path(&installed.binary);
    let socket = path(&installed.socket);
    let errors = path(&installed.stderr());
    let directory = path(installed.socket.parent().unwrap_or(Path::new("/")));
    format!(
        "mkdir -p {directory} && if command -v setsid >/dev/null 2>&1; then \
         {{ setsid {binary} --socket {socket} > {errors} 2>&1 < /dev/null & }}; else \
         {{ nohup {binary} --socket {socket} > {errors} 2>&1 < /dev/null & }}; fi"
    )
}

fn path(path: &Path) -> String {
    quoted(&path.to_string_lossy())
}

fn probe(socket: &Path) -> Result<Welcome, HandshakeError> {
    crate::dial(socket, ConnectionKind::Control, "muster probe").map(|(_, welcome)| welcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect()
    }

    #[test]
    fn the_daemon_is_found_under_the_far_machines_muster_home() {
        let installed = Installed::on(&environment(&[("HOME", "/home/dev")])).unwrap();
        assert_eq!(
            installed.binary,
            Path::new("/home/dev/.muster/daemon")
                .join(env!("CARGO_PKG_VERSION"))
                .join("muster-daemon")
        );
        assert_eq!(installed.socket, install::socket_path(Path::new("/home/dev/.muster")));

        let elsewhere =
            Installed::on(&environment(&[("HOME", "/home/dev"), ("MUSTER_HOME", "/m")])).unwrap();
        assert!(elsewhere.socket.starts_with("/m/daemon"));
        assert!(Installed::on(&environment(&[])).is_none(), "no home, nowhere to look");
    }

    #[test]
    fn the_start_script_quotes_every_path() {
        let installed = Installed {
            binary: PathBuf::from("/home/o'neil/.muster/daemon/0.9.0/muster-daemon"),
            socket: PathBuf::from("/home/o'neil/.muster/daemon/dev-1.sock"),
        };
        let script = start_script(&installed);
        assert!(script.contains(&quoted("/home/o'neil/.muster/daemon/0.9.0/muster-daemon")));
        assert!(script.contains(&quoted("/home/o'neil/.muster/daemon/dev-1.stderr")));
    }

    /// Run by the shells a remote might have, it returns at once and leaves the daemon running,
    /// told where to listen: bash forks a backgrounded list, and dash execs its last command.
    #[test]
    fn the_start_script_returns_and_leaves_the_daemon_running() {
        let root = std::env::temp_dir().join(format!("muster-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let told = root.join("told");
        let binary = root.join("muster-daemon");
        std::fs::write(
            &binary,
            format!("#!/bin/sh\necho \"$@\" > '{}'\nsleep 3\n", told.display()),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let installed = Installed { binary, socket: root.join("daemon").join("d.sock") };

        for shell in ["/bin/sh", "/bin/bash"] {
            let _ = std::fs::remove_file(&told);
            let started = Instant::now();
            let output = std::process::Command::new(shell)
                .arg("-c")
                .arg(start_script(&installed))
                .stdout(std::process::Stdio::piped())
                .output()
                .unwrap();
            assert!(output.status.success(), "{shell}: {output:?}");
            assert!(started.elapsed() < Duration::from_secs(5), "{shell} waited on the daemon");
            let deadline = Instant::now() + Duration::from_secs(5);
            while !told.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let told = std::fs::read_to_string(&told).unwrap_or_default();
            assert_eq!(told.trim(), format!("--socket {}", installed.socket.display()), "{shell}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
