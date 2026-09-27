//! `muster-daemon ssh`: Muster's port of `ghostty +ssh`, which Ghostty's shell integration runs
//! from its `ssh` wrapper when a pane's `ssh-terminfo` and `ssh-env` features are on
//! (`packaging/muster-daemon-data/bin/ghostty` hands it here).
//!
//! It does what Ghostty's does (`src/cli/ssh.zig` at the pin): unless the host is known to have
//! it already, installs the xterm-ghostty entry in the host's `~/.terminfo` over one connection
//! of its own, then runs ssh with `TERM` set to xterm-ghostty if that worked and xterm-256color
//! if not, forwarding the terminal's name and colors. A host that took the entry is remembered,
//! once the ssh run ends well, in a file of Muster's rather than Ghostty's.

use std::io::Write;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};

use muster_daemon_proto::install;

const USAGE: &str = "usage: muster-daemon ssh [--terminfo-source FILE] [--forward-env=BOOL] \
    [--terminfo=BOOL] [--cache=BOOL] [--verbose] -- SSH-ARGUMENTS...\n\n\
    Runs ssh as Ghostty's `ghostty +ssh` does, for a pane's `ssh` wrapper: installs the \
    xterm-ghostty terminfo entry on the host unless it is known to have it, then runs ssh with \
    TERM set to xterm-ghostty, or xterm-256color when the entry could not be installed.";

/// What Ghostty's install runs on the host: nothing if the entry is there already, and
/// otherwise `tic` into `~/.terminfo`, from the source on stdin.
const INSTALL: &str = "infocmp xterm-ghostty >/dev/null 2>&1 && exit 0\n\
    command -v tic >/dev/null 2>&1 || exit 1\n\
    mkdir -p ~/.terminfo 2>/dev/null && tic -x - 2>/dev/null && exit 0\n\
    exit 1";

const FALLBACK: &str = "xterm-256color";
const TERM: &str = crate::spawn::TERM;

#[derive(Debug, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "Ghostty's own +ssh flags, one each")]
struct Options {
    source: Option<PathBuf>,
    forward_env: bool,
    terminfo: bool,
    cache: bool,
    verbose: bool,
    ssh: Vec<String>,
}

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> ExitCode {
    let options = match parse(arguments) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(problem) => {
            eprintln!("muster-daemon ssh: {problem}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let cache = options
        .cache
        .then(|| install::muster_home(|name| std::env::var(name).ok()))
        .flatten()
        .map(|home| home.join("state").join("ssh-terminfo"));
    let (term, to_cache) = session(&options, cache.as_deref());

    let mut ssh = Command::new("ssh");
    if options.forward_env {
        ssh.args(["-o", &format!("SetEnv=TERM={term}")]);
        for name in ["COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"] {
            ssh.args(["-o", &format!("SendEnv={name}")]);
        }
    }
    ssh.args(&options.ssh);
    // The terminal's interrupt and quit are ssh's to act on while it runs, as with system(3):
    // this process ignores them, and ssh gets the defaults back.
    // SAFETY: signal is async-signal-safe, and the closure calls nothing else.
    unsafe {
        ssh.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }
    let status = match ssh.status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("muster-daemon ssh: could not run ssh: {error}");
            return ExitCode::FAILURE;
        }
    };
    if status.success()
        && let (Some(cache), Some(host)) = (&cache, to_cache)
    {
        remember(cache, &host);
    }
    ExitCode::from(exit_code(status))
}

/// The terminal ssh is to say it is, and the host to remember once ssh ends well.
fn session(options: &Options, cache: Option<&Path>) -> (&'static str, Option<String>) {
    if !options.terminfo {
        return (FALLBACK, None);
    }
    let Some(host) = destination(&options.ssh) else {
        warn("could not resolve the ssh destination; not installing terminfo");
        return (FALLBACK, None);
    };
    if cache.is_some_and(|cache| remembered(cache, &host)) {
        return (TERM, None);
    }
    let Some(source) = options.source.as_deref().and_then(|path| std::fs::read(path).ok()) else {
        warn("the xterm-ghostty entry's source is missing from muster-daemon-data");
        return (FALLBACK, None);
    };
    eprintln!("Setting up xterm-ghostty terminfo on {host}...");
    if let Err(problem) = install(&options.ssh, &source, options.verbose) {
        warn(&format!("failed to install terminfo: {problem}"));
        return (FALLBACK, None);
    }
    (TERM, Some(host))
}

