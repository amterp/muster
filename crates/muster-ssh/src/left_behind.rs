//! Masters an earlier Muster started and never ended.
//!
//! A Muster that exits without ending its masters leaves them running, reparented to launchd,
//! with every forward they carry still up. The one that matters is the reverse forward: it keeps
//! the old window's socket answering on the far machine, so a pane there that asks for its
//! window reaches a window that is gone rather than the one open now (kan a_2YAdjRtMB). A quit
//! ends its own masters, but a crash cannot, so the next Muster to open a tunnel looks for them.
//!
//! What makes one safe to end is its name. Every tunnel's paths carry the pid of the Muster that
//! opened it ([`tunnel_path`]), so a master whose pid no longer runs belongs to nobody. A pid that
//! runs is left alone whatever it is: it may be a second window on this Mac with its own master
//! to the same host, and ending that would drop every one of its panes. A pid reused by some
//! other program leaves a master running, which is the cheaper way to be wrong.

use std::path::Path;

use muster_core::diagnostics::log;
use muster_core::fields;

use crate::tunnel::end_master_at;

/// Where the tunnel named `name` of the Muster with this pid puts the path of this kind.
///
/// `ctl` for the master's control path and `sock` for the forwarded socket. The one spelling of
/// the name, because [`end_left_behind`] reads the pid back out of it.
pub fn tunnel_path(directory: &Path, pid: u32, name: &str, extension: &str) -> String {
    directory.join(format!("muster-{pid}-{name}.{extension}")).to_string_lossy().into_owned()
}

/// Ends every master in `directory` whose Muster is no longer running, and removes its paths.
///
/// Runs subprocesses, one bounded `-O exit` per master found, so a caller that cannot wait a
/// few seconds should run it on a thread of its own.
pub fn end_left_behind(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    let names: Vec<String> =
        entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
    for (pid, name) in left_behind(&names, std::process::id(), running) {
        let control_path = tunnel_path(directory, pid, &name, "ctl");
        let asked = end_master_at(&control_path);
        let _ = std::fs::remove_file(&control_path);
        let _ = std::fs::remove_file(tunnel_path(directory, pid, &name, "sock"));
        log::info(
            "tunnel.left_behind.ended",
            fields! {
                "pid" => pid.to_string(),
                "tunnel" => name,
                "asked" => match asked {
                    Ok(()) => "the master left when its control path asked it to".to_string(),
                    Err(detail) => format!("nothing ended it, and its paths are gone: {detail}"),
                },
            },
        );
    }
}

/// Which control paths among these file names belong to a Muster that is not running, as the pid
/// and the tunnel's name. Never this process's own, which its tunnels look after themselves.
fn left_behind(names: &[String], own: u32, running: impl Fn(u32) -> bool) -> Vec<(u32, String)> {
    names
        .iter()
        .filter_map(|name| owner(name))
        .filter(|(pid, _)| *pid != own && !running(*pid))
        .collect()
}

/// The pid and tunnel name in a control path's file name, as [`tunnel_path`] spells it.
fn owner(file_name: &str) -> Option<(u32, String)> {
    let rest = file_name.strip_prefix("muster-")?.strip_suffix(".ctl")?;
    let (pid, name) = rest.split_once('-')?;
    let pid = pid.parse().ok()?;
    (!name.is_empty()).then(|| (pid, name.to_string()))
}

/// Whether a process with this pid exists. One that exists and is not ours to signal still
/// counts, because the question is whether it might be a Muster, not whether we may end it.
fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return true };
    // SAFETY: signal 0 delivers nothing; it only asks the kernel whether the pid exists and may
    // be signalled.
    let asked = unsafe { libc::kill(pid, 0) };
    asked == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(spelled: &[&str]) -> Vec<String> {
        spelled.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn a_control_path_names_its_muster_and_its_tunnel() {
        assert_eq!(owner("muster-4242-devenv.ctl"), Some((4242, "devenv".to_string())));
        assert_eq!(owner("muster-4242-dev-box.ctl"), Some((4242, "dev-box".to_string())));
    }

    #[test]
    fn nothing_but_a_control_path_is_read_as_one() {
        // A pane's link socket is `muster-<pid>-<n>.sock` in the same directory, and is not a
        // master's to end.
        for name in ["muster-4242-0.sock", "muster-4242-devenv.sock", "muster-x-devenv.ctl"] {
            assert_eq!(owner(name), None, "{name}");
        }
        assert_eq!(owner("muster-4242-.ctl"), None);
    }

    #[test]
    fn only_a_muster_that_is_gone_leaves_a_master_behind() {
        let found = left_behind(
            &names(&["muster-1-devenv.ctl", "muster-2-devenv.ctl", "muster-3-devenv.ctl"]),
            3,
            |pid| pid == 2,
        );
        assert_eq!(
            found,
            [(1, "devenv".to_string())],
            "pid 2 is running, maybe as a second window, and pid 3 is this process"
        );
    }

    #[test]
    fn a_path_is_spelled_the_way_it_is_read_back() {
        let path = tunnel_path(Path::new("/tmp"), 77, "devenv", "ctl");
        assert_eq!(path, "/tmp/muster-77-devenv.ctl");
        let file = Path::new(&path).file_name().and_then(|name| name.to_str());
        assert_eq!(file.and_then(owner), Some((77, "devenv".to_string())));
    }
}
