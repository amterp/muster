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
    /// The numbers that decide how the window feels to drive.
    pub feel: Feel,
    /// What the window looks like, pane and chrome alike.
    pub appearance: Appearance,
    /// What a pane is, for the daemon that makes one.
    pub panes: Panes,
}

/// What Muster looks like.
///
/// Every value here is optional, and absent means the renderer's own default rather than one
/// Muster invented. That is the honest answer twice over: nobody has asked Muster for an
/// opinion about a monospace font, and a sixteen-entry palette written down here would be a
/// transcription of somebody else's rather than a decision. What the vocabulary is for is
/// naming what a person may change - a replacement renderer supplies the rest, and that is a
/// stated limit of the contract rather than a gap in it.
///
/// Split across `[font]`, `[colors]` and `[cursor]` because those are three subjects a person
/// edits at different times, and `pane_padding` sits at the root because it is one answer with
/// no siblings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Appearance {
    pub font: Font,
    pub colors: Colors,
    pub cursor: Cursor,

    /// Blank space between a pane's text and its edges, in points.
    ///
    /// Zero is a real answer rather than an absent one - a window of fifteen agent panes fits
    /// more rows without it, and somebody who wants that has to be able to say so.
    pub pane_padding: Option<u16>,
}

/// `[font]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Font {
    /// A family name as the system knows it - `Fira Code`, `Menlo`.
    ///
    /// Absent means whatever monospace the renderer would have picked, which is a question
    /// about the machine rather than about Muster: a family named here that nobody installed
    /// is worse than no answer at all.
    pub family: Option<String>,

    /// In points.
    pub size: Option<f32>,
}

/// `[colors]`.
///
/// Six that decide what a pane looks like, one that decides what Muster's own chrome looks
/// like, and the ANSI palette a program in a pane addresses by number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Colors {
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    pub cursor: Option<Rgb>,

    /// What a character under the cursor is painted in.
    pub cursor_text: Option<Rgb>,

    pub selection_background: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,

    /// The line between two regions.
    ///
    /// The one colour here that no renderer paints: everything else is inside a pane, and this
    /// is Muster's own chrome. It sits with the others anyway, because a person picking colours
    /// is picking all of them at once and its being drawn by a different piece of code is not
    /// something they should have to know.
    pub divider: Option<Rgb>,

    /// The sixteen ANSI colours, black through bright white, or none of them.
    ///
    /// All sixteen or nothing. A palette is a set rather than a list of independent choices -
    /// four entries of somebody's theme over twelve of the renderer's is a scheme nobody
    /// designed, and it would be indistinguishable from a file somebody stopped editing
    /// halfway.
    pub palette: Option<[Rgb; 16]>,
}

/// `[cursor]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub style: Option<CursorStyle>,

    /// Absent leaves it to the program in the pane, which can ask for either and often does.
    /// Naming it here overrules that, which is what somebody who finds a blinking cursor
    /// distracting is asking for.
    pub blink: Option<bool>,
}

/// The shapes a cursor comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
    /// An outline rather than a filled block.
    Hollow,
}

impl CursorStyle {
    pub const READABLE: [&'static str; 4] = ["block", "bar", "underline", "hollow"];

    pub fn read(name: &str) -> Option<CursorStyle> {
        match name {
            "block" => Some(CursorStyle::Block),
            "bar" => Some(CursorStyle::Bar),
            "underline" => Some(CursorStyle::Underline),
            "hollow" => Some(CursorStyle::Hollow),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CursorStyle::Block => "block",
            CursorStyle::Bar => "bar",
            CursorStyle::Underline => "underline",
            CursorStyle::Hollow => "hollow",
        }
    }
}

/// The small answers a terminal is expected to let somebody change about how it handles.
///
/// Grouped because they share a shape rather than a subject: each is one value, read once,
/// with a defensible default, and neither is a decision Muster wants to make on somebody's
/// behalf. What they have in common is that getting them wrong is an irritation nobody can
/// name - a resize that moves too far, a trackpad that scrolls too slowly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feel {
    /// How far a resize chord moves a divider.
    ///
    /// `None` leaves it to the daemon, which is what a keybinding meant before this existed
    /// and remains the default: herdr sizes its own rectangles and has an answer already.
    /// Naming a distance is for somebody who finds that answer too coarse or too fine.
    pub resize_step: Option<ResizeStep>,

