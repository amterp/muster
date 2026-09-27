//! Where an install's daemon listens.
//!
//! One daemon runs per machine per install (MIP-3, section 1). An install is a build that ships
//! as one unit: a release, or one checkout's development build. Each gets its own socket, so a
//! development build never adopts the release daemon, two working trees never adopt each other's,
//! and a release adopts the release daemon already running after an upgrade - which is what keeps
//! an upgrade from ending every agent.
//!
//! The name is fixed when this crate is built (`build.rs`), so the daemon and every client built
//! beside it agree on it without being told. `MUSTER_INSTALL` names it for a build that ships;
//! otherwise it is `dev-` and a hash of the checkout's path. Tests never use it: they start a
//! daemon on a socket of their own.

use std::path::{Path, PathBuf};

/// This build's install.
pub const INSTALL: &str = env!("MUSTER_DAEMON_INSTALL");

/// The socket this build's daemon listens on, under a Muster home.
pub fn socket_path(muster_home: &Path) -> PathBuf {
    muster_home.join("daemon").join(format!("{INSTALL}.sock"))
}

/// Where Muster keeps everything of its own on this machine: `MUSTER_HOME`, else `~/.muster`.
///
/// The rule `docs/configuration.md` states and the shell applies (`MusterHome.swift`), asked of
/// whatever environment the caller has. `None` when neither variable says anything.
pub fn muster_home(lookup: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| lookup(name).filter(|value| !value.is_empty());
    if let Some(explicit) = set("MUSTER_HOME") {
        return Some(PathBuf::from(explicit));
    }
    Some(Path::new(&set("HOME")?).join(".muster"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_is_a_name_a_file_can_have() {
        assert!(!INSTALL.is_empty());
        assert!(INSTALL.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    #[test]
    fn the_socket_is_named_for_the_install_under_the_home() {
        let path = socket_path(Path::new("/home/someone/.muster"));
        assert_eq!(path, Path::new("/home/someone/.muster/daemon").join(format!("{INSTALL}.sock")));
    }

    #[test]
    fn muster_home_wins_over_home_and_an_empty_one_does_not_count() {
        let environment = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_string())
            }
        };
        assert_eq!(
            muster_home(environment(&[("MUSTER_HOME", "/elsewhere"), ("HOME", "/home/a")])),
            Some(PathBuf::from("/elsewhere"))
        );
        assert_eq!(
            muster_home(environment(&[("MUSTER_HOME", ""), ("HOME", "/home/a")])),
            Some(PathBuf::from("/home/a/.muster"))
        );
        assert_eq!(muster_home(environment(&[])), None);
    }
}
