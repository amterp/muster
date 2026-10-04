//! A pane's headless terminal: every byte its program writes, parsed by the same libghostty-vt
//! a surface runs.
//!
//! This is what queries are answered from, what `pane read` and a replay read, and what a
//! surface is brought back to after it attaches or falls behind. It lives under the pane's own
//! lock, which the pane's reader holds per chunk and nothing holds while waiting on anything
//! else.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use muster_core::diagnostics::poison;
use muster_daemon_proto as proto;
use muster_vt::{Answers, ColorScheme, Palette, Rgb, Terminal, TerminalError, TerminalOptions};

use crate::effects::Happened;
use crate::pty::Grid;
use crate::spawn;
use crate::stream::{Bridge, Refusal, Written};

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
///
/// What that costs: the limit is per screen, so a pane holds 640 MB at worst on its primary and
/// alternate screens, and the surface holds the same images again. A replay carries no images,
/// so whenever one is sent this store is emptied too ([`Screen::attach`]): it never answers for
/// an id the surface no longer has. If the app ever exposes Ghostty's `image-storage-limit`, this
/// takes the same value.
const KITTY_IMAGE_BYTES: u64 = 320_000_000;

/// How long a kitty image still arriving may go without a chunk before the daemon takes its
/// program to be gone.
const LOADING_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Everything the app has said that each pane's terminal applies: what it draws with and
/// allows, and how much history to keep.
///
/// Numbered, because it reaches panes after the session's lock is let go, and two changes in
/// quick succession can reach a pane in either order: a pane applies a generation only if it is
/// newer than the one it has.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settled {
    pub(crate) generation: u64,
    pub(crate) appearance: Appearance,
    pub(crate) scrollback: usize,
    /// How far a wheel turn scrolls the program, as a multiple of Ghostty's own distances.
    pub(crate) scroll_multiplier: f64,
}

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
        let mut answers = Answers {
            color_scheme: self.scheme,
            // As Ghostty answers XTVERSION, and from the version TERM_PROGRAM_VERSION gives.
            version: format!("ghostty {}", spawn::GHOSTTY_VERSION),
            ..Answers::default()
        };
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

/// What a clear_screen did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cleared {
    /// Everything went, at a shell's prompt, and the shell is to redraw it.
    AtPrompt,
    /// History and the rows above the cursor went.
    Elsewhere,
    /// Nothing: the alternate screen is the program's, and the key is too.
    Alternate,
    /// Nothing yet: the pane is held for a handoff, and it is done if the handoff fails.
    Deferred,
}

pub(crate) struct Screen {
    terminal: Terminal,
    /// Where the terminal's effect handler leaves what a write produced, for whoever wrote to
    /// take once the write returns. Only ever locked by a holder of the pane's lock.
    happened: Arc<Mutex<Vec<Happened>>>,
    /// Every byte the terminal has been fed, which is where a stream attached now picks up.
    offset: u64,
    /// Moves whenever the screen may have: every write, and every resize, which rewraps it.
    content_seq: u64,
    /// How many times a program has set the title, counting a repeat of the same title.
    title_writes: u64,
    /// The settings last applied, by generation, with the two a newer one is compared with.
    generation: u64,
    scheme: Option<ColorScheme>,
    scrollback: usize,
    scroll_multiplier: f64,
    /// The bridge drawing this pane, if one is attached.
    bridge: Option<Bridge>,
    /// The size a kitty image still arriving had reached, and when it last grew.
    arriving: Option<(u64, Instant)>,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Screen")
            .field("offset", &self.offset)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Screen {
    pub(crate) fn new(grid: Grid, settled: &Settled) -> Result<Screen, TerminalError> {
        let mut terminal = Terminal::with_options(TerminalOptions {
            cell_pixels: cell_pixels(grid),
            scrollback_bytes: Some(settled.scrollback),
            kitty_image_bytes: Some(KITTY_IMAGE_BYTES),
            answers: settled.appearance.answers(),
            terminfo_name: Some(spawn::TERM.to_string()),
            ..TerminalOptions::new(grid.cols, grid.rows)
        })?;
        let happened = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&happened);
        terminal.set_effect_handler(move |effect| {
            poison::lock(&heard, "daemon.pane.effects").push(Happened::from_effect(&effect));
        });
        let mut screen = Screen {
            terminal,
            happened,
            offset: 0,
            content_seq: 0,
            title_writes: 0,
            generation: settled.generation,
            scheme: settled.appearance.scheme,
            scrollback: settled.scrollback,
            scroll_multiplier: settled.scroll_multiplier,
            bridge: None,
            arriving: None,
        };
        screen.appear(&settled.appearance);
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
        let arrived = self.terminal.kitty_image_loading_bytes();
        if self.arriving.is_none_or(|(bytes, _)| bytes != arrived) {
            self.arriving = (arrived > 0).then(|| (arrived, Instant::now()));
        }
        if !bytes.is_empty() {
            self.content_seq += 1;
        }
        let happened = std::mem::take(&mut *poison::lock(&self.happened, "daemon.pane.effects"));
        self.title_writes +=
            happened.iter().filter(|happening| matches!(happening, Happened::Title(_))).count()
                as u64;
        happened
    }

