//! Muster's own daemon, on a machine that is not this one.
//!
//! [`crate::daemon`] is this arrangement for the machine Muster is running on, and everything
//! it says applies here: Muster runs the daemon it pinned rather than whatever is installed,
//! under a herdr session of its own, started and never stopped. What changes is only that the
//! binary has to get there first.
//!
//! **This machine fetches the daemon, and the far one never has to.** Whoever is running
//! Muster demonstrably has web access - they got Muster somehow - while a devenv is often a
//! container or a build box with no route out. So the release asset is downloaded here,
//! verified against the pin here, kept in a cache here, and pushed over the ssh master that is
//! already open. Getting a daemon over there asks nothing of that machine's connectivity.
//!
//! That is a claim about the install and no longer about everything the daemon does afterwards.
//! Muster leaves herdr's `manifest_check` at herdr's own default, which is on, because freezing
//! agent-detection rules is what once made `working` unreachable (`crate::config`, and kan
//! a_2HxSqYtuA) - so the daemon started here goes looking for manifests like any other. What a
//! machine with no route out does about that is herdr's answer rather than Muster's, and has
//! not been measured.
//!
//! Four platforms are pinned and this build carries none of them, which is the other half of
//! the decision: bundling all four would be about 72 MB of app, most of it daemons for machines
//! nobody using that copy will ever attach to.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_ssh::{Platform, Remote, quoted};
use sha2::{Digest, Sha256};

use crate::daemon::{Reached, answers};
use crate::discovery::OWN_SESSION;
use crate::pin::pinned;

/// How long a daemon started over ssh gets to answer.
///
/// Longer than the local ten seconds because every probe is a round trip through the master
/// rather than a connect to a path on this disk, and a devenv is often at the end of a VPN.
/// Same reasoning as locally: giving up early reports "no daemon" about one that is seconds
/// away, and the window that produces is the empty one this path exists to prevent.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// Ensures Muster's own daemon is running over there, installing it if it is not there at all.
///
/// `socket_path` is the *local* end of the tunnel, so "does it answer" is asked exactly the way
/// it is asked of a daemon on this machine - which is what keeps the adopt-before-start rule
/// one rule rather than two. A daemon an earlier Muster left running is holding somebody's
/// agents, and is reused.
///
/// `environment` is the far machine's, as `remote_environment` read it. `cache` is a directory
/// on *this* machine, or nothing when the shell had nowhere to put one - in which case every
/// launch that has to install pays for the download again, which is slow rather than broken.
///
/// `configuration` is the derived herdr config, the same text a daemon on this machine is given.
/// It is placed before anything else happens, including before the daemon is found to be already
/// running: `herdr server` reads its config once at startup, so a file written afterwards is a
/// file that daemon never sees - and one that is already running has to be asked to read again,
/// which needs the file to be there first.
pub fn ensure_running(
    remote: &Remote,
    environment: &BTreeMap<String, String>,
    socket_path: &str,
    cache: Option<&str>,
    configuration: &str,
) -> Result<Reached, String> {
    let pin = pinned()?;
    let home = muster_home(environment).ok_or_else(|| {
        format!(
            "{} answered, and nothing in its environment says where home is - neither \
             MUSTER_HOME nor HOME - so there is nowhere on it to put a daemon. That machine's \
             panes are absent from the window and nothing else is affected. Name an installed \
             daemon's socket in the config file's `socket` key to attach one somebody else \
             started.",
            remote.host(),
        )
    })?;
    let installed = installed_at(&home, &pin.version);
    let config_path = format!("{home}/state/herdr.toml");

    // Before the probe rather than after it, so that both answers below have a current file to
    // work from. Costs one command over a master that is already open.
    remote.place(&config_path, configuration.as_bytes(), "0644")?;
    if answers(socket_path) {
        return Ok(Reached::Adopted);
    }

    if !is_installed(remote, &installed)? {
        let asset = asset_name(&remote.platform()?, remote.host())?;
        let want = pin.checksum(&asset).ok_or_else(|| {
            format!(
                "{} is a {asset}, and Muster's herdr pin carries no checksum for that - so \
                 there is no daemon this build could put on it. That machine's panes are \
                 absent from the window. The pin carries macOS and Linux on aarch64 and \
                 x86_64.",
                remote.host(),
            )
        })?;
        let bytes = acquire(&pin.url(&asset), cache, &pin.version, &asset, want)?;
        log::info(
            "daemon.remote.installing",
            fields! {
                "host" => remote.host(),
                "asset" => asset.clone(),
                "path" => installed.clone(),
                "bytes" => bytes.len().to_string(),
            },
        );
        remote.place(&installed, &bytes, "0755")?;
    }
    link(remote, &home, &installed)?;

    start(remote, &home, &installed, &config_path, socket_path)?;
    Ok(Reached::Started)
}