fn warn(what: &str) {
    eprintln!("Warning: {what}");
}

/// `user@hostname`, as `ssh -G` resolves the arguments.
fn destination(ssh: &[String]) -> Option<String> {
    let output = Command::new("ssh").arg("-G").args(ssh).stderr(Stdio::null()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value = |key: &str| {
        text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(' ').map(str::to_string))
    };
    let (user, host) = (value("user")?, value("hostname")?);
    (!user.is_empty() && !host.is_empty()).then(|| format!("{user}@{host}"))
}

/// Pipes the entry's source to [`INSTALL`] on the host, over a connection that is its own
/// master and ends with it, so no socket is left behind.
fn install(ssh: &[String], source: &[u8], verbose: bool) -> Result<(), String> {
    let control = std::env::temp_dir().join(format!("muster-ssh-{}", std::process::id()));
    let mut child = Command::new("ssh")
        .args(["-o", "ControlMaster=yes", "-o", "ControlPersist=no"])
        .args(["-o", &format!("ControlPath={}", control.display())])
        .args(ssh)
        .arg(INSTALL)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(if verbose { Stdio::inherit() } else { Stdio::null() })
        .spawn()
        .map_err(|error| format!("could not run ssh: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source);
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("ssh exited {}", exit_code(status))) }
}

fn remembered(cache: &Path, host: &str) -> bool {
    std::fs::read_to_string(cache).is_ok_and(|hosts| hosts.lines().any(|line| line == host))
}

fn remember(cache: &Path, host: &str) {
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(cache);
    if let Ok(mut file) = file {
        let _ = writeln!(file, "{host}");
    }
}

/// ssh's exit code, or 128 plus the signal that ended it, as a shell reports it.
fn exit_code(status: ExitStatus) -> u8 {
    let code = status.code().or_else(|| status.signal().map(|signal| 128 + signal));
    u8::try_from(code.unwrap_or(1)).unwrap_or(1)
}

fn parse(arguments: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut options = Options {
        source: None,
        forward_env: true,
        terminfo: true,
        cache: true,
        verbose: false,
        ssh: Vec::new(),
    };
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        let (flag, value) =
            argument.split_once('=').map_or((argument.as_str(), None), |(f, v)| (f, Some(v)));
        let boolean = |value: Option<&str>| match value {
            None | Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(other) => Err(format!("{flag}={other} is not true or false")),
        };
        match flag {
            "--" => {
                options.ssh = arguments.by_ref().collect();
                break;
            }
            "--terminfo-source" => {
                let path = value.map(str::to_string).or_else(|| arguments.next());
                options.source = Some(PathBuf::from(path.ok_or("--terminfo-source needs a file")?));
            }
            "--forward-env" => options.forward_env = boolean(value)?,
            "--terminfo" => options.terminfo = boolean(value)?,
            "--cache" => options.cache = boolean(value)?,
            "--verbose" => options.verbose = boolean(value)?,
            "--help" | "-h" => return Ok(None),
            other => return Err(format!("{other} is not an option; ssh's own go after --")),
        }
    }
    if options.ssh.is_empty() {
        return Err("no ssh arguments were given".to_string());
    }
    Ok(Some(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Result<Option<Options>, String> {
        parse(arguments.iter().map(|argument| (*argument).to_string()))
    }

    #[test]
    fn the_wrappers_flags_and_sshs_own_arguments_are_told_apart() {
        let options =
            parsed(&["--forward-env=false", "--", "-p", "2222", "host", "ls"]).unwrap().unwrap();
        assert!(!options.forward_env && options.terminfo && options.cache);
        assert_eq!(options.ssh, ["-p", "2222", "host", "ls"]);
        assert!(parsed(&["--terminfo=maybe", "--", "host"]).unwrap_err().contains("maybe"));
        assert!(parsed(&["--",]).unwrap_err().contains("no ssh arguments"));
        assert!(parsed(&["host"]).unwrap_err().contains("after --"));
    }
}
