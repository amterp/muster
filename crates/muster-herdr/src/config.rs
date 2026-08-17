//! What Muster tells its daemon, in the daemon's own config format.
//!
//! The backend's half of the arrangement the renderer already has. libghostty has no setter,
//! so the shell writes a derived file and hands over its path; herdr reads a config file at
//! startup and has no setter either, so the core writes one and names it with
//! `HERDR_CONFIG_PATH`. One mechanism per dependency, and neither dependency's vocabulary
//! escapes its adapter: this is the only function in Muster that knows what a herdr config
//! key is called, the way `ghosttyConfiguration` is the only one that knows a ghostty key.
//!
//! **The file is written even when nothing is configured**, which is the one place this
//! differs from the renderer's. An unconfigured appearance produces no file at all, because
//! every value in it is somebody's preference. An unconfigured Muster still has an opinion
//! here: a daemon it pinned by checksum and shipped inside its own bundle does not go looking
//! for its own updates.

use muster_core::config::{Panes, ShellMode};

/// Puts the derived config where the daemon will look, and says what it wrote.
///
/// Through a temporary beside it rather than straight over the top, which matters more here
/// than it does for the arrangement file: herdr re-reads this on its own schedule, and a
/// half-written file is not an error over there - it parses as far as it can, keeps
/// defaults for the rest, and hands a pane settings nobody chose.
///
/// The text is returned so the caller can compare it against the last one and skip a daemon
/// that has nothing to re-read.
pub fn write_configuration(path: &str, panes: &Panes) -> Result<String, String> {
    let text = configuration_text(panes);
    let file = std::path::PathBuf::from(path);
    let staged = file.with_extension("writing");
    std::fs::create_dir_all(file.parent().unwrap_or(std::path::Path::new(".")))
        .and_then(|()| std::fs::write(&staged, &text))
        .and_then(|()| std::fs::rename(&staged, &file))
        .map_err(|error| error.to_string())?;
    Ok(text)
}

/// One derived herdr config, as the bytes a daemon will read.
///
/// The same text wherever it lands, which is the whole shape of this: a person writes one
/// setting and every daemon Muster starts runs it, whichever machine that daemon is on. What
/// differs per machine is the file it goes in, and that is the caller's problem - written here
/// for a daemon on this one, and sent over an ssh master for a daemon on another.
pub fn configuration_text(panes: &Panes) -> String {
    herdr_configuration(panes).join("\n") + "\n"
}

/// One derived herdr config, as lines.
///
/// Lines rather than a blob so the corpus can hold it as something a reviewer reads, which is
/// the same reason a config case's `given.file` is a list.
///
/// `new_cwd` is deliberately absent. herdr's own default is to follow the pane being split
/// from, which is the behaviour `muster_core::intent`'s `SplitPane` already documents Muster
/// relying on - so naming it here would be Muster restating a default it agrees with, and the
/// day herdr changes that default is the day this should notice rather than silently keep the
/// old one.
pub fn herdr_configuration(panes: &Panes) -> Vec<String> {
    let mut lines = vec![
        "# Written by Muster from its own config file. Rewritten on every launch, so an edit".to_string(),
        "# here is lost - the settings are `scrollback_bytes` and `[shell]` in ~/.muster/config.toml.".to_string(),
    ];

    if panes.shell.command.is_some() || panes.shell.mode != ShellMode::Auto {
        lines.push(String::new());
        lines.push("[terminal]".to_string());
        if let Some(command) = &panes.shell.command {
            lines.push(format!("default_shell = {}", quoted(command)));
        }
        // `auto` is herdr's own default, so saying it would be Muster pinning a default it
        // has no opinion about.
        if panes.shell.mode != ShellMode::Auto {
            lines.push(format!("shell_mode = {}", quoted(spell(panes.shell.mode))));
        }
    }

    if let Some(bytes) = panes.scrollback_bytes {
        lines.push(String::new());
        lines.push("[advanced]".to_string());
        lines.push(format!("scrollback_limit_bytes = {bytes}"));
    }

    // Unconditional, and the reason this file is written even when nothing was asked for.
    // Both default to true in herdr, so a daemon Muster pinned by version and checksum,
    // downloaded and verified by `./dev` and staged inside the app bundle, would otherwise
    // check for its own updates. Pinning it is what makes a green suite a statement about
    // the daemon the corpus was recorded against, and an update check is the one thing that
    // can move it off that pin without anybody asking.
    lines.push(String::new());
    lines.push("[update]".to_string());
    lines.push("version_check = false".to_string());
    lines.push("manifest_check = false".to_string());

    lines
}

/// Muster's word for how a shell starts, in herdr's spelling.
///
/// They agree today, which is worth going through a function anyway: the vocabulary is
/// Muster's and the spelling is the daemon's, and a replacement backend that called it
/// something else would change this line and nothing above it.
fn spell(mode: ShellMode) -> &'static str {
    match mode {
        ShellMode::Auto => "auto",
        ShellMode::Login => "login",
        ShellMode::NonLogin => "non_login",
    }
}

/// A TOML basic string.
///
/// Hand-rolled rather than `{:?}`, whose escapes are Rust's: it spells an escape character
/// `\u{1b}`, which TOML does not read. Only a quote and a backslash need escaping here,
/// because the core refuses a control character in a shell name at the point somebody can
/// still be told about it.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