/// Points a name that never moves at the daemon of the moment.
///
/// A pane's frames come from a herdr CLI the bridge runs on the far machine, and the bridge is
/// told which daemon to ask but not which binary to ask with - so it needs a path it can name
/// without being told, and a version-scoped one is exactly what it cannot name. This is that
/// path, and it is the far machine's version of the `~/.muster/bin` the shell keeps here.
///
/// Refreshed on every attach rather than written once, for the same reason the local `muster`
/// link is: the thing it points at moves when the pin does, and a link left over from an older
/// version is a pane rendered by a daemon nobody pinned.
fn link(remote: &Remote, home: &str, installed: &str) -> Result<(), String> {
    let path = format!("{home}/bin/herdr");
    remote
        .shell(&format!(
            "mkdir -p {} && ln -sf {} {}",
            quoted(&format!("{home}/bin")),
            quoted(installed),
            quoted(&path),
        ))
        .map(|_| ())
}

/// Starts the daemon over there and waits for the forwarded socket to answer.
///
/// The script it sends is [`start_script`], which is where the detaching is explained: ssh
/// waits for a command's output to end and a daemon's never does.
///
/// The output file is the only place a daemon that never bound can say why. herdr opens a log
/// of its own once it is running, so what lands here is what happens before that - a binary for
/// the wrong architecture, a socket path over the `sockaddr_un` limit.
fn start(
    remote: &Remote,
    home: &str,
    binary: &str,
    config_path: &str,
    socket_path: &str,
) -> Result<(), String> {
    let output = format!("{home}/state/herdr.out");
    let script = start_script(home, binary, config_path, &output);
    log::info(
        "daemon.remote.starting",
        fields! {
            "host" => remote.host(),
            "binary" => binary,
            "session" => OWN_SESSION,
            // Whose config decides what a pane runs over there, which is the first question
            // when a devenv pane opens a different shell from the one beside it.
            "config" => config_path,
            "output" => output.clone(),
        },
    );
    remote.shell(&script)?;

    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if answers(socket_path) {
            log::info(
                "daemon.remote.started",
                fields! { "host" => remote.host(), "socket" => socket_path },
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "the daemon on {} did not answer within {}s, so that machine's panes are absent from \
         the window and nothing else is affected. {output} over there is where it said why - a \
         binary for the wrong architecture and a socket path over the ~104 bytes a Unix socket \
         allows both look like this. Deleting {binary} makes the next launch install it again.",
        remote.host(),
        START_TIMEOUT.as_secs(),
    ))
}