    /// What one notch of the wheel is worth, in lines.
    ///
    /// A multiplier rather than a line count, because the thing being scaled is a delta whose
    /// size is the input device's business: a trackpad reports many small ones and a wheel
    /// mouse a few large ones, and only the person using them knows which needs adjusting.
    pub scroll_multiplier: f64,
}

impl Default for Feel {
    fn default() -> Feel {
        Feel { resize_step: None, scroll_multiplier: 1.0 }
    }
}

/// How far a resize chord moves a divider, and in what.
///
/// Two units because neither one is right for everybody. A cell is about 8 by 17 points, so a
/// single number in cells moves a divider roughly twice as far up and down as it does side to
/// side, and four symmetric chords that travel visibly different distances is not what a hand
/// expects - points are what a nudge is felt in. Cells keep their own advantage: they survive
/// a font size change, where a distance in points does not, and `cmd+=` is a thing people
/// press.
///
/// `px` is screen points, the same unit as `pane_padding` and `[font] size`, rather than
/// backing pixels. Two length keys in one file that mean different things is a trap, and on
/// this platform a number somebody calls a pixel is a point almost everywhere they meet one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeStep {
    Cells(u16),
    Points(u16),
}

impl ResizeStep {
    /// Reads `20c` or `150px`, and says what it could not read.
    ///
    /// **The suffix is required**, and that is the whole of what makes two units safe: a bare
    /// `20` meaning cells beside a suffixed `"150px"` is a form you have to know rather than
    /// read, and there is no reading of a bare number that is obviously right. Spelled `c`
    /// rather than `cells` because kitty already spells this exact ambiguity that way, so
    /// somebody arriving from a terminal that solved it has one less thing to learn.
    pub fn parse(text: &str) -> Result<ResizeStep, String> {
        let trimmed = text.trim();
        let unreadable = || {
            format!(
                "`resize_step` in the config file is `{text}`, which is not a distance Muster \
                 can read. Write a whole number and a unit: `\"20c\"` moves twenty cells, and \
                 `\"150px\"` moves a hundred and fifty points."
            )
        };

        let (digits, step): (&str, fn(u16) -> ResizeStep) =
            if let Some(digits) = trimmed.strip_suffix("px") {
                (digits, ResizeStep::Points)
            } else if let Some(digits) = trimmed.strip_suffix('c') {
                (digits, ResizeStep::Cells)
            } else {
                return Err(unreadable());
            };

        let distance: u16 = digits.trim().parse().map_err(|_| unreadable())?;
        if distance == 0 {
            return Err(format!(
                "`resize_step` in the config file is `{text}`, so a resize chord would move \
                 nothing. Name a distance of at least one, or leave the key out to keep the \
                 daemon's own step."
            ));
        }
        Ok(step(distance))
    }

    /// This step in cells, along an axis whose cell measures `cell_points` points.
    ///
    /// The conversion lives here rather than in the shell so that every caller gets the same
    /// arithmetic - a chord today, and the CLI when it arrives. Cells are the identity case
    /// of it, which is why supporting both units costs close to nothing once the shell
    /// measures a cell at all.
    ///
    /// Rounded to a whole cell and floored at one, because the daemon resizes a grid: a step
    /// too small to move a column is a chord that looks broken, and the person who asked for
    /// `"4px"` on a wide font meant the smallest nudge available rather than none.
    ///
    /// `None` when a step in points meets a caller that could not measure a cell. The caller
    /// is expected to fall back to the daemon's own step and say so, because a distance
    /// guessed here would be wrong by whatever the font happens to be.
    pub fn cells(self, cell_points: Option<f32>) -> Option<f32> {
        match self {
            ResizeStep::Cells(cells) => Some(f32::from(cells)),
            ResizeStep::Points(points) => {
                let cell = cell_points.filter(|cell| cell.is_finite() && *cell > 0.0)?;
                Some((f32::from(points) / cell).round().max(1.0))
            }
        }
    }
}

