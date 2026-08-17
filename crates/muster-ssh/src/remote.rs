use std::io::Write;
use std::process::{Command, Stdio};

/// The machine at the other end of a master somebody is already holding open.
///
/// Everything here rides that connection with `-S`, so nothing pays a handshake. The master
/// belongs to a [`crate::Tunnel`]; this is a handle to the far machine rather than to the
/// connection, and it is a plain value so that a caller can keep one without keeping the
/// tunnel borrowed.
///
/// Transport only, like the rest of this crate. What gets copied over there and what gets run
/// is the caller's vocabulary, not this one's.
#[derive(Debug, Clone)]
pub struct Remote {
    host: String,
    control_path: String,
}

/// What the far machine says it is, in `uname`'s words.
///
/// Left in `uname`'s spelling rather than mapped to anything, because what the pair means is
/// the caller's question - a release asset name here, and something else to whoever asks next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub system: String,
    pub machine: String,
}

impl Remote {
    pub fn over(host: &str, control_path: &str) -> Remote {
        Remote { host: host.to_string(), control_path: control_path.to_string() }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// Runs one command over the master and hands back what it printed.
    ///
    /// The arguments are joined with spaces by ssh and parsed again by the far shell, so
    /// anything that could hold a space or a quote has to arrive already quoted - see
    /// [`quoted`]. That is a property of ssh rather than a choice here, and it is why this
    /// takes an argument vector it does not itself quote: a caller building a path knows
    /// whether it came from a person.
    pub fn run(&self, argv: &[&str]) -> Result<String, String> {
        self.run_on(argv, &[])
    }

    /// The same, with bytes on the command's standard input.
    fn run_on(&self, argv: &[&str], input: &[u8]) -> Result<String, String> {
        let mut child = Command::new("ssh")
            .args(["-S", &self.control_path, "-o", "BatchMode=yes", &self.host])
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "could not run ssh to reach {} ({error}). Check that ssh is on PATH.",
                    self.host
                )
            })?;
        // Taken rather than borrowed, so the pipe is closed before the output is waited on. A
        // command reading its input would otherwise wait for an end that never comes, and this
        // would wait for it - each holding the other still.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("ssh to {} would not take an input pipe", self.host))?;
        let written = stdin.write_all(input).and_then(|()| stdin.flush());
        drop(stdin);

        let output = child
            .wait_with_output()
            .map_err(|error| format!("ssh to {} would not finish ({error})", self.host))?;
        written.map_err(|error| {
            format!(
                "ssh to {} stopped taking input part-way through ({error}), so whatever was \
                 being sent arrived truncated. Its own message: {}",
                self.host,
                String::from_utf8_lossy(&output.stderr).trim(),
            )
        })?;
        if !output.status.success() {
            return Err(format!(
                "`{}` on {} failed ({}). Its own message: {}",
                argv.join(" "),
                self.host,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Runs a shell script over there.
    ///
    /// The script is quoted here, so a caller writes it as it would type it - but the paths
    /// *inside* it still need [`quoted`] of their own, because the far shell parses them once
    /// this outer layer is stripped.
    pub fn shell(&self, script: &str) -> Result<String, String> {
        self.shell_on(script, &[])
    }

    /// The same, with bytes on the script's standard input.
    pub fn shell_on(&self, script: &str, input: &[u8]) -> Result<String, String> {
        self.run_on(&["sh", "-c", &quoted(script)], input)
    }

    /// Puts these bytes at this path over there, and makes them executable if asked.
    ///
    /// Through a staged sibling and a rename, for the same reason the daemon's config file is
    /// written that way here: a copy that is interrupted half-way leaves a truncated file, and
    /// a truncated daemon is a machine that looks installed and will not start. The rename is
    /// the step that makes the path appear complete or not at all.
    ///
    /// `path` must be absolute. A tilde would need the far shell to expand it, which is exactly
    /// what the quoting below prevents - callers resolve it from the environment they already
    /// asked the far end for.
    pub fn place(&self, path: &str, bytes: &[u8], mode: &str) -> Result<(), String> {
        let directory = path.rsplit_once('/').map_or(".", |(parent, _)| parent);
        let staged = format!("{path}.placing");
        let script = format!(
            "mkdir -p {} && cat > {} && chmod {mode} {} && mv -f {} {}",
            quoted(directory),
            quoted(&staged),
            quoted(&staged),
            quoted(&staged),
            quoted(path),
        );
        self.shell_on(&script, bytes).map(|_| ())
    }

    /// What the far machine is, so a caller can work out what to send it.
    pub fn platform(&self) -> Result<Platform, String> {
        let answer = self.run(&["uname", "-sm"])?;
        let said = answer.trim();
        let (system, machine) = said.split_once(' ').ok_or_else(|| {
            format!(
                "{} answered `uname -sm` with {said:?}, which is not a system and a machine. \
                 There is no way to tell what binary that host would run.",
                self.host,
            )
        })?;
        Ok(Platform { system: system.trim().to_string(), machine: machine.trim().to_string() })
    }
}

/// One shell word, whatever is in it.
///
/// Needed twice over for anything sent through [`Remote::run`]: ssh hands the far shell a
/// command line rather than an argument vector, so a script arrives having been parsed once,
/// and the paths inside it are parsed a second time when the script runs.
///
/// Single quotes because they are the only quoting a POSIX shell does not look inside. A quote
/// within the text ends the run, escapes itself outside it, and opens a new one, which is the
/// `'\''` below.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_word_is_quoted_whole() {
        assert_eq!(quoted("/home/dev/.muster"), "'/home/dev/.muster'");
    }

    #[test]
    fn a_space_survives_because_the_quotes_do() {
        assert_eq!(quoted("/home/a b/x"), "'/home/a b/x'");
    }

    #[test]
    fn a_quote_closes_and_reopens_rather_than_escaping() {
        assert_eq!(quoted("it's"), r"'it'\''s'");
    }

    #[test]
    fn quoting_a_quoted_script_survives_the_second_parse() {
        // What ssh actually sends: the script has already been quoted once for the shell that
        // runs it, and is quoted again for the shell that reads the command line.
        let script = format!("cat > {}", quoted("/tmp/x"));
        assert_eq!(quoted(&script), r"'cat > '\''/tmp/x'\'''");
    }
}
