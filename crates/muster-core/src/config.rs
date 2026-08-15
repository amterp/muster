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

use std::collections::BTreeMap;

use toml::Value;

use crate::input::{Action, Binding, Bindings, Chord, OptionAsAlt, PaneInputSettings};

use crate::composition::{Daemon, DaemonId, Endpoint};

/// Everything a config file says.
///
/// `Eq` is absent because of the feel's floats, on the same terms as `Composition`: a
/// multiplier is a float, and a float has no total equality.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// In the order the file lists them, which is the order their regions are laid out.
    pub daemons: Vec<Daemon>,
    /// Which chord asks for which of Muster's own actions, with the file's answers over the
    /// defaults. A file that names none leaves every default in place.
    pub bindings: Bindings,
    /// What the file says about keystrokes on their way to a pane, which is a different
    /// question from which chord Muster keeps for itself.
    pub input: PaneInputSettings,
    /// The numbers and the one colour that decide how the window feels to drive.
    pub feel: Feel,
}

/// The small answers a terminal is expected to let somebody change.
///
/// Grouped because they share a shape rather than a subject: each is one value, read once,
/// with a defensible default, and none of them is a decision Muster wants to make on
/// somebody's behalf. What they have in common is that getting them wrong is an irritation
/// nobody can name - a resize that moves too far, a trackpad that scrolls too slowly, a
/// divider you cannot see against your theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feel {
    /// How many cells a resize chord moves a divider.
    ///
    /// `None` leaves it to the daemon, which is what a keybinding meant before this existed
    /// and remains the default: herdr sizes its own rectangles and has an answer already.
    /// Naming a number is for somebody who finds that answer too coarse or too fine.
    ///
    /// Whole cells, unlike the `amount` it becomes at the seam. That field is a float so a
    /// CLI can place a divider exactly; a chord cannot mean half a cell, and accepting `1.5`
    /// here would be a number the terminal has nowhere to put.
    pub resize_step: Option<u16>,

    /// What one notch of the wheel is worth, in lines.
    ///
    /// A multiplier rather than a line count, because the thing being scaled is a delta whose
    /// size is the input device's business: a trackpad reports many small ones and a wheel
    /// mouse a few large ones, and only the person using them knows which needs adjusting.
    pub scroll_multiplier: f64,

    /// The line between two regions, as `#rrggbb`.
    ///
    /// `None` takes the platform's own separator colour, which is right on a system theme and
    /// wrong beside a pane painted from somebody's Ghostty config. This is Muster's own chrome
    /// rather than libghostty's, so no terminal config can reach it and none ever will - which
    /// is why it is here rather than waiting on Muster's appearance vocabulary.
    pub divider_color: Option<Rgb>,
}

impl Default for Feel {
    fn default() -> Feel {
        Feel { resize_step: None, scroll_multiplier: 1.0, divider_color: None }
    }
}

/// A colour, as the config file spells one and as a shell can paint it.
///
/// Three bytes rather than a string, so that reading the file is where a bad colour is
/// refused - a shell handed `"#gg0000"` would have to either fail or invent something, and
/// both happen too far from the person who typed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    /// Reads `#rrggbb`, or `rrggbb`, and says what it could not read.
    ///
    /// One spelling and its bare variant, rather than the whole CSS colour vocabulary. Names
    /// and `rgb()` would be a second colour language to document and to keep, and hex is what
    /// every terminal config in this space already uses.
    pub fn parse(text: &str) -> Result<Rgb, String> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "`{text}` is not a colour Muster can read. Write six hex digits, with or \
                 without a leading hash - `#4a4a4a`."
            ));
        }
        let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or_default();
        Ok(Rgb { red: byte(0), green: byte(2), blue: byte(4) })
    }
}

impl std::fmt::Display for Rgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }
}

/// The keys a `[[daemon]]` block may carry.
const DAEMON_KEYS: [&str; 4] = ["id", "socket", "host", "ssh_options"];

/// The keys the file itself may carry.
const ROOT_KEYS: [&str; 7] = [
    "daemon",
    "keymap",
    "text",
    "option_as_alt",
    "resize_step",
    "scroll_multiplier",
    "divider_color",
];

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
    Ok(Config {
        daemons,
        bindings: read_keymap(root)?,
        input: PaneInputSettings {
            option_as_alt: read_option_as_alt(root)?,
            text: read_text(root)?,
        },
        feel: read_feel(root)?,
    })
}

/// The three knobs, each absent from the file more often than not.
fn read_feel(root: &toml::Table) -> Result<Feel, String> {
    let mut feel = Feel::default();

    if let Some(value) = root.get("resize_step") {
        let step = value
            .as_integer()
            .and_then(|step| u16::try_from(step).ok())
            .filter(|step| *step > 0)
            .ok_or_else(|| {
                format!(
                    "`resize_step` in the config file is {}, and it has to be a whole number \
                     of cells, at least one. None of the file was applied. Leave it out to \
                     keep the daemon's own step, which is what a chord meant before this key \
                     existed.",
                    written(value)
                )
            })?;
        feel.resize_step = Some(step);
    }

    if let Some(value) = root.get("scroll_multiplier") {
        let multiplier = number(value, "scroll_multiplier")?;
        if !(multiplier.is_finite() && multiplier > 0.0) {
            return Err(format!(
                "`scroll_multiplier` in the config file is {multiplier}, so the wheel would \
                 scroll nowhere or backwards. None of the file was applied. It scales what \
                 the input device reports, so 1 is the device's own answer, 2 is twice as \
                 far, and 0.5 is half."
            ));
        }
        feel.scroll_multiplier = multiplier;
    }

    if let Some(value) = root.get("divider_color") {
        let text = value.as_str().ok_or_else(|| {
            format!(
                "`divider_color` in the config file is {}, and it has to be a string of six \
                 hex digits - `divider_color = \"#4a4a4a\"`. None of the file was applied.",
                described(value)
            )
        })?;
        feel.divider_color = Some(
            Rgb::parse(text)
                .map_err(|refusal| format!("{refusal} None of the file was applied."))?,
        );
    }

    Ok(feel)
}