impl std::fmt::Display for ResizeStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResizeStep::Cells(cells) => write!(f, "{cells}c"),
            ResizeStep::Points(points) => write!(f, "{points}px"),
        }
    }
}

/// What a pane is: what it runs, and how much of it a person can scroll back through.
///
/// Muster acts on neither. It translates both onward for the daemon, the way [`Appearance`]
/// is translated onward for the renderer - and it is here for the same reason that one is.
/// A person who wants deeper scrollback was previously told to learn that herdr exists and
/// find its config file, and a `default_shell` somebody set for their own terminal decided
/// what every Muster pane ran. Both are questions about Muster's window, so both are asked
/// in Muster's file.
///
/// Absent means the daemon's own default, on the same terms as an absent colour meaning the
/// renderer's: Muster has no opinion about which shell somebody uses, and a scrollback depth
/// written down here would be a transcription of somebody else's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Panes {
    /// How much of a pane's history the daemon keeps, in bytes.
    ///
    /// Bytes rather than lines because that is what the buffer is measured in - a line has
    /// no fixed size, so a count of them would be a number that did not mean what it said.
    /// Zero is a real answer, and herdr defines it: a pane that keeps only what is on
    /// screen.
    pub scrollback_bytes: Option<u64>,

    /// `[shell]`.
    pub shell: Shell,
}

/// What a pane runs, and how it starts.
///
/// A block rather than two root keys because they are one subject with two answers, and
/// nobody sets the second without having thought about the first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shell {
    /// The program a new pane runs.
    ///
    /// Absent means the shell the machine already thinks is yours, which is what every
    /// terminal does and what a pane did before this key existed.
    pub command: Option<String>,

    pub mode: ShellMode,
}

/// Whether a pane's shell starts as a login shell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShellMode {
    /// Let the daemon decide, which is what it did before this key existed.
    #[default]
    Auto,
    /// Reads the login files - `.zprofile`, `.profile` - as a terminal's first shell does.
    Login,
    /// Skips them, which is faster and is what a subshell would have been.
    NonLogin,
}

