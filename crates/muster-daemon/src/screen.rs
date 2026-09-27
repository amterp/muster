//! A pane's headless terminal: every byte its program writes, parsed by the same libghostty-vt
//! a surface runs.
//!
//! This is what queries are answered from, what `pane read` and a replay read, and what a
//! surface is brought back to after it attaches or falls behind. It lives under the pane's own
//! lock, which the pane's reader holds per chunk and nothing holds while waiting on anything
//! else.

use std::sync::{Arc, Mutex, OnceLock};

use muster_core::diagnostics::poison;
use muster_daemon_proto as proto;
use muster_vt::{Answers, ColorScheme, Palette, Rgb, Terminal, TerminalError, TerminalOptions};

use crate::effects::Happened;
use crate::pty::Grid;
use crate::stream::{Bridge, Refusal};

/// History a pane keeps when the app has not said: Ghostty's own `scrollback-limit`, so the
/// daemon's copy holds what the surface beside it holds.
pub(crate) const DEFAULT_SCROLLBACK: usize = 10_000_000;

/// Bytes of kitty graphics kept per screen: Ghostty's own `image-storage-limit`.
///
/// The daemon answers a program's image commands and the surface's answers are discarded, so
/// the daemon's store has to say what the surface's would. libghostty evicts the oldest image
/// to make room and fails only when one image is larger than the whole store, so any smaller
/// store refuses an image the surface would take, or forgets an id the surface still holds.
/// The bytes are spent only by panes whose programs send images, which the surface holds too.
const KITTY_IMAGE_BYTES: u64 = 320_000_000;

/// What the app draws with and allows, as the terminal is told it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Appearance {
    palette: Palette,
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    cursor: Option<Rgb>,
    pub(crate) scheme: Option<ColorScheme>,
    clipboard_write: bool,
}

impl Appearance {
    pub(crate) fn of(settings: &proto::Settings) -> Appearance {
        let mut palette = *builtin_palette();
        let mut appearance = Appearance {
            palette,
            foreground: None,
            background: None,
            cursor: None,
            scheme: None,
            clipboard_write: settings.clipboard_write.unwrap_or(true),
        };
        if let Some(sent) = &settings.palette {
            for (entry, &color) in palette.iter_mut().zip(&sent.entries) {
                *entry = rgb(color);
            }
            appearance.palette = palette;
            appearance.foreground = Some(rgb(sent.foreground));
            appearance.background = Some(rgb(sent.background));
            appearance.cursor = sent.cursor.map(rgb);
            appearance.scheme = match sent.scheme() {
                proto::ColorScheme::Dark => Some(ColorScheme::Dark),
                proto::ColorScheme::Light => Some(ColorScheme::Light),
                proto::ColorScheme::Unspecified => None,
            };
        }
        appearance
    }

    fn answers(&self) -> Answers {
        let mut answers = Answers { color_scheme: self.scheme, ..Answers::default() };
        // Clipboard access (52) is claimed only while writes are allowed, as Ghostty claims it.
        if !self.clipboard_write {
            answers.primary_attributes.1.retain(|&feature| feature != 52);
        }
        answers
    }
}

/// libghostty's own palette, for the entries an app's palette leaves out.
fn builtin_palette() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        Terminal::new(1, 1).map_or([Rgb::default(); 256], |terminal| terminal.default_palette())
    })
}

fn rgb(color: u32) -> Rgb {
    let [_, r, g, b] = color.to_be_bytes();
    Rgb { r, g, b }
}

/// What a program is told when the app's appearance changes, if it asked (mode 2031).
pub(crate) fn scheme_report(scheme: ColorScheme) -> &'static [u8] {
    match scheme {
        ColorScheme::Dark => b"\x1b[?997;1n",
        ColorScheme::Light => b"\x1b[?997;2n",
    }
}

pub(crate) struct Screen {
    terminal: Terminal,
    /// Where the terminal's effect handler leaves what a write produced, for whoever wrote to
    /// take once the write returns. Only ever locked by a holder of the pane's lock.
    happened: Arc<Mutex<Vec<Happened>>>,
    /// Every byte the terminal has been fed, which is where a stream attached now picks up.
    offset: u64,
    grid: Grid,
    /// The bridge drawing this pane, if one is attached.
    bridge: Option<Bridge>,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Screen")
            .field("offset", &self.offset)
            .field("grid", &self.grid)
            .finish_non_exhaustive()
    }
}