/// The script that starts the daemon over there, as the far shell will read it.
///
/// Its own function because the shell grammar in it is the whole of kan a_2HpkpQlZP, and a
/// string is something a test can run. It used to end `... nohup <binary> server >> <out> 2>&1
/// < /dev/null &`, where `&` applies to the whole `&&` list rather than to `nohup` - so a far
/// shell that forks runs the list in a subshell and waits on herdr inside it. The redirect
/// covers herdr's descriptors and not the subshell's, so that subshell holds ssh's stdout and
/// stderr for as long as the daemon lives, and [`Remote::shell`] is a `wait_with_output` that
/// wants end of file on both. Measured against a RHEL-family box: four minutes in `start`, and
/// a Quit that had been queued behind it fired the instant the block cleared.
///
/// Which shell is over there decides whether it happens. bash forks; dash execs the last
/// command of a backgrounded list, so the subshell is replaced by the redirected process and
/// the pipes close. The devenv container's `/bin/sh` is dash, which is why the daemon-backed
/// remote tests cannot see this - `crates/muster-herdr/tests/remote_start_script.rs` runs the
/// script under bash instead.
///
/// The brace group is what fixes it: `{ cmd & }` runs in the current shell, so the redirects
/// land on the daemon and the shell ssh started exits as soon as it has forked one.
///
/// `nohup` rather than `setsid`, which would be tidier and is not portable - a remote may be
/// macOS, which ships no such command - and `nohup` is what the desideratum actually needs: the
/// agents keep working when the connection goes.
pub fn start_script(home: &str, binary: &str, config_path: &str, output: &str) -> String {
    format!(
        "mkdir -p {} && {{ HERDR_SESSION={} HERDR_CONFIG_PATH={} nohup {} server >> {} 2>&1 < \
         /dev/null & }}",
        quoted(&format!("{home}/state")),
        quoted(OWN_SESSION),
        quoted(config_path),
        quoted(binary),
        quoted(output),
    )
}

/// Whether a daemon of this version is already sitting over there.
///
/// Presence at a version-named path is the evidence, and that is a smaller claim than the one
/// the local cache makes: the bytes were verified before they were sent, and [`Remote::place`]
/// renames rather than writing in place, so a path that exists holds a whole verified binary or
/// does not exist. Re-checking the digest over there would mean naming a tool that is spelled
/// `shasum` on one platform and `sha256sum` on another, for a directory that is Muster's own.
fn is_installed(remote: &Remote, path: &str) -> Result<bool, String> {
    // `test` reports absence by exiting non-zero, which `run` reads as a failure - so the
    // question is asked in a way that always succeeds and answers in its output instead.
    let answer = remote.shell(&format!("test -x {} && echo yes || echo no", quoted(path)))?;
    Ok(answer.trim() == "yes")
}

/// The pinned bytes for one platform, from the cache or from the release.
///
/// Verified every time rather than only after a download, for the reason `./dev` gives: a
/// cached file can be replaced or truncated between runs, and the failure would otherwise
/// surface as a daemon on somebody's devenv that will not start.
fn acquire(
    url: &str,
    cache: Option<&str>,
    version: &str,
    asset: &str,
    want: &str,
) -> Result<Vec<u8>, String> {
    let Some(cache) = cache else {
        // Nowhere to keep it, so it is fetched to a temporary that goes away. Slow rather than
        // broken, which is the same shape of answer every other "the shell named nowhere"
        // case gets.
        let scratch = std::env::temp_dir().join(format!("muster-{asset}-{version}"));
        let path = scratch.to_string_lossy().into_owned();
        fetch(url, &path)?;
        let bytes = verified(&path, want, url)?;
        let _ = std::fs::remove_file(&path);
        return Ok(bytes);
    };

    let path = cached_at(cache, version, asset);
    if !std::path::Path::new(&path).exists() {
        fetch(url, &path)?;
    }
    verified(&path, want, url)
}

