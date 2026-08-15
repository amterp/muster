//! Configuration, as a file.
//!
//! Parsing only, and deliberately. Reading a file is I/O, so it happens at the edge from a
//! path the shell chose - the same division [`crate::mirror`]'s socket discovery already
//! uses by taking an environment map rather than reading one. What is left here is text in
//! and records out, which is what makes configuration answerable to the corpus like
//! everything else this core decides.
//!
//! TOML rather than the JSON already in the tree, because this is the one file a person
//! hand-edits and JSON has nowhere to put a comment.
//!
//! A file is applied whole or not at all. Keeping the daemons that parsed and dropping the
//! one that did not would leave a window whose contents depend on which line had the typo,
//! and the caller's fallback - find the local daemon the way herdr's own client would - is
//! close enough to what anyone wants that the partial answer buys nothing.
//!
//! Unknown keys are refused rather than ignored. Ignoring them is what herdr's socket API
//! does, and the cost is a `target_pane_id` misspelled as `pane_id` that silently means
//! something else (`docs/observations/herdr-0.8.0.md` section 6). A typo in a file someone
//! typed deserves a sentence naming it, not a daemon that never appears.

use toml::Value;

use crate::input::{Action, Bindings};

use crate::composition::{Daemon, DaemonId, Endpoint};

/// Everything a config file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// In the order the file lists them, which is the order their regions are laid out.
    pub daemons: Vec<Daemon>,
    /// Which chord asks for which of Muster's own actions, with the file's answers over the
    /// defaults. A file that names none leaves every default in place.
    pub bindings: Bindings,
}

/// The keys a `[[daemon]]` block may carry.
const DAEMON_KEYS: [&str; 4] = ["id", "socket", "host", "ssh_options"];

/// The keys the file itself may carry.
const ROOT_KEYS: [&str; 2] = ["daemon", "keymap"];

/// Reads a config file's text.
///
/// The refusal is prose rather than an error type, for the reason the seam's `Failure` is:
/// there is nothing here a caller can usefully branch on, and the only thing to do with a
/// bad config is tell whoever wrote it which line to look at.
pub fn parse(text: &str) -> Result<Config, String> {
    // The parser's own message is appended rather than reworded, because it carries the line
    // and the column and nothing here could reconstruct either.
    let root: Value = toml::from_str(text).map_err(|error| {
        format!(
            "the config file is not valid TOML, so none of it was applied and Muster is \
             attached to whatever it can find for itself rather than to what this file \
             names. The parser says: {error}"
        )
    })?;
    let root = root.as_table().ok_or_else(|| {
        "the config file's top level is not a table, which no TOML document should be able \
         to produce. None of it was applied. This is a bug in the parser rather than in the \
         file."
            .to_string()
    })?;
    known_keys(root.keys(), &ROOT_KEYS, "the config file")?;

    let mut daemons: Vec<Daemon> = Vec::new();
    for (index, block) in daemon_blocks(root)?.iter().enumerate() {
        let daemon = read_daemon(block, index)?;
        if let Some(clash) = daemons.iter().find(|held| held.id == daemon.id) {
            return Err(format!(
                "two daemons are called `{}`, and a name is how everything else refers to \
                 one - a region, a log line, a pane that belongs to it. None of the file was \
                 applied. Rename one: the first is {}, the second is {}.",
                daemon.id,
                describe(&clash.endpoint),
                describe(&daemon.endpoint),
            ));
        }
        daemons.push(daemon);
    }
    Ok(Config { daemons, bindings: read_keymap(root)? })
}

/// The `[keymap]` block, over the defaults.
///
/// Partial by design: a file that names one action rebinds one action. Requiring all fifteen
/// to change one is a file nobody edits twice, and it would silently drop an action the day
/// Muster grew a sixteenth.
///
/// An empty chord unbinds outright, which is different from not mentioning it. Somebody who
/// wants ⌘W back for closing the window has to be able to say so, and the alternative is
/// binding it to a chord nobody presses and hoping.
fn read_keymap(root: &toml::Table) -> Result<Bindings, String> {
    let mut bindings = Bindings::default();
    let Some(value) = root.get("keymap") else {
        return Ok(bindings);
    };
    let block = value.as_table().ok_or_else(|| {
        format!(
            "`keymap` in the config file is {}, and it has to be a table of actions - \
             `[keymap]` with a line like `split_right = \"cmd+d\"` under it. None of the \
             file was applied.",
            described(value)
        )
    })?;

    for (name, chord) in block {
        let action = Action::parse(name).ok_or_else(|| {
            format!(
                "`{name}` in the config file's [keymap] is not something Muster does, so \
                 none of the file was applied. What it does: {}.",
                Action::ALL.map(Action::as_str).join(", ")
            )
        })?;
        let chord = chord.as_str().ok_or_else(|| {
            format!(
                "`{name}` in the config file's [keymap] is {}, and a binding is a string like \
                 `\"cmd+shift+d\"`. None of the file was applied.",
                described(chord)
            )
        })?;
        if chord.trim().is_empty() {
            bindings.unbind(action);
            continue;
        }
        bindings.bind(action, chord).map_err(|refusal| {
            format!(
                "the config file binds `{name}` to `{chord}`, which Muster cannot read: \
                 {refusal} None of the file was applied."
            )
        })?;
    }
    Ok(bindings)
}

