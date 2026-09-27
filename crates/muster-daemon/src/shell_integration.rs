//! Ghostty's shell integration, injected the way Ghostty injects it, so a pane's prompts carry
//! OSC 133 marks and the cursor, title and PATH features work as they do in Ghostty.
//!
//! Ported from Ghostty's `src/termio/shell_integration.zig` at the pinned commit (MIT; see
//! `NOTICE`), and changed: only bash, zsh and fish (MIP-3, section 3); the scripts are found in
//! the daemon's data directory rather than Ghostty's resources; and a shell that cannot be
//! integrated is simply started as it is, since the daemon only ever starts a shell with the
//! flags it chose itself - Ghostty's handling of `--norc`, `--rcfile` and `-c` has nothing to
//! act on here.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// What makes `shell` load the integration: arguments to put before its own, and variables to
/// set.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Integration {
    pub(crate) arguments: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
}

/// The integration for `shell`, started in `environment`, with the scripts under `scripts`. No
/// integration for a shell Ghostty has none for.
pub(crate) fn for_shell(
    shell: &str,
    environment: &[(OsString, OsString)],
    scripts: &Path,
) -> Integration {
    let get = |name: &str| {
        environment
            .iter()
            .find(|(existing, _)| existing == OsStr::new(name))
            .map(|(_, value)| value.to_string_lossy().into_owned())
    };
    let path = |relative: &str| scripts.join(relative).display().to_string();
    let mut integration = Integration::default();
    let mut set = |name: &str, value: String| integration.environment.push((name.into(), value));

    match shell.rsplit('/').next() {
        // Apple's bash 3.2 at /bin/bash turns off the ENV-in-POSIX-mode startup this relies on,
        // and SIP keeps /bin unwritable, so that path is always Apple's.
        Some("bash") if cfg!(target_os = "macos") && shell == "/bin/bash" => {}
        Some("bash") => {
            // POSIX mode reads $ENV at startup and nothing else; the script then leaves POSIX
            // mode and reads the startup files bash would have.
            if let Some(previous) = get("ENV") {
                set("GHOSTTY_BASH_ENV", previous);
            }
            set("ENV", path("bash/ghostty.bash"));
            set("GHOSTTY_BASH_INJECT", "1".to_string());
            // POSIX mode's history file is ~/.sh_history.
            if get("HISTFILE").is_none()
                && let Some(home) = get("HOME")
            {
                set("HISTFILE", format!("{home}/.bash_history"));
                set("GHOSTTY_BASH_UNEXPORT_HISTFILE", "1".to_string());
            }
            integration.arguments.push("--posix".to_string());
        }
        Some("zsh") => {
            // zsh reads .zshenv from $ZDOTDIR. Ghostty's .zshenv puts the person's ZDOTDIR back and
            // sources their own .zshenv.
            if let Some(previous) = get("ZDOTDIR") {
                set("GHOSTTY_ZSH_ZDOTDIR", previous);
            }
            set("ZDOTDIR", path("zsh"));
        }
        Some("fish") => {
            // fish sources vendor_conf.d from every data directory; the script takes its own
            // back out of XDG_DATA_DIRS once it has run.
            let dir = scripts.display().to_string();
            let data_dirs =
                get("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            set("GHOSTTY_SHELL_INTEGRATION_XDG_DIR", dir.clone());
            set("XDG_DATA_DIRS", format!("{dir}:{data_dirs}"));
        }
        _ => {}
    }
    integration
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPTS: &str = "/data/shell-integration";

    fn integrate(shell: &str, environment: &[(&str, &str)]) -> Integration {
        let environment: Vec<(OsString, OsString)> =
            environment.iter().map(|(name, value)| ((*name).into(), (*value).into())).collect();
        for_shell(shell, &environment, Path::new(SCRIPTS))
    }

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect()
    }

    #[test]
    fn bash_starts_in_posix_mode_with_the_script_as_its_env() {
        let integration = integrate("/usr/bin/bash", &[("HOME", "/home/a")]);
        assert_eq!(integration.arguments, ["--posix"]);
        assert_eq!(
            integration.environment,
            pairs(&[
                ("ENV", "/data/shell-integration/bash/ghostty.bash"),
                ("GHOSTTY_BASH_INJECT", "1"),
                ("HISTFILE", "/home/a/.bash_history"),
                ("GHOSTTY_BASH_UNEXPORT_HISTFILE", "1"),
            ])
        );
    }

    #[test]
    fn bash_keeps_a_previous_env_and_history_file() {
        let integration =
            integrate("bash", &[("ENV", "/etc/mine"), ("HISTFILE", "/h"), ("HOME", "/home/a")]);
        assert_eq!(
            integration.environment,
            pairs(&[
                ("GHOSTTY_BASH_ENV", "/etc/mine"),
                ("ENV", "/data/shell-integration/bash/ghostty.bash"),
                ("GHOSTTY_BASH_INJECT", "1"),
            ])
        );
    }

    #[test]
    fn apples_bash_is_left_alone_on_macos_only() {
        let integrated = integrate("/bin/bash", &[]) != Integration::default();
        assert_eq!(integrated, !cfg!(target_os = "macos"));
    }

    #[test]
    fn zsh_reads_its_startup_from_the_integration_and_remembers_zdotdir() {
        assert_eq!(
            integrate("/bin/zsh", &[]),
            Integration {
                arguments: vec![],
                environment: pairs(&[("ZDOTDIR", "/data/shell-integration/zsh")]),
            }
        );
        assert_eq!(
            integrate("zsh", &[("ZDOTDIR", "/home/a/.config/zsh")]).environment,
            pairs(&[
                ("GHOSTTY_ZSH_ZDOTDIR", "/home/a/.config/zsh"),
                ("ZDOTDIR", "/data/shell-integration/zsh"),
            ])
        );
    }

    #[test]
    fn fish_finds_the_integration_among_its_data_directories() {
        assert_eq!(
            integrate("/opt/homebrew/bin/fish", &[]).environment,
            pairs(&[
                ("GHOSTTY_SHELL_INTEGRATION_XDG_DIR", SCRIPTS),
                ("XDG_DATA_DIRS", "/data/shell-integration:/usr/local/share:/usr/share"),
            ])
        );
        assert_eq!(
            integrate("fish", &[("XDG_DATA_DIRS", "/nix/share")]).environment[1],
            ("XDG_DATA_DIRS".to_string(), "/data/shell-integration:/nix/share".to_string())
        );
    }

    #[test]
    fn other_shells_are_started_as_they_are() {
        for shell in ["/bin/sh", "/bin/dash", "/usr/bin/nu", "tcsh"] {
            assert_eq!(integrate(shell, &[]), Integration::default(), "{shell}");
        }
    }
}