    pub(crate) fn content_seq(&self) -> u64 {
        self.content_seq
    }

    pub(crate) fn title_writes(&self) -> u64 {
        self.title_writes
    }

    /// How far a wheel turn scrolls the program, as the settings last applied say.
    pub(crate) fn scroll_multiplier(&self) -> f64 {
        self.scroll_multiplier
    }

    /// Applies settings newer than the ones the terminal has, and says what changed: whether
    /// the scheme turned for a program that asked to hear of it (mode 2031), and a scrollback
    /// the terminal would not take. Older settings are ignored.
    pub(crate) fn settle(&mut self, settled: &Settled) -> Option<Settling> {
        if settled.generation <= self.generation {
            return None;
        }
        self.generation = settled.generation;
        self.scroll_multiplier = settled.scroll_multiplier;
        self.appear(&settled.appearance);
        let turned = settled.appearance.scheme.filter(|&scheme| self.scheme != Some(scheme));
        self.scheme = settled.appearance.scheme;
        let mut settling = Settling {
            report: turned
                .filter(|_| self.terminal.mode(muster_vt::Mode::COLOR_SCHEME_REPORT))
                .map(scheme_report),
            scrollback: Ok(()),
        };
        if settled.scrollback != self.scrollback {
            settling.scrollback = self.terminal.set_scrollback_bytes(settled.scrollback);
            if settling.scrollback.is_ok() {
                self.scrollback = settled.scrollback;
            }
        }
        Some(settling)
    }

    fn appear(&mut self, appearance: &Appearance) {
        self.terminal.set_default_palette(&appearance.palette);
        self.terminal.set_default_colors(
            appearance.foreground,
            appearance.background,
            appearance.cursor,
        );
        self.terminal.set_answers(appearance.answers());
    }

    /// Attaches a bridge where the stream stands: told the offset, then a replay composed now,
    /// so no byte is lost or doubled between the replay and the output after it. A bridge
    /// already attached stays unless `takeover`, and is told why it goes.
    pub(crate) fn attach(&mut self, bridge: Bridge, takeover: bool) -> Result<(), Refusal> {
        if self.bridge.is_some() && !takeover {
            return Err(bridge.refused(
                proto::AttachRefusal::AttachedElsewhere,
                "another bridge is drawing this pane; attach with takeover to replace it"
                    .to_string(),
            ));
        }
        if let Some(displaced) = self.bridge.take() {
            displaced.detach(proto::DetachReason::TakenOver);
        }
        bridge.attached(self.offset);
        bridge.replay(&self.terminal.replay());
        self.forget_kitty_images(Instant::now());
        self.bridge = Some(bridge);
        Ok(())
    }