/// The `[[daemon]]` blocks, or an empty list when the file names none.
///
/// A file with no daemons in it is allowed. It means "everything else in here, and find the
/// daemon yourself", which is what the file will look like the day it holds only a keymap.
fn daemon_blocks(root: &toml::Table) -> Result<Vec<toml::Table>, String> {
    let Some(value) = root.get("daemon") else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| {
        "the config file's `daemon` is not a list of blocks, so no daemon in it was applied. \
         Each one is its own `[[daemon]]` block, with the double brackets - a single \
         `[daemon]` declares one table and is the usual way to arrive here."
            .to_string()
    })?;
    array
        .iter()
        .map(|entry| {
            entry.as_table().cloned().ok_or_else(|| {
                "one of the `[[daemon]]` entries is not a block, so no daemon in the file was \
                 applied. Every entry is a `[[daemon]]` header followed by its keys."
                    .to_string()
            })
        })
        .collect()
}

fn read_daemon(block: &toml::Table, index: usize) -> Result<Daemon, String> {
    let where_ = format!("the {} `[[daemon]]` block", ordinal(index));
    known_keys(block.keys(), &DAEMON_KEYS, &where_)?;

    let id = string(block, "id", &where_)?.ok_or_else(|| {
        format!(
            "{where_} has no `id`, so there is no name to refer to it by and none of the \
             file was applied. An id is Muster's own name for a daemon rather than anything \
             the daemon knows about itself - `local` and `devenv` read well in a log line, \
             which is where they are met."
        )
    })?;
    if id.is_empty() {
        return Err(format!(
            "{where_} has an empty `id`, so nothing could refer to it and none of the file \
             was applied. Give it a name that reads well in a log line."
        ));
    }

    let socket = string(block, "socket", &where_)?;
    let host = string(block, "host", &where_)?;
    let ssh_options = strings(block, "ssh_options", &where_)?;

    let endpoint = match host {
        Some(host) if host.is_empty() => {
            return Err(format!(
                "{where_} has an empty `host`, so there is nothing for ssh to connect to and \
                 none of the file was applied. Either name a destination ssh accepts, or drop \
                 the key to mean a daemon on this machine."
            ));
        }
        Some(host) => {
            Endpoint::Ssh { host, options: ssh_options.unwrap_or_default(), socket_path: socket }
        }
        None => {
            if ssh_options.is_some() {
                return Err(format!(
                    "{where_} sets `ssh_options` and no `host`, so it describes a daemon on \
                     this machine reached over ssh, which is not a thing. None of the file was \
                     applied. Add the `host` this was meant for, or drop the options."
                ));
            }
            Endpoint::Local { socket_path: socket }
        }
    };
    Ok(Daemon { id: DaemonId::new(id), endpoint })
}

/// Refuses a key nobody will read.
///
/// Named against the block it was found in, because a config file with two daemons in it
/// has two places a `hosts` could be sitting and only one of them is wrong.
fn known_keys<'a>(
    found: impl Iterator<Item = &'a String>,
    known: &[&str],
    where_: &str,
) -> Result<(), String> {
    for key in found {
        if !known.contains(&key.as_str()) {
            return Err(format!(
                "{where_} has a key called {key:?}, which Muster does not read. None of the \
                 file was applied, because a key nobody reads is usually a misspelling of one \
                 somebody meant. Known keys here: {}.",
                known.join(", ")
            ));
        }
    }
    Ok(())
}

fn string(block: &toml::Table, key: &str, where_: &str) -> Result<Option<String>, String> {
    match block.get(key) {
        None => Ok(None),
        Some(Value::String(found)) => Ok(Some(found.clone())),
        Some(found) => Err(format!(
            "{where_} sets `{key}` to {}, and it has to be a string. None of the file was \
             applied.",
            described(found)
        )),
    }
}

fn strings(block: &toml::Table, key: &str, where_: &str) -> Result<Option<Vec<String>>, String> {
    let Some(value) = block.get(key) else { return Ok(None) };
    let array = value.as_array().ok_or_else(|| {
        format!(
            "{where_} sets `{key}` to {}, and it has to be a list of strings. None of the \
             file was applied.",
            described(value)
        )
    })?;
    array
        .iter()
        .map(|entry| match entry {
            Value::String(found) => Ok(found.clone()),
            found => Err(format!(
                "{where_} has {} in its `{key}`, and every entry has to be a string. None of \
                 the file was applied.",
                described(found)
            )),
        })
        .collect::<Result<Vec<String>, String>>()
        .map(Some)
}

/// What a value is, for a message that has to say why it was refused.
fn described(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "a string",
        // One answer for both, because the difference between 2222 and 2222.0 is not what
        // anybody who typed a port without quotes needs to hear about.
        Value::Integer(_) | Value::Float(_) => "a number",
        Value::Boolean(_) => "a true or false",
        Value::Datetime(_) => "a date",
        Value::Array(_) => "a list",
        Value::Table(_) => "a block",
    }
}

/// How a daemon is reached, in a sentence.
fn describe(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::Local { socket_path: None } => "a daemon on this machine".to_string(),
        Endpoint::Local { socket_path: Some(path) } => {
            format!("a daemon on this machine at {path}")
        }
        Endpoint::Ssh { host, .. } => format!("a daemon on {host}"),
    }
}

fn ordinal(index: usize) -> String {
    match index {
        0 => "first".to_string(),
        1 => "second".to_string(),
        2 => "third".to_string(),
        _ => format!("{}th", index + 1),
    }
}
