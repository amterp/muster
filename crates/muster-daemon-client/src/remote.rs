//! Starting the daemon on another machine, over an ssh master that forwards its socket here.
//!
//! The same rule as on this machine: whatever answers is adopted, and a daemon is started only
//! when nothing does. It is started from where this version's daemon is installed over there,
//! `~/.muster/daemon/<version>/`, with its data directory beside it, and listens on the install's
//! socket (MIP-3, sections 1 and 12). What is installed there is checked first, and replaced
//! with what this app carries (`crate::install`) when it is missing or is not the same build.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::HandshakeError;
use muster_daemon_proto::{ConnectionKind, Welcome, install};
use muster_ssh::{Platform, Remote, quoted};

use crate::install::{Carried, Payload};
use crate::launch::Reached;

/// How long a daemon started over there may take to answer through the forward.
///
/// Longer than on this machine, since a devenv is often at the end of a VPN. And a Mac over
/// there holds a daemon just installed while it scans it, as this one does its own: a reinstall
/// of the debug daemon was first heard from 46 s after it was started, on a loaded machine. A
/// daemon that dies meanwhile is caught by the liveness check, so this wait is only ever spent
/// on one that is still running.
const START_PATIENCE: Duration = Duration::from_secs(90);

const DIAL_INTERVAL: Duration = Duration::from_millis(20);

/// How often a starting daemon is checked for having exited. Each check is an ssh round trip,
/// so far fewer of them than dials.
const EXIT_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// A shell on the machine the daemon is to run on: ssh's in the app, and in tests a local `sh`,
/// where the far side and this one are the same machine.
pub trait Far {
    fn host(&self) -> &str;
    /// Runs `script` there and returns what it printed.
    fn shell(&self, script: &str) -> Result<String, String> {
        self.shell_on(script, &[])
    }
    /// The same, with `input` on the script's standard input.
    fn shell_on(&self, script: &str, input: &[u8]) -> Result<String, String>;
}

impl Far for Remote {
    fn host(&self) -> &str {
        Remote::host(self)
    }

    fn shell_on(&self, script: &str, input: &[u8]) -> Result<String, String> {
        Remote::shell_on(self, script, input)
    }
}

/// Where this version's daemon is on a machine whose environment is `environment`, as
/// `muster_ssh::remote_environment` read it, and where it listens there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The version's directory, which an install replaces whole.
    pub directory: PathBuf,
    pub binary: PathBuf,
    pub socket: PathBuf,
}

impl Installed {
    /// None when the far environment names no home at all.
    pub fn on(environment: &BTreeMap<String, String>) -> Option<Installed> {
        let home = install::muster_home(|name| environment.get(name).cloned())?;
        let directory = home.join("daemon").join(env!("CARGO_PKG_VERSION"));
        Some(Installed {
            binary: directory.join("muster-daemon"),
            directory,
            socket: install::socket_path(&home),
        })
    }

    fn stderr(&self) -> PathBuf {
        self.socket.with_extension("stderr")
    }

    /// The digest of the install that put this build there, which says whether it is the
    /// one this app carries.
    fn stamp(&self) -> PathBuf {
        self.directory.join("installed")
    }
}