    /// Ghostty's clear_screen ([`Terminal::clear_screen`]), and the surface drawing the pane
    /// sent the result, since nothing in the output stream says what happened. True at a prompt,
    /// where the shell is to be asked to draw its prompt again.
    pub(crate) fn clear_screen(&mut self) -> Cleared {
        if self.terminal.active_screen() == muster_vt::Screen::Alternate {
            return Cleared::Alternate;
        }
        self.content_seq += 1;
        let at_prompt = self.terminal.clear_screen();
        if let Some(bridge) = &self.bridge {
            bridge.replay(&self.terminal.replay());
            self.forget_kitty_images(Instant::now());
        }
        if at_prompt { Cleared::AtPrompt } else { Cleared::Elsewhere }
    }

    /// Lets go of bridge `id`, if it is still the one attached.
    pub(crate) fn detach(&mut self, id: u64) {
        if self.bridge.as_ref().is_some_and(|bridge| bridge.id() == id) {
            self.bridge = None;
        }
    }

    /// Tells the attached bridge why the pane is going, and says when that has been written.
    pub(crate) fn close(&mut self, reason: proto::DetachReason) -> Option<Written> {
        self.bridge.take().map(|bridge| bridge.detach(reason))
    }

    /// Whether the attached bridge's window is full: output waits for its credit.
    pub(crate) fn bridge_is_full(&self) -> bool {
        self.bridge.as_ref().is_some_and(Bridge::is_full)
    }

    /// Takes a bridge's acknowledgement, and catches it up with the screen once a bridge that
    /// fell behind has half its window free again.
    pub(crate) fn acknowledge(&mut self, id: u64, bytes: u64) {
        let Some(bridge) = self.bridge.as_mut().filter(|bridge| bridge.id() == id) else { return };
        if bridge.acknowledge(bytes) {
            bridge.replay(&self.terminal.catch_up());
            self.forget_kitty_images(Instant::now());
        }
    }

    /// Forgets the kitty images, since the surface a replay was just sent to holds none.
    ///
    /// An image still arriving is spared, since forgetting it would fail the chunks to come,
    /// unless it has not grown for [`LOADING_GRACE`]: its program was cut off mid-upload, and
    /// libghostty would keep it arriving, and the images known, until the pane is reset.
    fn forget_kitty_images(&mut self, now: Instant) {
        if self.arriving.is_some_and(|(_, grew)| now.duration_since(grew) >= LOADING_GRACE) {
            self.terminal.drop_kitty_image_loading();
            self.arriving = None;
        }
        self.terminal.forget_kitty_images();
    }

    pub(crate) fn resize(&mut self, grid: Grid) -> Result<(), TerminalError> {
        self.content_seq += 1;
        self.terminal.resize(grid.cols, grid.rows, cell_pixels(grid))
    }

    /// Up to `count` rows of text from `first`, counted from the oldest history still held, one
    /// line per row, with the rows the pane holds in all.
    pub(crate) fn rows(&self, first: u64, count: u32) -> (Vec<String>, u64) {
        let total = self.terminal.total_rows() as u64;
        let covered = total.saturating_sub(first).min(u64::from(count));
        if covered == 0 {
            return (Vec::new(), total);
        }
        let start = u32::try_from(first).unwrap_or(u32::MAX);
        let last = start.saturating_add(u32::try_from(covered - 1).unwrap_or(u32::MAX));
        let text = self.terminal.screen_text(start, last);
        // The formatter drops blank rows at the end of a range; a page keeps one line per row.
        let mut lines: Vec<String> =
            if text.is_empty() { Vec::new() } else { text.split('\n').map(String::from).collect() };
        lines.resize(usize::try_from(covered).unwrap_or(usize::MAX), String::new());
        (lines, total)
    }

    pub(crate) fn terminal(&self) -> &Terminal {
        &self.terminal
    }
}

/// What applying new settings came to.
#[derive(Debug)]
pub(crate) struct Settling {
    /// What to tell the program, which asked to hear when light turns dark or back.
    pub(crate) report: Option<&'static [u8]>,
    pub(crate) scrollback: Result<(), TerminalError>,
}