/// Downloads one asset, through a staged name so a half-written file never looks cached.
///
/// `curl` rather than an HTTP client of Muster's own. This is the one thing in the whole app
/// that touches the network, it runs at most once per platform per pinned version, and the
/// alternative is a dependency tree carried by every build for it.
fn fetch(url: &str, path: &str) -> Result<(), String> {
    let staged = format!("{path}.part");
    let directory = std::path::Path::new(path)
        .parent()
        .ok_or_else(|| format!("{path} names no directory to download into"))?;
    std::fs::create_dir_all(directory).map_err(|error| {
        format!("could not make {} to download the daemon into ({error})", directory.display())
    })?;

    log::info("daemon.remote.fetching", fields! { "url" => url, "path" => path });
    // -f so an error page never lands on disk looking like a binary. The checksum would catch
    // that anyway; this makes the message say "404" rather than "these are not the bytes".
    let ran = std::process::Command::new("curl")
        .args(["-fsSL", "-o", &staged, url])
        .output()
        .map_err(|error| {
            format!(
                "could not run curl to download the daemon ({error}). This is the one step \
                 that needs the network, and it runs on this machine rather than on the \
                 remote. Check that curl is on PATH."
            )
        })?;
    if !ran.status.success() {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "could not download the pinned daemon from {url} ({}). That machine's panes are \
             absent from the window and nothing else is affected. This is the one step that \
             needs the network - the remote's own connectivity is not involved. curl said: {}",
            ran.status,
            String::from_utf8_lossy(&ran.stderr).trim(),
        ));
    }
    std::fs::rename(&staged, path)
        .map_err(|error| format!("could not put the downloaded daemon at {path} ({error})"))
}

/// What a downloaded daemon is judged by: SHA-256, lowercase hex, no separators.
///
/// Its own function because `deps/herdr.pin` spells its checksums this way, so the algorithm
/// and the spelling are part of the pin's contract rather than this file's choice.
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The file's bytes, once they are the bytes the pin names.
///
/// A mismatch removes the file and refuses. Refuses rather than warns because the whole point
/// of the pin is that the daemon Muster runs is the daemon its corpus was recorded against, and
/// a warning nobody reads turns that into a preference.
fn verified(path: &str, want: &str, url: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read the downloaded daemon at {path} ({error})"))?;
    let found = digest(&bytes);
    if found != want {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "{path} is not the herdr this build pins, so nothing was installed and that \
             machine's panes are absent from the window.\n  expected {want}\n  found    \
             {found}\nIt has been removed, so the next launch downloads it again from {url}. \
             If it happens twice, the release has been republished and the pin has to move \
             deliberately."
        ));
    }
    Ok(bytes)
}

/// Which release asset a machine saying this would run.
///
/// Strict on both halves, unlike `./dev`'s version of this, and deliberately: `./dev` runs on
/// the machine it is asking about and can treat "not Darwin" as Linux, while this is asking
/// about somebody else's. Sending an x86_64 Linux binary to a FreeBSD box on that reasoning
/// would fail as a daemon that will not start, three layers from the guess that caused it.
pub fn asset_name(platform: &Platform, host: &str) -> Result<String, String> {
    let system = match platform.system.as_str() {
        "Darwin" => "macos",
        "Linux" => "linux",
        other => {
            return Err(format!(
                "{host} says it is running {other}, and Muster pins a herdr for macOS and Linux \
                 only - so there is no daemon this build could put on it. That machine's panes \
                 are absent from the window. Install a herdr over there yourself and name its \
                 socket in the config file's `socket` key."
            ));
        }
    };
    let machine = match platform.machine.as_str() {
        "arm64" | "aarch64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        other => {
            return Err(format!(
                "{host} says its machine is {other}, and Muster pins a herdr for aarch64 and \
                 x86_64 only - so there is no daemon this build could put on it. That machine's \
                 panes are absent from the window. Install a herdr over there yourself and name \
                 its socket in the config file's `socket` key."
            ));
        }
    };
    Ok(format!("herdr-{system}-{machine}"))
}

/// Where a downloaded asset is kept on this machine.
///
/// Under Muster's own home rather than in an XDG cache tree, because one directory holds
/// everything Muster owns and `MUSTER_HOME` moves the lot - which also means a test running
/// under a scratch home gets a scratch cache without asking for one.
///
/// Named for the asset as well as the version, because one Muster attached to two devenvs of
/// different architectures needs both at once.
pub fn cached_at(cache: &str, version: &str, asset: &str) -> String {
    format!("{cache}/herdr/{version}/{asset}/herdr")
}