/// Dials the daemon through `local_socket`, the forward's end here, and starts the one
/// `installed` names over there if nothing answers.
///
/// `environment` is the daemon's whole environment, and so every pane's starting point there:
/// build it with [`crate::environment::for_far_daemon`] from that machine's own, never pass an
/// ssh session's through.
pub fn ensure_running(
    remote: &impl Far,
    installed: &Installed,
    carried: &Carried,
    local_socket: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<(Reached, Welcome), String> {
    match probe(local_socket) {
        Ok(welcome) => return Ok((Reached::Adopted, welcome)),
        Err(HandshakeError::Unreachable(_)) => {}
        Err(HandshakeError::Stalled(why)) => {
            return Err(format!(
                "the daemon on {} is not answering through the ssh forward: {why}. Its panes \
                 are absent from this window until it answers. Either the connection to {} has \
                 stalled (a VPN that dropped with the ssh master still up), or the daemon there \
                 is stopped or stuck; its log is beside its socket there.",
                remote.host(),
                remote.host()
            ));
        }
        Err(error) => {
            return Err(format!(
                "the daemon on {} would not talk to this app: {error}. Its panes are still \
                 running, and this window cannot show them until a Muster that speaks its \
                 protocol is used.",
                remote.host()
            ));
        }
    }

    let (platform, stamp, executable) = survey(remote, installed)?;
    let payload = carried.payload(remote.host(), &platform)?;
    if !executable || stamp != payload.stamp {
        put_there(remote, installed, &payload)?;
    }

    log::info(
        "daemon.remote.starting",
        fields! {
            "host" => remote.host(),
            "binary" => installed.binary.display(),
            "socket" => installed.socket.display(),
        },
    );
    let marker = crate::start_marker();
    let started = remote.shell(&start_script(installed, &marker, environment))?;
    let pid: u32 = started.trim().parse().map_err(|_| {
        format!(
            "the start script on {} did not say which process it started (it printed {:?}), so \
             its panes are absent from the window. This is likely a bug in the script.",
            remote.host(),
            started.trim()
        )
    })?;

    let deadline = Instant::now() + START_PATIENCE;
    let mut next_exit_check = Instant::now() + EXIT_CHECK_INTERVAL;
    loop {
        if let Ok(welcome) = probe(local_socket) {
            // A rival starter's daemon can take the socket first, and this one then exits.
            let reached =
                if welcome.launch == marker { Reached::Started } else { Reached::Adopted };
            log::info(
                "daemon.remote.started",
                fields! {
                    "host" => remote.host(),
                    "daemon_pid" => welcome.pid,
                    "instance" => welcome.instance,
                    "reached" => format!("{reached:?}"),
                },
            );
            return Ok((reached, welcome));
        }
        let now = Instant::now();
        if now >= next_exit_check {
            next_exit_check = now + EXIT_CHECK_INTERVAL;
            // `|| true`, since the far shell's failure is an error to ssh, and a daemon that is
            // gone is exactly the answer this asks for.
            let alive =
                remote.shell(&format!("kill -0 {pid} 2>/dev/null && echo alive || true"))?;
            // One more dial: a daemon that lost the race exits, and the winner answers.
            if alive.trim() != "alive" && probe(local_socket).is_err() {
                return Err(format!(
                    "the daemon on {} exited before it answered, so its panes are absent from \
                     the window. It said: {}",
                    remote.host(),
                    said(remote, installed, &marker)
                ));
            }
        }
        if now >= deadline {
            return Err(format!(
                "the daemon on {} was started and did not answer within {}s, so its panes are \
                 absent from the window. It said: {}",
                remote.host(),
                START_PATIENCE.as_secs(),
                said(remote, installed, &marker)
            ));
        }
        std::thread::sleep(DIAL_INTERVAL);
    }
}

/// What the machine is, which build is installed there, and whether its daemon can run: one
/// round trip, since every step before a start is a wait somebody sees.
fn survey(remote: &impl Far, installed: &Installed) -> Result<(Platform, String, bool), String> {
    let said = remote.shell(&format!(
        "printf '%s\\n' \"$(uname -sm)\" \"$(cat {} 2>/dev/null)\"; \
         test -x {} && echo executable || echo missing",
        path(&installed.stamp()),
        path(&installed.binary),
    ))?;
    let mut lines = said.lines();
    let uname = lines.next().unwrap_or_default();
    let platform = Platform::from_uname(uname).ok_or_else(|| {
        format!(
            "{} answered `uname -sm` with {uname:?}, which is not a system and a machine, so \
             there is no telling which daemon it runs and its panes are absent from the window.",
            remote.host()
        )
    })?;
    let stamp = lines.next().unwrap_or_default().trim().to_string();
    let executable = lines.next().is_some_and(|line| line.trim() == "executable");
    Ok((platform, stamp, executable))
}

/// Installs `payload` in the version's directory over there, replacing whatever was in it.
fn put_there(remote: &impl Far, installed: &Installed, payload: &Payload) -> Result<(), String> {
    log::info(
        "daemon.remote.installing",
        fields! {
            "host" => remote.host(),
            "build" => payload.build,
            "bytes" => payload.archive.len(),
            "directory" => installed.directory.display(),
        },
    );
    let began = Instant::now();
    remote.shell_on(&install_script(installed, &payload.stamp), &payload.archive).map_err(
        |error| {
            format!(
                "could not install muster-daemon on {host} ({error}), so its panes are absent \
                 from the window and nothing else is affected. Check that {host} has `tar` and \
                 room in {directory}.",
                host = remote.host(),
                directory = installed.directory.display(),
            )
        },
    )?;
    log::info(
        "daemon.remote.installed",
        fields! {
            "host" => remote.host(),
            "build" => payload.build,
            "ms" => began.elapsed().as_millis(),
        },
    );
    Ok(())
}

/// Unpacks the archive on standard input into a directory beside the version's, then swaps it
/// in, so the version's directory is at every moment the whole of one install.
///
/// The staging names carry the shell's pid, so two windows installing at once never write into
/// one directory. If another install's directory lands in place between the check and the
/// move, `mv` puts this one inside it, and it is removed from there.
fn install_script(installed: &Installed, stamp: &str) -> String {
    let directory = path(&installed.directory);
    format!(
        "d={directory}; s=\"$d.placing.$$\"; o=\"$d.old.$$\"; \
         rm -rf \"$s\" && mkdir -p \"$s\" && tar -xf - -C \"$s\" && \
         printf %s {stamp} > \"$s/installed\" && \
         {{ if [ -d \"$d\" ]; then mv \"$d\" \"$o\"; fi; mv \"$s\" \"$d\"; }} && \
         rm -rf \"$o\" \"$d/${{s##*/}}\"",
        stamp = quoted(stamp),
    )
}

/// What the daemon wrote to its stderr file since this start's marker.
fn said(remote: &impl Far, installed: &Installed, marker: &str) -> String {
    let text = remote.shell(&format!("cat {} 2>/dev/null", path(&installed.stderr())));
    match crate::after_marker(&text.unwrap_or_default(), marker) {
        "" => "nothing".to_string(),
        said => said.to_string(),
    }
}

/// The command that starts the daemon over there, returns at once, and prints its pid.
///
/// In a session of its own where the machine has `setsid`, and under `nohup` where it does not
/// (a remote Mac ships no `setsid`), so the daemon outlives the ssh connection that started it.
/// Both exec the daemon, so `$!` is the daemon itself. The brace group keeps the redirects on
/// the daemon: dash execs the last command of a backgrounded list, and without the group ssh
/// would wait on pipes the daemon holds open. The stderr file is appended to, under `marker`,
/// because another start may be writing to it at the same moment, and the daemon repeats
/// `marker` in its welcome so a start knows its own daemon from a rival's. `env -i` starts the daemon
/// with `environment` and nothing of the ssh session that ran the script; it execs too.
pub fn start_script(
    installed: &Installed,
    marker: &str,
    environment: &BTreeMap<String, String>,
) -> String {
    let given: Vec<String> =
        environment.iter().map(|(name, value)| quoted(&format!("{name}={value}"))).collect();
    let command = format!("env -i {} {}", given.join(" "), path(&installed.binary));
    let socket = path(&installed.socket);
    let errors = path(&installed.stderr());
    let directory = path(installed.socket.parent().unwrap_or(Path::new("/")));
    let marker = quoted(marker);
    let daemon = format!("{command} --socket {socket} --launch {marker}");
    format!(
        "mkdir -p {directory} && echo {marker} >> {errors} && \
         if command -v setsid >/dev/null 2>&1; then \
         {{ setsid {daemon} >> {errors} 2>&1 < /dev/null & }}; else \
         {{ nohup {daemon} >> {errors} 2>&1 < /dev/null & }}; fi; echo $!"
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
            directory: PathBuf::from("/home/o'neil/.muster/daemon/0.9.0"),
            binary: PathBuf::from("/home/o'neil/.muster/daemon/0.9.0/muster-daemon"),
            socket: PathBuf::from("/home/o'neil/.muster/daemon/dev-1.sock"),
        };
        let script = start_script(&installed, "--- a marker ---", &BTreeMap::new());
        assert!(script.contains(&quoted("/home/o'neil/.muster/daemon/0.9.0/muster-daemon")));
        assert!(script.contains(&quoted("/home/o'neil/.muster/daemon/dev-1.stderr")));
    }

    /// This machine standing in for the far one: the same shell, no ssh.
    struct Here;

    impl Far for Here {
        fn host(&self) -> &'static str {
            "here"
        }

        fn shell_on(&self, script: &str, input: &[u8]) -> Result<String, String> {
            use std::io::Write as _;
            let mut child = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .map_err(|error| error.to_string())?;
            let mut stdin = child.stdin.take().expect("stdin was piped");
            let input = input.to_vec();
            let writer = std::thread::spawn(move || stdin.write_all(&input));
            let output = child.wait_with_output().map_err(|error| error.to_string())?;
            writer.join().expect("the writer does not panic").map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(format!("the script exited with {}", output.status));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
    }

    /// What `Here` installs from: `daemon` as this Mac's own, with a library and a data
    /// directory beside it.
    fn carrying(daemon: PathBuf, library: PathBuf, data: PathBuf) -> Carried {
        Carried { linux: None, mac: Some(daemon), mac_library: Some(library), data: Some(data) }
    }

    fn installed_in(root: &Path) -> Installed {
        let directory = root.join("installed");
        Installed {
            binary: directory.join("muster-daemon"),
            directory,
            socket: root.join("daemon").join("d.sock"),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let root = PathBuf::from(format!("/tmp/muster-test/r{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn executable(path: &Path, script: &str) {
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }

    /// A daemon that dies at once is noticed by its exit, not by the start's whole patience.
    #[test]
    fn a_remote_daemon_that_dies_at_once_says_why_quickly() {
        let root = scratch("dies");
        let binary = root.join("carried");
        executable(&binary, "#!/bin/sh\necho 'no data directory beside me' >&2\nexit 1\n");
        std::fs::create_dir_all(root.join("data")).unwrap();
        let carried = carrying(binary.clone(), binary, root.join("data"));
        let installed = installed_in(&root);
        let started = Instant::now();
        let error =
            ensure_running(&Here, &installed, &carried, &installed.socket, &BTreeMap::new())
                .unwrap_err();
        // Well inside the start's own 90 seconds, with room for a loaded machine: it takes
        // under a second alone, once took twelve beside the rest of the suite, and a new
        // executable can wait over twenty for macOS to scan it before it runs at all.
        assert!(started.elapsed() < Duration::from_mins(1), "took {:?}", started.elapsed());
        assert!(error.contains("no data directory beside me"), "{error}");
        assert!(error.contains("exited before it answered"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run by the shells a remote might have, it returns at once, prints the daemon's pid and
    /// leaves the daemon running, told where to listen: bash forks a backgrounded list, and dash
    /// execs its last command. macOS ships both, so neither needs a container.
    #[test]
    fn the_start_script_returns_and_leaves_the_daemon_running() {
        let root = scratch("start");
        let told = root.join("told");
        let binary = root.join("muster-daemon");
        executable(
            &binary,
            // Written whole and then renamed, so the test never reads it half written.
            &format!(
                "#!/bin/sh\n{{ echo \"$$ $@\"; env; }} > '{0}.part' && mv '{0}.part' '{0}'\nsleep 3\n",
                told.display()
            ),
        );
        let given = environment(&[("HOME", "/home/o'neil"), ("LANG", "C.UTF-8")]);
        let installed = Installed {
            directory: root.clone(),
            binary,
            socket: root.join("daemon").join("d.sock"),
        };

        for shell in ["/bin/dash", "/bin/bash"] {
            let _ = std::fs::remove_file(&told);
            let started = Instant::now();
            let output = std::process::Command::new(shell)
                .arg("-c")
                .arg(start_script(&installed, "--- a marker ---", &given))
                .env("SSH_CONNECTION", "10.0.0.1 22 10.0.0.2 22")
                .env("SSH_AUTH_SOCK", "/tmp/ssh-XXXX/agent.1")
                .stdout(std::process::Stdio::piped())
                .output()
                .unwrap();
            assert!(output.status.success(), "{shell}: {output:?}");
            assert!(started.elapsed() < Duration::from_secs(5), "{shell} waited on the daemon");
            // The harness's one deadline, since at a load of nineteen the fake daemon has taken
            // over five seconds to write its first line.
            muster_harness::until_file(&told, "the fake daemon to say what it was given");
            let told = std::fs::read_to_string(&told).unwrap_or_default();
            let mut lines = told.lines();
            let printed = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                lines.next().unwrap_or_default(),
                format!(
                    "{} --socket {} --launch --- a marker ---",
                    printed.trim(),
                    installed.socket.display()
                ),
                "{shell}: the pid printed is the daemon's own"
            );
            // What the daemon's own `sh` adds for itself is not what it was given.
            let mut inherited: Vec<&str> = lines
                .filter(|line| !["PWD=", "SHLVL=", "_="].iter().any(|own| line.starts_with(own)))
                .collect();
            inherited.sort_unstable();
            assert_eq!(
                inherited,
                ["HOME=/home/o'neil", "LANG=C.UTF-8"],
                "{shell}: the daemon gets what it was given and nothing of the ssh session"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Mac with no daemon gets this one, with the libghostty-vt it links and its data, is
    /// started from where it was put, and is left alone once it has this build. The far side
    /// is this machine through `sh`, which is a Mac like any other the app attaches.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn a_machine_with_nothing_on_it_is_installed_to_and_then_adopted() {
        let root = scratch("install");
        let carried = carrying(
            muster_harness::built_daemon(),
            muster_vt::library_path().expect("a Mac build loads libghostty-vt from a file"),
            PathBuf::from(muster_harness::DAEMON_DATA),
        );
        let installed = installed_in(&root);
        let given =
            environment(&[("HOME", &root.display().to_string()), ("PATH", "/usr/bin:/bin")]);

        let (reached, started) =
            ensure_running(&Here, &installed, &carried, &installed.socket, &given).unwrap();
        assert_eq!(reached, Reached::Started);
        for placed in ["muster-daemon", "libghostty-vt.dylib", "muster-daemon-data/terminfo"] {
            assert!(installed.directory.join(placed).exists(), "{placed} was not installed");
        }
        let stamp = std::fs::read_to_string(installed.stamp()).unwrap();
        let payload = carried.payload("here", &Platform::from_uname("Darwin arm64").unwrap());
        assert_eq!(stamp, payload.unwrap().stamp, "the install says which build it is");

        let (reached, adopted) =
            ensure_running(&Here, &installed, &carried, &installed.socket, &given).unwrap();
        assert_eq!((reached, adopted.instance), (Reached::Adopted, started.instance));

        // Another build over there is replaced by this one before anything is started.
        crate::launch::stop(&installed.socket, Duration::from_secs(10)).unwrap();
        std::fs::write(installed.stamp(), "another build").unwrap();
        let (reached, _) =
            ensure_running(&Here, &installed, &carried, &installed.socket, &given).unwrap();
        assert_eq!(reached, Reached::Started);
        assert_eq!(std::fs::read_to_string(installed.stamp()).unwrap(), stamp);
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".placing"))
            .collect();
        assert!(leftovers.is_empty(), "a staged install was left behind: {leftovers:?}");

        crate::launch::stop(&installed.socket, Duration::from_secs(10)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