/// The most text one page of `pane read` holds: a quarter of the largest message, so an answer
/// never comes near the size a client refuses, however deep the scrollback.
pub(crate) const PAGE_BYTES: usize = 4 << 20;

/// Rows formatted per hold of the pane's lock while a page is read, so a long page stalls the
/// pane's output a batch at a time rather than for the whole page.
const PAGE_BATCH: u32 = 256;

/// The most rows a read of the last rows takes under one hold of the screen. Row numbers count
/// from the oldest row held, so output that prunes the top between finding the last row and
/// reading up to it would shift or empty the answer; a hold this short costs the pane's output
/// well under a millisecond, where one across a 4 MiB read would not.
pub(crate) const HELD_TAIL: u32 = 512;

/// A page of text from `first_row`: `rows` of them, or to the last row when `rows` is zero,
/// stopping short at `limit` bytes. `read` returns a batch's lines and the rows held in all.
pub(crate) fn page(
    first_row: u64,
    rows: u32,
    limit: usize,
    mut read: impl FnMut(u64, u32) -> (Vec<String>, u64),
) -> proto::PaneText {
    let end = if rows == 0 { u64::MAX } else { first_row.saturating_add(u64::from(rows)) };
    let mut text = String::new();
    let mut held: u32 = 0;
    let mut total_rows = 0;
    let mut batch = PAGE_BATCH;
    loop {
        let next = first_row + u64::from(held);
        if next >= end {
            break;
        }
        let count = u32::try_from(end - next).unwrap_or(u32::MAX).min(batch);
        let (lines, total) = read(next, count);
        total_rows = total;
        if lines.is_empty() {
            break;
        }
        let joined = lines.join("\n");
        let separator = usize::from(!text.is_empty() || held > 0);
        if text.len() + separator + joined.len() > limit {
            if held > 0 {
                break;
            }
            if count > 1 {
                batch = count / 2;
                continue;
            }
            // One row larger than a page on its own: as much of it as fits.
            let mut cut = limit;
            while !joined.is_char_boundary(cut) {
                cut -= 1;
            }
            text.push_str(&joined[..cut]);
            held = 1;
            break;
        }
        if separator == 1 {
            text.push('\n');
        }
        text.push_str(&joined);
        held += u32::try_from(lines.len()).unwrap_or(u32::MAX);
    }
    proto::PaneText { first_row, text, total_rows, rows: held, turn: None }
}

/// The last `last` rows ending at the last row with anything on it, stopping short at `limit`
/// bytes with the newest rows kept. The blank rest of the screen beneath a prompt is held as
/// rows, and is not what a reader asking for the newest ones wants.
///
/// Found from the bottom a batch at a time and read backwards from there, so no row older than
/// the ones sent is formatted.
pub(crate) fn last_page(
    last: u32,
    limit: usize,
    mut read: impl FnMut(u64, u32) -> (Vec<String>, u64),
) -> proto::PaneText {
    let (end, total_rows) = written_end(&mut read);
    newest_back_to(end.saturating_sub(u64::from(last)), end, total_rows, limit, read)
}

/// The rows from `first` to the last row with anything on it, stopping short at `limit` bytes
/// with the newest rows kept: a turn's output, whose end is what its reader came for.
pub(crate) fn since(
    first: u64,
    limit: usize,
    mut read: impl FnMut(u64, u32) -> (Vec<String>, u64),
) -> proto::PaneText {
    let (end, total_rows) = written_end(&mut read);
    newest_back_to(first.min(end), end, total_rows, limit, read)
}

/// The row after the last one with anything on it, and the rows held in all.
fn written_end(read: &mut impl FnMut(u64, u32) -> (Vec<String>, u64)) -> (u64, u64) {
    let (_, total_rows) = read(0, 0);
    let mut end = total_rows;
    while end > 0 {
        let from = end.saturating_sub(u64::from(PAGE_BATCH));
        let (lines, _) = read(from, u32::try_from(end - from).unwrap_or(PAGE_BATCH));
        if let Some(at) = lines.iter().rposition(|line| !line.trim().is_empty()) {
            return (from + at as u64 + 1, total_rows);
        }
        end = from;
    }
    (0, total_rows)
}