/// Where the config Muster writes for the far machine's daemon lands.
///
/// The far machine's version of `~/.muster/state/herdr.toml`, and public because the seam has to
/// remember it: a setting changed while the window is open is re-placed here and the daemon is
/// asked to read again, exactly as it is for a daemon on this machine.
pub fn configuration_path(environment: &BTreeMap<String, String>) -> Option<String> {
    Some(format!("{}/state/herdr.toml", muster_home(environment)?))
}

/// Where the daemon goes on the far machine.
pub fn installed_at(home: &str, version: &str) -> String {
    format!("{home}/herdr/{version}/herdr")
}

/// Where the far machine keeps everything of Muster's.
///
/// A second copy of the rule the shell answers here (`Sources/MusterMac/MusterHome.swift`), and
/// the one place a second copy is unavoidable: only the shell can ask an OS a question, and
/// this is a question about an OS the shell is not running on.
pub fn muster_home(environment: &BTreeMap<String, String>) -> Option<String> {
    let lookup = |name: &str| environment.get(name).filter(|value| !value.is_empty());
    if let Some(explicit) = lookup("MUSTER_HOME") {
        return Some(explicit.trim_end_matches('/').to_string());
    }
    Some(format!("{}/.muster", lookup("HOME")?.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(system: &str, machine: &str) -> Platform {
        Platform { system: system.to_string(), machine: machine.to_string() }
    }

    /// The mapping itself is stated in corpus/conformance/remote-daemon.json. This is the
    /// other direction, and it belongs here rather than there because it is a claim about
    /// this repo's own pin file rather than a rule a second implementation would obey.
    #[test]
    fn every_asset_the_pin_carries_is_one_some_platform_names() {
        let pin = pinned().expect("the pin should parse");
        let named: Vec<String> =
            [("Darwin", "arm64"), ("Darwin", "x86_64"), ("Linux", "aarch64"), ("Linux", "x86_64")]
                .iter()
                .map(|(system, machine)| {
                    asset_name(&platform(system, machine), "host").expect("all four are pinned")
                })
                .collect();
        for asset in pin.checksums.keys() {
            assert!(
                named.contains(asset),
                "the pin carries {asset}, which no platform this maps would ask for"
            );
        }
    }

    /// The two below stay in Rust while the rest of this module's cases moved to the corpus.
    /// They read and delete a real file, and the refusal quotes a path and a URL that differ
    /// per run, so neither is a pure function over stated inputs. What the corpus states is
    /// the part that is: the digest these compare.
    #[test]
    fn bytes_that_are_not_the_pinned_ones_are_removed_and_refused() {
        let path = std::env::temp_dir().join(format!("muster-verify-{}", std::process::id()));
        std::fs::write(&path, b"not a daemon").expect("the scratch file should write");
        let name = path.to_string_lossy().into_owned();

        let refusal = verified(&name, &"0".repeat(64), "https://example.invalid/herdr")
            .expect_err("these are not the pinned bytes");
        assert!(refusal.contains("expected"), "the refusal should show both digests");
        assert!(
            !path.exists(),
            "a file that failed its checksum should be gone, so the next launch fetches again"
        );
    }

    #[test]
    fn bytes_that_are_the_pinned_ones_come_back() {
        let path = std::env::temp_dir().join(format!("muster-verify-ok-{}", std::process::id()));
        std::fs::write(&path, b"herdr").expect("the scratch file should write");
        let name = path.to_string_lossy().into_owned();
        let want = digest(b"herdr");

        let read = verified(&name, &want, "https://example.invalid/herdr")
            .expect("these are the bytes it was told to expect");
        assert_eq!(read, b"herdr");
        let _ = std::fs::remove_file(&path);
    }
}