/// A number the file may have written as an integer or a float, since TOML tells them apart
/// and nobody writing `resize_step = 2` means anything different from `2.0`.
///
/// Whole numbers are converted through `i32`, which is exact where `i64 as f64` is not. What
/// that refuses is a knob past two billion, and every knob here is a handful of cells or a
/// small multiplier - so the refusal lands on a value nobody meant, with the same sentence as
/// any other unusable one.
fn number(value: &Value, key: &str) -> Result<f64, String> {
    value.as_float().or_else(|| i32::try_from(value.as_integer()?).ok().map(f64::from)).ok_or_else(
        || {
            format!(
                "`{key}` in the config file is {}, and it has to be a number. None of the \
                 file was applied.",
                described(value)
            )
        },
    )
}

/// Whether the option key means alt, as the file says.
///
/// Muster's default is `never`, which is macOS's own behavior: option composes `†` out of
/// `opt+t` and the pane receives that character. Right for somebody typing accents and wrong
/// for somebody whose agent binds alt chords, and only they know which they are.
fn read_option_as_alt(root: &toml::Table) -> Result<OptionAsAlt, String> {
    let Some(value) = root.get("option_as_alt") else {
        return Ok(OptionAsAlt::default());
    };
    let name = value.as_str().ok_or_else(|| {
        format!(
            "`option_as_alt` in the config file is {}, and it has to be one of {}. None of \
             the file was applied.",
            described(value),
            quoted(&OptionAsAlt::READABLE),
        )
    })?;
    OptionAsAlt::read(name).ok_or_else(|| {
        format!(
            "`option_as_alt` in the config file is {name:?}, which Muster does not know, so \
             none of the file was applied. It is one of {}: `never` leaves option composing \
             characters the way macOS does, and the others make that side of the keyboard \
             send alt instead.",
            quoted(&OptionAsAlt::READABLE),
        )
    })
}

/// The `[text]` block: chords that stand for literal bytes.
///
/// Keyed by chord rather than by action, which is the other way round from `[keymap]`, and
/// deliberately. An action has one chord, so naming the action reads best there; text has no
/// name at all, so the chord is the only thing left to key on.
fn read_text(root: &toml::Table) -> Result<BTreeMap<Binding, Vec<u8>>, String> {
    let mut text = BTreeMap::new();
    let Some(value) = root.get("text") else {
        return Ok(text);
    };
    let block = value.as_table().ok_or_else(|| {
        format!(
            "`text` in the config file is {}, and it has to be a table of chords - `[text]` \
             with a line like `\"shift+enter\" = \"\\n\"` under it. None of the file was \
             applied.",
            described(value)
        )
    })?;

    for (chord, bytes) in block {
        let binding = Chord::parse(chord)
            .map(|Chord { key, modifiers }| Binding::new(key, modifiers))
            .map_err(|refusal| {
                format!(
                    "the config file's [text] has a chord `{chord}` which Muster cannot \
                     read: {refusal} None of the file was applied."
                )
            })?;
        let bytes = bytes.as_str().ok_or_else(|| {
            format!(
                "the config file's [text] gives `{chord}` {}, and it has to be the string to \
                 send - `\"\\n\"` for a newline. None of the file was applied.",
                described(bytes)
            )
        })?;
        // An empty string is refused rather than taken as an unbinding. A chord bound to no
        // bytes and a chord left to the encoder look identical from the pane and mean
        // opposite things here, and nothing in the file yet needs the distinction.
        if bytes.is_empty() {
            return Err(format!(
                "the config file's [text] gives `{chord}` an empty string, so pressing it \
                 would send nothing at all. None of the file was applied. Delete the line to \
                 leave the chord alone."
            ));
        }
        if text.insert(binding, bytes.as_bytes().to_vec()).is_some() {
            return Err(format!(
                "the config file's [text] binds `{chord}` twice under different spellings, \
                 and one of them silently wins. None of the file was applied."
            ));
        }
    }
    Ok(text)
}

/// A list of names, quoted, for a refusal that has to say what was allowed.
fn quoted(names: &[&str]) -> String {
    names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ")
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
/// A value as the file wrote it, when that is more use than its type.
///
/// A refusal about `resize_step = 0` should say `0` - the type is not what is wrong with it,
/// and "is a number, and it has to be a whole number" reads like a bug in Muster. Anything
/// that is not a number falls back to naming the type, which is all there is to say about a
/// string where a count belongs.
fn written(value: &Value) -> String {
    match value {
        Value::Integer(whole) => whole.to_string(),
        Value::Float(number) => number.to_string(),
        other => described(other).to_string(),
    }
}

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