/// The rows from `oldest` up to `end`, read backwards so that `limit` cuts the oldest.
fn newest_back_to(
    oldest: u64,
    end: u64,
    total_rows: u64,
    limit: usize,
    mut read: impl FnMut(u64, u32) -> (Vec<String>, u64),
) -> proto::PaneText {
    let mut newest_first: Vec<String> = Vec::new();
    let mut bytes = 0;
    let mut first_row = end;
    'reading: while first_row > oldest {
        let from = first_row.saturating_sub(u64::from(PAGE_BATCH)).max(oldest);
        let (lines, _) = read(from, u32::try_from(first_row - from).unwrap_or(PAGE_BATCH));
        for line in lines.into_iter().rev() {
            let separator = usize::from(!newest_first.is_empty());
            if bytes + separator + line.len() > limit {
                break 'reading;
            }
            bytes += separator + line.len();
            newest_first.push(line);
            first_row -= 1;
        }
    }
    newest_first.reverse();
    let rows = u32::try_from(newest_first.len()).unwrap_or(u32::MAX);
    proto::PaneText { first_row, text: newest_first.join("\n"), total_rows, rows, turn: None }
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

    fn settled(generation: u64, settings: &proto::Settings) -> Settled {
        Settled {
            generation,
            appearance: Appearance::of(settings),
            scrollback: DEFAULT_SCROLLBACK,
            scroll_multiplier: 1.0,
        }
    }

    #[test]
    fn settings_older_than_the_ones_applied_are_ignored() {
        let grid = Grid { cols: 20, rows: 3, width_px: 0, height_px: 0 };
        let mut screen =
            Screen::new(grid, &settled(1, &proto::Settings::default())).expect("a terminal");
        screen.feed(b"\x1b[?2031h");
        let turned = screen.settle(&settled(3, &palette(proto::ColorScheme::Light)));
        assert_eq!(turned.and_then(|settling| settling.report), Some(&b"\x1b[?997;2n"[..]));
        assert!(screen.settle(&settled(2, &palette(proto::ColorScheme::Dark))).is_none());
        let again = screen.settle(&settled(4, &palette(proto::ColorScheme::Light)));
        assert_eq!(again.and_then(|settling| settling.report), None, "light was light already");
    }

    /// A pane whose image upload was cut off between two chunks, with the shell back at its
    /// prompt.
    fn upload_cut_off() -> Screen {
        let grid = Grid { cols: 20, rows: 3, width_px: 200, height_px: 60 };
        let mut screen =
            Screen::new(grid, &settled(1, &proto::Settings::default())).expect("a terminal");
        screen.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=7,q=2,m=1;AAAA\x1b\\");
        screen.feed(b"\r\n$ ");
        assert!(screen.terminal.kitty_image_loading(), "libghostty keeps it arriving");
        screen
    }

    #[test]
    fn an_upload_cut_off_is_let_go_once_it_has_stopped_growing_for_a_while() {
        let mut screen = upload_cut_off();
        screen.forget_kitty_images(Instant::now() + LOADING_GRACE / 2);
        assert!(screen.terminal.kitty_image_loading(), "a pause is not a program gone");

        screen.forget_kitty_images(Instant::now() + LOADING_GRACE);
        assert!(!screen.terminal.kitty_image_loading(), "the upload was let go");
    }

    #[test]
    fn an_upload_still_growing_is_spared() {
        let mut screen = upload_cut_off();
        let paused = Instant::now() + LOADING_GRACE;
        screen.feed(b"\x1b_Gm=1;AAAA\x1b\\");
        screen.forget_kitty_images(paused);
        assert!(screen.terminal.kitty_image_loading(), "it grew just now");
    }

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
            Screen::new(grid, &settled(0, &proto::Settings::default())).expect("a terminal");
        screen.feed(b"a\r\nb\r\nc\r\nd\r\ne");
        let read = |first_row, rows| page(first_row, rows, PAGE_BYTES, |at, n| screen.rows(at, n));
        let two = read(1, 2);
        assert_eq!((two.first_row, two.text.as_str(), two.total_rows, two.rows), (1, "b\nc", 5, 2));
        assert_eq!(read(3, 0).text, "d\ne", "zero rows reads to the end");
    }

    #[test]
    fn output_and_a_resize_move_the_content_count_and_title_writes_are_counted() {
        let grid = Grid { cols: 20, rows: 3, width_px: 0, height_px: 0 };
        let mut screen =
            Screen::new(grid, &settled(0, &proto::Settings::default())).expect("a terminal");
        screen.feed(b"hello");
        let written = screen.content_seq();
        screen.resize(Grid { cols: 10, ..grid }).expect("a resize");
        assert!(screen.content_seq() > written, "a resize rewraps the screen");
        screen.feed(b"\x1b]2;same\x07\x1b]0;same\x07");
        assert_eq!(screen.title_writes(), 2, "a repeated title is still a write");
    }

    #[test]
    fn a_page_has_a_line_for_every_row_even_a_blank_one() {
        let grid = Grid { cols: 20, rows: 3, width_px: 0, height_px: 0 };
        let mut screen =
            Screen::new(grid, &settled(0, &proto::Settings::default())).expect("a terminal");
        screen.feed(b"a\r\n\r\n\r\nb");
        assert_eq!(screen.rows(0, 3).0, ["a", "", ""]);
        assert_eq!(screen.rows(1, 2).0, ["", ""]);
    }

    /// A pane of `rows` rows, each `width` bytes, read the way a pane's screen is.
    fn rows_of(width: usize, rows: u64) -> impl FnMut(u64, u32) -> (Vec<String>, u64) {
        move |first, count| {
            let last = rows.min(first + u64::from(count));
            ((first..last).map(|row| format!("{row:0width$}")).collect(), rows)
        }
    }

    #[test]
    fn a_page_stops_before_the_batch_that_would_take_it_past_its_limit() {
        let full = page(0, 0, 10 * 1024, rows_of(9, 100_000));
        assert!(full.text.len() <= 10 * 1024);
        assert_eq!(full.rows as usize, full.text.split('\n').count());
        assert_eq!(
            full.text.split('\n').next_back(),
            Some(format!("{:09}", full.rows - 1).as_str())
        );
        assert_eq!(full.total_rows, 100_000);
    }

    #[test]
    fn the_last_rows_are_the_newest_that_fit() {
        let newest = last_page(3, PAGE_BYTES, rows_of(9, 1_000));
        assert_eq!(newest.text, "000000997\n000000998\n000000999");
        assert_eq!((newest.first_row, newest.rows, newest.total_rows), (997, 3, 1_000));
        let cut = last_page(1_000, 30, rows_of(9, 1_000));
        assert_eq!(cut.text, "000000997\n000000998\n000000999", "the newest kept, not the oldest");
        assert_eq!(last_page(5, PAGE_BYTES, rows_of(9, 0)).text, "");
    }

    #[test]
    fn a_turn_reads_from_its_first_row_and_keeps_the_newest_when_it_is_too_long() {
        let turn = since(997, PAGE_BYTES, rows_of(9, 1_000));
        assert_eq!(turn.text, "000000997\n000000998\n000000999");
        assert_eq!((turn.first_row, turn.rows), (997, 3));
        let cut = since(0, 30, rows_of(9, 1_000));
        assert_eq!(cut.first_row, 997, "the newest kept, not the oldest");
        assert_eq!(since(2_000, PAGE_BYTES, rows_of(9, 1_000)).rows, 0, "a turn past the end");
    }

    #[test]
    fn a_first_batch_over_the_limit_is_halved_and_one_huge_row_is_cut() {
        let halved = page(0, 0, 100, rows_of(9, 1_000));
        assert_eq!(halved.rows, 8, "eight 9-byte rows and their newlines fit in 100 bytes");
        let cut = page(5, 0, 100, rows_of(1_000, 10));
        assert_eq!((cut.rows, cut.text.len()), (1, 100));
    }
}
