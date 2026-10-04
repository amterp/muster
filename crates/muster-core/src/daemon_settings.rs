//! What Muster tells each daemon about itself, from the config file.
//!
//! A daemon reads no config of Muster's (MIP-3, section 9), so everything it needs arrives over
//! the protocol: at connect, and again whenever the config changes. The shell a pane runs and
//! how much history it keeps are the daemon's to act on; the palette and the cursor are what
//! the daemon tells programs that ask, so they must be what the window draws.
//!
//! Pure, so the one rule here that is a decision rather than a copy - what palette a config
//! that named only some colours stands for - is answerable to a test.

use crate::config::{ClipboardWrite, Config, Cursor, Rgb, Shell};

/// Ghostty's own default background and foreground (`src/config/Config.zig`), which the
/// renderer draws when the config names none.
const DEFAULT_BACKGROUND: Rgb = Rgb { red: 0x28, green: 0x2c, blue: 0x34 };
const DEFAULT_FOREGROUND: Rgb = Rgb { red: 0xff, green: 0xff, blue: 0xff };

/// Everything one daemon is told, as of one config.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonSettings {
    pub shell: Shell,
    pub scrollback_bytes: Option<u64>,
    /// `None` when the config names no colour at all, so the daemon keeps libghostty's own
    /// palette, which is what the renderer draws then too.
    pub palette: Option<Palette>,
    pub cursor: Cursor,
    /// Whether programs may set the clipboard. The window applies a write; the daemon is told
    /// so that a program asking what the terminal supports gets the truth.
    pub clipboard_write: ClipboardWrite,
    /// How far a wheel turn scrolls a program, which the daemon applies after rounding a notch
    /// up to one, as the surface applies it to its own scrolling.
    pub scroll_multiplier: f64,
    /// Whether the daemon types a pane's name into its agent's session as its name.
    pub name_sessions: bool,
}

/// The colours programs are told the terminal has, when they ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// The sixteen the config named, or none for libghostty's own.
    pub entries: Vec<Rgb>,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
    /// Whether the background is dark, which is what a program asking for the colour scheme
    /// (mode 2031) is told.
    pub dark: bool,
}

impl Default for DaemonSettings {
    /// What an empty config file says.
    fn default() -> DaemonSettings {
        DaemonSettings::from(&Config::default())
    }
}

impl DaemonSettings {
    pub fn from(config: &Config) -> DaemonSettings {
        let colors = &config.appearance.colors;
        let named = colors.background.is_some()
            || colors.foreground.is_some()
            || colors.cursor.is_some()
            || colors.palette.is_some();
        let palette = named.then(|| {
            // A colour the config left out is the renderer's default, so the daemon answers
            // with what the window actually draws.
            let background = colors.background.unwrap_or(DEFAULT_BACKGROUND);
            Palette {
                entries: colors.palette.map(|entries| entries.to_vec()).unwrap_or_default(),
                foreground: colors.foreground.unwrap_or(DEFAULT_FOREGROUND),
                background,
                cursor: colors.cursor,
                dark: is_dark(background),
            }
        });
        DaemonSettings {
            shell: config.panes.shell.clone(),
            scrollback_bytes: config.panes.scrollback_bytes,
            palette,
            cursor: config.appearance.cursor,
            clipboard_write: config.panes.clipboard_write,
            scroll_multiplier: config.feel.scroll_multiplier,
            name_sessions: config.panes.name_sessions,
        }
    }
}

/// Whether a background is dark: its relative luminance below the middle, by the sRGB weights.
fn is_dark(color: Rgb) -> bool {
    let luminance = 0.2126 * f64::from(color.red)
        + 0.7152 * f64::from(color.green)
        + 0.0722 * f64::from(color.blue);
    luminance < 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        crate::config::parse(text).expect("the case's config parses")
    }

    #[test]
    fn a_config_naming_no_colour_leaves_the_daemon_its_own_palette() {
        assert_eq!(DaemonSettings::from(&config("")).palette, None);
    }

    /// A config that names a background and nothing else still tells a program the foreground
    /// the window draws, which is the renderer's default.
    #[test]
    fn a_colour_left_out_is_the_one_the_renderer_draws() {
        let settings = DaemonSettings::from(&config("[colors]\nbackground = \"#fafafa\"\n"));
        let palette = settings.palette.expect("a background was named");
        assert_eq!(palette.foreground, DEFAULT_FOREGROUND);
        assert!(palette.entries.is_empty(), "no palette named, so libghostty's stands");
        assert!(!palette.dark, "a near-white background is light");
    }

    #[test]
    fn the_default_background_is_dark() {
        let settings = DaemonSettings::from(&config("[colors]\nforeground = \"#dddddd\"\n"));
        assert!(settings.palette.expect("a foreground was named").dark);
    }

    #[test]
    fn the_shell_scrollback_and_cursor_are_the_configs() {
        let settings = DaemonSettings::from(&config(
            "scrollback_bytes = 1000\n[shell]\ncommand = \"/bin/zsh\"\nmode = \"login\"\n\
             [cursor]\nstyle = \"bar\"\nblink = false\n",
        ));
        assert_eq!(settings.scrollback_bytes, Some(1000));
        assert_eq!(settings.shell.command.as_deref(), Some("/bin/zsh"));
        assert_eq!(settings.cursor.blink, Some(false));
    }
}