impl ShellMode {
    pub const READABLE: [&'static str; 3] = ["auto", "login", "non_login"];

    pub fn read(name: &str) -> Option<ShellMode> {
        match name {
            "auto" => Some(ShellMode::Auto),
            "login" => Some(ShellMode::Login),
            "non_login" => Some(ShellMode::NonLogin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ShellMode::Auto => "auto",
            ShellMode::Login => "login",
            ShellMode::NonLogin => "non_login",
        }
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
const ROOT_KEYS: [&str; 12] = [
    "daemon",
    "keymap",
    "text",
    "option_as_alt",
    "resize_step",
    "scroll_multiplier",
    "pane_padding",
    "scrollback_bytes",
    "font",
    "colors",
    "cursor",
    "shell",
];

/// The keys `[font]` may carry.
const FONT_KEYS: [&str; 2] = ["family", "size"];

/// The keys `[colors]` may carry.
const COLOR_KEYS: [&str; 8] = [
    "background",
    "foreground",
    "cursor",
    "cursor_text",
    "selection_background",
    "selection_foreground",
    "divider",
    "palette",
];

/// The keys `[cursor]` may carry.
const CURSOR_KEYS: [&str; 2] = ["style", "blink"];

/// The keys `[shell]` may carry.
const SHELL_KEYS: [&str; 2] = ["command", "mode"];

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
        appearance: read_appearance(root)?,
        panes: read_panes(root)?,
    })
}

/// `scrollback_bytes` and `[shell]`.
fn read_panes(root: &toml::Table) -> Result<Panes, String> {
    let mut panes = Panes {
        scrollback_bytes: None,
        shell: read_shell(block(root, "shell", &SHELL_KEYS)?.as_ref())?,
    };

    if let Some(value) = root.get("scrollback_bytes") {
        panes.scrollback_bytes = Some(
            value.as_integer().and_then(|bytes| u64::try_from(bytes).ok()).ok_or_else(|| {
                format!(
                    "`scrollback_bytes` in the config file is {}, and it has to be a whole \
                     number of bytes, zero or more. None of the file was applied. Bytes rather \
                     than lines because that is what the daemon measures the buffer in; leave \
                     it out for its own answer.",
                    written(value)
                )
            })?,
        );
    }

    Ok(panes)
}

fn read_shell(block: Option<&toml::Table>) -> Result<Shell, String> {
    let Some(block) = block else { return Ok(Shell::default()) };
    let mut shell = Shell::default();

    if let Some(command) = string(block, "command", "the config file's [shell]")? {
        // Refused here rather than left to the daemon, because herdr reads an empty
        // `default_shell` as "whatever SHELL says" - so an empty string would silently mean
        // the same as leaving the key out, and somebody who wrote one meant something by it.
        if command.is_empty() {
            return Err("`command` in the config file's [shell] is empty, and a pane has to run \
                        something. None of the file was applied. Leave the key out for the shell \
                        this machine already thinks is yours."
                .to_string());
        }
        // A control character in a program name reaches the daemon as an environment or an
        // argv byte and is refused there, a process and a machine away from whoever typed it.
        if command.chars().any(char::is_control) {
            return Err(format!(
                "`command` in the config file's [shell] is {command:?}, which holds a control \
                 character. None of the file was applied. A program name is a path - if that is \
                 a pasted escape sequence, retype it."
            ));
        }
        shell.command = Some(command);
    }

    if let Some(value) = block.get("mode") {
        let name = value.as_str().ok_or_else(|| {
            format!(
                "`mode` in the config file's [shell] is {}, and it has to be one of {}. None of \
                 the file was applied.",
                described(value),
                quoted(&ShellMode::READABLE),
            )
        })?;
        shell.mode = ShellMode::read(name).ok_or_else(|| {
            format!(
                "`mode` in the config file's [shell] is {name:?}, which is not a way of starting \
                 a shell that Muster knows, so none of the file was applied. It is one of {}.",
                quoted(&ShellMode::READABLE),
            )
        })?;
    }

    Ok(shell)
}

/// The two knobs, each absent from the file more often than not.
fn read_feel(root: &toml::Table) -> Result<Feel, String> {
    let mut feel = Feel::default();

    if let Some(value) = root.get("resize_step") {
        let text = value.as_str().ok_or_else(|| unsuffixed(value))?;
        let step = ResizeStep::parse(text)
            .map_err(|why| format!("{why} None of the file was applied."))?;
        feel.resize_step = Some(step);
    }

    if let Some(value) = root.get("scroll_multiplier") {
        let multiplier = number(value, "scroll_multiplier", "the config file")?;
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

    Ok(feel)
}

/// `[font]`, `[colors]`, `[cursor]` and `pane_padding`.
fn read_appearance(root: &toml::Table) -> Result<Appearance, String> {
    let mut appearance = Appearance {
        font: read_font(block(root, "font", &FONT_KEYS)?.as_ref())?,
        colors: read_colors(block(root, "colors", &COLOR_KEYS)?.as_ref())?,
        cursor: read_cursor(block(root, "cursor", &CURSOR_KEYS)?.as_ref())?,
        pane_padding: None,
    };

    if let Some(value) = root.get("pane_padding") {
        appearance.pane_padding = Some(
            value.as_integer().and_then(|points| u16::try_from(points).ok()).ok_or_else(|| {
                format!(
                    "`pane_padding` in the config file is {}, and it has to be a whole \
                         number of points, zero or more. None of the file was applied. Zero \
                         is the one that fits the most rows in a window; leave it out for \
                         the renderer's own answer.",
                    written(value)
                )
            })?,
        );
    }

    Ok(appearance)
}

/// One of the appearance blocks, checked for keys nobody reads.
///
/// Cloned rather than borrowed because the alternative is threading a lifetime through three
/// readers to save copying at most eight small values, once, at startup.
fn block(root: &toml::Table, name: &str, known: &[&str]) -> Result<Option<toml::Table>, String> {
    let Some(value) = root.get(name) else { return Ok(None) };
    let table = value.as_table().ok_or_else(|| {
        format!(
            "`{name}` in the config file is {}, and it has to be a block - `[{name}]` with \
             its settings under it. None of the file was applied. Known keys there: {}.",
            described(value),
            known.join(", ")
        )
    })?;
    known_keys(table.keys(), known, &format!("the config file's [{name}]"))?;
    Ok(Some(table.clone()))
}

fn read_font(block: Option<&toml::Table>) -> Result<Font, String> {
    let Some(block) = block else { return Ok(Font::default()) };

    let mut size = None;
    if let Some(value) = block.get("size") {
        let points = number(value, "size", "the config file's [font]")?;
        // libghostty's own range. Refused here rather than clamped there, because a font size
        // of zero silently becoming 1 is a window nobody can read and no line explaining it.
        if !(points.is_finite() && (1.0..=255.0).contains(&points)) {
            return Err(format!(
                "`size` in the config file's [font] is {}, and it has to be a number of points \
                 between 1 and 255. None of the file was applied.",
                written(value)
            ));
        }
        // Narrowed only after the range check, so the cast is over a value between 1 and 255.
        // What it loses is precision past the seventh digit of a font size, which no renderer
        // takes and nobody typed.
        #[allow(clippy::cast_possible_truncation)]
        {
            size = Some(points as f32);
        }
    }

    Ok(Font {
        // An empty family is treated as absent rather than refused, because it is the one way
        // to write "whatever this machine has" in a file that otherwise names one.
        family: string(block, "family", "the config file's [font]")?
            .filter(|family| !family.trim().is_empty()),
        size,
    })
}

fn read_colors(block: Option<&toml::Table>) -> Result<Colors, String> {
    let Some(block) = block else { return Ok(Colors::default()) };
    Ok(Colors {
        background: color(block, "background")?,
        foreground: color(block, "foreground")?,
        cursor: color(block, "cursor")?,
        cursor_text: color(block, "cursor_text")?,
        selection_background: color(block, "selection_background")?,
        selection_foreground: color(block, "selection_foreground")?,
        divider: color(block, "divider")?,
        palette: read_palette(block)?,
    })
}

/// One `#rrggbb` under `[colors]`, refused where it was written.
fn color(block: &toml::Table, key: &str) -> Result<Option<Rgb>, String> {
    let Some(value) = block.get(key) else { return Ok(None) };
    let text = value.as_str().ok_or_else(|| {
        format!(
            "`{key}` in the config file's [colors] is {}, and it has to be a string of six hex \
             digits - `{key} = \"#4a4a4a\"`. None of the file was applied.",
            described(value)
        )
    })?;
    Rgb::parse(text).map(Some).map_err(|refusal| {
        format!("`{key}` in the config file's [colors]: {refusal} None of the file was applied.")
    })
}

/// The sixteen ANSI colours, all of them or none.
fn read_palette(block: &toml::Table) -> Result<Option<[Rgb; 16]>, String> {
    let Some(entries) = strings(block, "palette", "the config file's [colors]")? else {
        return Ok(None);
    };
    if entries.len() != 16 {
        return Err(format!(
            "`palette` in the config file's [colors] has {} {}, and it has to have exactly 16 - \
             one for each ANSI colour, black through bright white. None of the file was \
             applied. A palette is a set rather than a list of separate choices, so a partial \
             one would leave the rest as the renderer's and produce a scheme nobody designed.",
            entries.len(),
            if entries.len() == 1 { "entry" } else { "entries" }
        ));
    }

    let mut palette = [Rgb { red: 0, green: 0, blue: 0 }; 16];
    for (at, entry) in entries.iter().enumerate() {
        palette[at] = Rgb::parse(entry).map_err(|refusal| {
            format!(
                "the {} entry of `palette` in the config file's [colors]: {refusal} None of the \
                 file was applied.",
                ordinal(at)
            )
        })?;
    }
    Ok(Some(palette))
}

fn read_cursor(block: Option<&toml::Table>) -> Result<Cursor, String> {
    let Some(block) = block else { return Ok(Cursor::default()) };
    let mut cursor = Cursor::default();

    if let Some(value) = block.get("style") {
        let name = value.as_str().ok_or_else(|| {
            format!(
                "`style` in the config file's [cursor] is {}, and it has to be one of {}. None \
                 of the file was applied.",
                described(value),
                quoted(&CursorStyle::READABLE),
            )
        })?;
        cursor.style = Some(CursorStyle::read(name).ok_or_else(|| {
            format!(
                "`style` in the config file's [cursor] is {name:?}, which is not a shape Muster \
                 knows, so none of the file was applied. It is one of {}.",
                quoted(&CursorStyle::READABLE),
            )
        })?);
    }

    if let Some(value) = block.get("blink") {
        cursor.blink = Some(value.as_bool().ok_or_else(|| {
            format!(
                "`blink` in the config file's [cursor] is {}, and it has to be true or false. \
                 None of the file was applied. Leave it out to let the program in the pane \
                 decide, which is what it did before this key existed.",
                described(value)
            )
        })?);
    }

    Ok(cursor)
}

/// A number the file may have written as an integer or a float, since TOML tells them apart
/// and nobody writing `scroll_multiplier = 2` means anything different from `2.0`.
///
/// Whole numbers are converted through `i32`, which is exact where `i64 as f64` is not. What
/// that refuses is a knob past two billion, and every knob here is a handful of cells or a
/// small multiplier - so the refusal lands on a value nobody meant, with the same sentence as
/// any other unusable one.
fn number(value: &Value, key: &str, where_: &str) -> Result<f64, String> {
    value.as_float().or_else(|| i32::try_from(value.as_integer()?).ok().map(f64::from)).ok_or_else(
        || {
            format!(
                "`{key}` in {where_} is {}, and it has to be a number. None of the file was \
                 applied.",
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
/// A refusal about `pane_padding = 1.5` should say `1.5` - the type is not what is wrong with
/// it, and "is a number, and it has to be a whole number" reads like a bug in Muster. Anything
/// that is not a number falls back to naming the type, which is all there is to say about a
/// string where a count belongs.
fn written(value: &Value) -> String {
    match value {
        Value::Integer(whole) => whole.to_string(),
        Value::Float(number) => number.to_string(),
        other => described(other).to_string(),
    }
}

/// `resize_step` written without a unit, which is the form that used to work.
///
/// Its own sentence rather than the generic refusal, because somebody meeting this is reading
/// a file that parsed yesterday: what they need is the new spelling of the number they
/// already chose, not an account of what is wrong with it. A value that is not a number gets
/// the plain form, since there is no number to hand back.
fn unsuffixed(value: &Value) -> String {
    // Only a whole number gets its own value handed back, because only a whole number is a
    // legal step: offering `"1.5c"` to somebody who wrote `1.5` would be advice that fails.
    let advice = match value {
        Value::Integer(whole) if *whole > 0 => format!(
            "Write `\"{whole}c\"` to keep moving that many cells, or `\"{whole}px\"` to move \
             that many points instead."
        ),
        _ => "Write a whole number and a unit - `\"20c\"` for cells, `\"150px\"` for points."
            .to_string(),
    };
    format!(
        "`resize_step` in the config file is {}, and it needs a unit now. None of the file was \
         applied. {advice} Two units, each spelled out, so there is nothing to guess.",
        written(value)
    )
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