impl Screen {
    pub(crate) fn new(
        grid: Grid,
        scrollback: usize,
        appearance: &Appearance,
    ) -> Result<Screen, TerminalError> {
        let mut terminal = Terminal::with_options(TerminalOptions {
            cell_pixels: cell_pixels(grid),
            scrollback_bytes: Some(scrollback),
            kitty_image_bytes: Some(KITTY_IMAGE_BYTES),
            answers: appearance.answers(),
            ..TerminalOptions::new(grid.cols, grid.rows)
        })?;
        let happened = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&happened);
        terminal.set_effect_handler(move |effect| {
            poison::lock(&heard, "daemon.pane.effects").push(Happened::from_effect(&effect));
        });
        let mut screen = Screen { terminal, happened, offset: 0, grid, bridge: None };
        screen.appear(appearance);
        Ok(screen)
    }

    /// Output for the pane: to the attached bridge first, so a surface never waits on the
    /// parse, then to the terminal. Returns what it asked for.
    pub(crate) fn output(&mut self, bytes: &[u8]) -> Vec<Happened> {
        if let Some(bridge) = &mut self.bridge {
            bridge.offer(bytes);
        }
        self.feed(bytes)
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<Happened> {
        self.terminal.write(bytes);
        self.offset += bytes.len() as u64;
        std::mem::take(&mut *poison::lock(&self.happened, "daemon.pane.effects"))
    }

    pub(crate) fn appear(&mut self, appearance: &Appearance) {
        self.terminal.set_default_palette(&appearance.palette);
        self.terminal.set_default_colors(
            appearance.foreground,
            appearance.background,
            appearance.cursor,
        );
        self.terminal.set_answers(appearance.answers());
    }

    pub(crate) fn set_scrollback(&mut self, bytes: usize) -> Result<(), TerminalError> {
        self.terminal.set_scrollback_bytes(bytes)
    }

    /// Attaches a bridge where the stream stands: told the offset, then a replay composed now,
    /// so no byte is lost or doubled between the replay and the output after it. A bridge
    /// already attached stays unless `takeover`, and is told why it goes.
    pub(crate) fn attach(&mut self, bridge: Bridge, takeover: bool) -> Result<(), Refusal> {
        if self.bridge.is_some() && !takeover {
            return Err(bridge.refused(
                "another bridge is drawing this pane; attach with takeover to replace it"
                    .to_string(),
            ));
        }
        if let Some(displaced) = self.bridge.take() {
            displaced.detach(proto::DetachReason::TakenOver);
        }
        bridge.attached(self.offset);
        bridge.replay(&self.terminal.replay());
        self.bridge = Some(bridge);
        Ok(())
    }

    /// Lets go of bridge `id`, if it is still the one attached.
    pub(crate) fn detach(&mut self, id: u64) {
        if self.bridge.as_ref().is_some_and(|bridge| bridge.id() == id) {
            self.bridge = None;
        }
    }

    /// Tells the attached bridge why the pane is going.
    pub(crate) fn close(&mut self, reason: proto::DetachReason) {
        if let Some(bridge) = self.bridge.take() {
            bridge.detach(reason);
        }
    }

    /// Takes a bridge's acknowledgement, and catches it up with the screen once a bridge that
    /// fell behind has acknowledged everything it was sent.
    pub(crate) fn acknowledge(&mut self, id: u64, bytes: u64) {
        let Some(bridge) = self.bridge.as_mut().filter(|bridge| bridge.id() == id) else { return };
        if bridge.acknowledge(bytes) {
            bridge.replay(&self.terminal.catch_up());
        }
    }

    pub(crate) fn resize(&mut self, grid: Grid) -> Result<(), TerminalError> {
        self.terminal.resize(grid.cols, grid.rows, cell_pixels(grid))?;
        self.grid = grid;
        Ok(())
    }

    /// A page of the active screen's text, rows counted from the oldest history still held.
    pub(crate) fn text(&self, first_row: u64, rows: u32) -> proto::PaneText {
        let total_rows = self.terminal.total_rows() as u64;
        let first = u32::try_from(first_row).unwrap_or(u32::MAX);
        let last = if rows == 0 { u32::MAX } else { first.saturating_add(rows - 1) };
        proto::PaneText { first_row, text: self.terminal.screen_text(first, last), total_rows }
    }

    pub(crate) fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub(crate) fn grid(&self) -> Grid {
        self.grid
    }
}

/// One cell's size in pixels, zero while no surface has said.
fn cell_pixels(grid: Grid) -> (u32, u32) {
    (
        u32::from(grid.width_px) / u32::from(grid.cols.max(1)),
        u32::from(grid.height_px) / u32::from(grid.rows.max(1)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(scheme: proto::ColorScheme) -> proto::Settings {
        proto::Settings {
            palette: Some(proto::Palette {
                entries: vec![0x11_22_33],
                foreground: 0xff_ff_ff,
                background: 0x10_20_30,
                cursor: None,
                scheme: scheme.into(),
            }),
            ..proto::Settings::default()
        }
    }

    #[test]
    fn a_short_palette_keeps_libghostty_for_the_entries_it_leaves_out() {
        let appearance = Appearance::of(&palette(proto::ColorScheme::Dark));
        assert_eq!(appearance.palette[0], Rgb { r: 0x11, g: 0x22, b: 0x33 });
        assert_eq!(appearance.palette[1..], builtin_palette()[1..]);
        assert_eq!(appearance.background, Some(Rgb { r: 0x10, g: 0x20, b: 0x30 }));
        assert_eq!(appearance.scheme, Some(ColorScheme::Dark));
    }

    #[test]
    fn clipboard_access_is_claimed_only_while_writes_are_allowed() {
        let allowed = Appearance::of(&proto::Settings::default()).answers();
        assert!(allowed.primary_attributes.1.contains(&52));
        let denied = Appearance::of(&proto::Settings {
            clipboard_write: Some(false),
            ..proto::Settings::default()
        })
        .answers();
        assert!(!denied.primary_attributes.1.contains(&52));
        assert!(denied.primary_attributes.1.contains(&22), "only clipboard access goes");
    }

    #[test]
    fn a_page_of_text_counts_from_the_oldest_row() {
        let grid = Grid { cols: 20, rows: 3, width_px: 0, height_px: 0 };
        let mut screen =
            Screen::new(grid, DEFAULT_SCROLLBACK, &Appearance::of(&proto::Settings::default()))
                .expect("a terminal");
        screen.feed(b"a\r\nb\r\nc\r\nd\r\ne");
        let page = screen.text(1, 2);
        assert_eq!((page.first_row, page.text.as_str(), page.total_rows), (1, "b\nc", 5));
        assert_eq!(screen.text(3, 0).text, "d\ne", "zero rows reads to the end");
    }
}
