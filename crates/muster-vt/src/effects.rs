//! What a program's output asks of the world outside the terminal.
//!
//! A bell, a title, a desktop notification, a clipboard write, and the answers to a
//! program's queries all arrive in the middle of `write`, as libghostty-vt callbacks. The
//! daemon is the only thing answering a pane's queries and the only source of its effects
//! (MIP-3 section 7), so these are how it hears about them.
//!
//! Every callback runs synchronously inside `ghostty_terminal_vt_write`, on the thread that
//! is parsing the pane's output. A handler must not block, and cannot reach the terminal:
//! the terminal is mid-write, which the borrow in `Terminal::write` already makes true.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::ffi;

/// One thing a program's output asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect<'a> {
    /// Bytes answering a query, to be written to the program's input.
    Reply(&'a [u8]),
    Bell,
    /// OSC 0 or 2.
    Title(&'a str),
    /// OSC 7, as the URL the program reported.
    Pwd(&'a str),
    /// OSC 52 (or iTerm2's OSC 1337 Copy), already decoded. No representations means clear.
    ClipboardWrite {
        location: ClipboardLocation,
        contents: Vec<ClipboardContent<'a>>,
    },
    /// OSC 9 or 777.
    Notification {
        title: &'a str,
        body: &'a str,
    },
    /// OSC 9;4.
    Progress(Progress),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardLocation {
    Standard,
    Selection,
    Primary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardContent<'a> {
    pub mime: &'a str,
    pub data: &'a [u8],
}

/// What a progress report asks the app to show. Percentages are 0 through 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Remove,
    Set(u8),
    Error(Option<u8>),
    Indeterminate,
    Pause(Option<u8>),
}

/// The terminal a program is told it is talking to.
///
/// Defaults are Ghostty's own answers (`src/termio/stream_handler.zig`), because a pane runs
/// as `xterm-ghostty` and a program that checks should find what that name promises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answers {
    /// XTVERSION (`CSI > q`). Empty answers as libghostty.
    pub version: String,
    /// DA1: conformance level and feature codes.
    pub primary_attributes: (u16, Vec<u16>),
    /// DA2: device type, firmware version, ROM cartridge.
    pub secondary_attributes: (u16, u16, u16),
    /// The reply to ENQ. Empty sends nothing.
    pub enquiry: Vec<u8>,
    /// The answer to `CSI ? 996 n`, and what mode 2031 reports. None ignores the query.
    pub color_scheme: Option<ColorScheme>,
}

impl Default for Answers {
    fn default() -> Answers {
        Answers {
            version: String::new(),
            // 62 is VT220 conformance, 22 ANSI color, 52 clipboard access - which Ghostty
            // claims while clipboard writes are allowed, its default and Muster's.
            primary_attributes: (62, vec![22, 52]),
            secondary_attributes: (1, 10, 0),
            enquiry: Vec::new(),
            color_scheme: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

pub(crate) type EffectHandler = Box<dyn FnMut(Effect<'_>) + Send>;

/// What the callbacks reach through libghostty's userdata pointer. Boxed by the terminal,
/// so its address holds while the terminal moves.
pub(crate) struct Effects {
    pub(crate) handler: Option<EffectHandler>,
    pub(crate) answers: Answers,
}

impl Effects {
    fn emit(&mut self, effect: Effect<'_>) {
        if let Some(handler) = self.handler.as_mut() {
            // A handler that panics must not unwind into Zig, which is undefined behavior;
            // and aborting would end every agent in the daemon for one pane's bug. The
            // effect is dropped instead.
            let _ = catch_unwind(AssertUnwindSafe(|| handler(effect)));
        }
    }
}

/// Registers every callback on `terminal`, reaching `effects`.
///
/// # Safety
///
/// `effects` must stay at this address, and alive, until the terminal is freed.
pub(crate) unsafe fn register(terminal: ffi::GhosttyTerminal, effects: *mut Effects) {
    // Checks each function against the callback type terminal.h documents for its option,
    // then hands libghostty the function pointer itself, which is what the option takes.
    macro_rules! callback {
        ($type:ty, $function:expr) => {{
            let typed: $type = Some($function);
            typed.map_or(std::ptr::null(), |function| function as *const c_void)
        }};
    }

    let options: [(ffi::GhosttyTerminalOption, *const c_void); 14] = [
        (ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA, effects.cast_const().cast()),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            callback!(ffi::GhosttyTerminalWritePtyFn, write_pty),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL,
            callback!(ffi::GhosttyTerminalBellFn, bell),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            callback!(ffi::GhosttyTerminalTitleChangedFn, title_changed),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
            callback!(ffi::GhosttyTerminalPwdChangedFn, pwd_changed),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
            callback!(ffi::GhosttyTerminalClipboardWriteFn, clipboard_write),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
            callback!(ffi::GhosttyTerminalDesktopNotificationFn, notification),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT,
            callback!(ffi::GhosttyTerminalProgressReportFn, progress),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_XTVERSION,
            callback!(ffi::GhosttyTerminalXtversionFn, xtversion),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_ENQUIRY,
            callback!(ffi::GhosttyTerminalEnquiryFn, enquiry),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
            callback!(ffi::GhosttyTerminalDeviceAttributesFn, device_attributes),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
            callback!(ffi::GhosttyTerminalColorSchemeFn, color_scheme),
        ),
        (
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SIZE,
            callback!(ffi::GhosttyTerminalSizeFn, size),
        ),
        // Off in libghostty by default, and kept off: a program can set a title and have it
        // read back into its input, which types a command the user never did.
        (ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_REPORT, std::ptr::null()),
    ];
    for (option, value) in options {
        // SAFETY: the caller keeps `effects` alive at this address; every callback was
        // checked against its option's type above, and libghostty copies the pointers.
        unsafe { ffi::ghostty_terminal_set(terminal, option, value) };
    }
}

/// The `Effects` behind a callback's userdata.
///
/// # Safety
///
/// `userdata` is the pointer `register` installed, and no other reference to it is live:
/// callbacks run one at a time, inside a `write` that holds the terminal mutably.
unsafe fn effects<'a>(userdata: *mut c_void) -> &'a mut Effects {
    // SAFETY: as the function's contract says.
    unsafe { &mut *userdata.cast::<Effects>() }
}

/// # Safety
///
/// `raw` came from libghostty and is valid for the duration of the callback.
unsafe fn text<'a>(raw: ffi::GhosttyString) -> &'a str {
    if raw.ptr.is_null() || raw.len == 0 {
        return "";
    }
    // SAFETY: libghostty reports the borrowed bytes' true length.
    let bytes = unsafe { std::slice::from_raw_parts(raw.ptr, raw.len) };
    std::str::from_utf8(bytes).unwrap_or("")
}

/// # Safety
///
/// As for `text`.
unsafe fn bytes<'a>(raw: ffi::GhosttyString) -> &'a [u8] {
    if raw.ptr.is_null() || raw.len == 0 {
        return &[];
    }
    // SAFETY: libghostty reports the borrowed bytes' true length.
    unsafe { std::slice::from_raw_parts(raw.ptr, raw.len) }
}

fn terminal_string(
    terminal: ffi::GhosttyTerminal,
    data: ffi::GhosttyTerminalData,
) -> ffi::GhosttyString {
    let mut raw = ffi::GhosttyString { ptr: std::ptr::null(), len: 0 };
    // SAFETY: TITLE and PWD write a borrowed string. Reading is not the re-entrant write
    // the callback contract forbids, and libghostty stores the new value before calling.
    unsafe { ffi::ghostty_terminal_get(terminal, data, (&raw mut raw).cast()) };
    raw
}

unsafe extern "C" fn write_pty(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    length: usize,
) {
    if data.is_null() {
        return;
    }
    // SAFETY: userdata is ours (see `effects`); the bytes are valid for this call.
    let (effects, reply) = unsafe { (effects(userdata), std::slice::from_raw_parts(data, length)) };
    effects.emit(Effect::Reply(reply));
}

unsafe extern "C" fn bell(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: userdata is ours.
    unsafe { effects(userdata) }.emit(Effect::Bell);
}

unsafe extern "C" fn title_changed(terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    let raw = terminal_string(terminal, ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE);
    // SAFETY: userdata is ours; the string is the terminal's, valid for this call.
    unsafe { effects(userdata).emit(Effect::Title(text(raw))) };
}

unsafe extern "C" fn pwd_changed(terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    let raw = terminal_string(terminal, ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD);
    // SAFETY: as above.
    unsafe { effects(userdata).emit(Effect::Pwd(text(raw))) };
}

unsafe extern "C" fn clipboard_write(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const ffi::GhosttyClipboardWrite,
) -> ffi::GhosttyClipboardWriteResult {
    // SAFETY: libghostty passes a live request for the duration of the call.
    let Some(write) = (unsafe { write.as_ref() }) else {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    };
    let location = match write.location {
        ffi::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_SELECTION => {
            ClipboardLocation::Selection
        }
        ffi::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_PRIMARY => {
            ClipboardLocation::Primary
        }
        _ => ClipboardLocation::Standard,
    };
    let raw = if write.contents.is_null() || write.contents_len == 0 {
        &[][..]
    } else {
        // SAFETY: libghostty reports the borrowed array's true length.
        unsafe { std::slice::from_raw_parts(write.contents, write.contents_len) }
    };
    // SAFETY: every string is borrowed for the duration of the call.
    let contents = raw
        .iter()
        .map(|content| unsafe {
            ClipboardContent { mime: text(content.mime), data: bytes(content.data) }
        })
        .collect();
    // SAFETY: userdata is ours.
    unsafe { effects(userdata) }.emit(Effect::ClipboardWrite { location, contents });
    // OSC 52 has no acknowledgement, so whether the app applies it is its policy, not an
    // answer the program can receive.
    ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS
}

unsafe extern "C" fn notification(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    request: *const ffi::GhosttyTerminalDesktopNotification,
) {
    // SAFETY: libghostty passes a live request for the duration of the call.
    let Some(request) = (unsafe { request.as_ref() }) else { return };
    // SAFETY: userdata is ours; both strings are borrowed for this call.
    unsafe {
        effects(userdata)
            .emit(Effect::Notification { title: text(request.title), body: text(request.body) });
    }
}

unsafe extern "C" fn progress(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const ffi::GhosttyTerminalProgressReport,
) {
    // SAFETY: libghostty passes a live report for the duration of the call.
    let Some(report) = (unsafe { report.as_ref() }) else { return };
    let percent = u8::try_from(report.progress).ok().map(|value| value.min(100));
    let progress = match report.state {
        ffi::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_SET => {
            Progress::Set(percent.unwrap_or(0))
        }
        ffi::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR => {
            Progress::Error(percent)
        }
        ffi::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE => {
            Progress::Indeterminate
        }
        ffi::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE => {
            Progress::Pause(percent)
        }
        _ => Progress::Remove,
    };
    // SAFETY: userdata is ours.
    unsafe { effects(userdata) }.emit(Effect::Progress(progress));
}

unsafe extern "C" fn xtversion(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) -> ffi::GhosttyString {
    // SAFETY: userdata is ours. The string lives in `Effects`, which outlives the call.
    let version = unsafe { &effects(userdata).answers.version };
    ffi::GhosttyString { ptr: version.as_ptr(), len: version.len() }
}

unsafe extern "C" fn enquiry(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) -> ffi::GhosttyString {
    // SAFETY: as above.
    let reply = unsafe { &effects(userdata).answers.enquiry };
    ffi::GhosttyString { ptr: reply.as_ptr(), len: reply.len() }
}

unsafe extern "C" fn device_attributes(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut ffi::GhosttyDeviceAttributes,
) -> bool {
    // SAFETY: userdata is ours; `out` is libghostty's, writable for this call.
    let (answers, Some(out)) = (unsafe { &effects(userdata).answers }, unsafe { out.as_mut() })
    else {
        return false;
    };
    let (level, features) = &answers.primary_attributes;
    out.primary.conformance_level = *level;
    let count = features.len().min(out.primary.features.len());
    out.primary.features[..count].copy_from_slice(&features[..count]);
    out.primary.num_features = count;
    let (device_type, firmware, cartridge) = answers.secondary_attributes;
    out.secondary.device_type = device_type;
    out.secondary.firmware_version = firmware;
    out.secondary.rom_cartridge = cartridge;
    out.tertiary.unit_id = 0;
    true
}

unsafe extern "C" fn color_scheme(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut ffi::GhosttyColorScheme,
) -> bool {
    // SAFETY: userdata is ours; `out` is libghostty's, writable for this call.
    let (answers, Some(out)) = (unsafe { &effects(userdata).answers }, unsafe { out.as_mut() })
    else {
        return false;
    };
    let Some(scheme) = answers.color_scheme else { return false };
    *out = match scheme {
        ColorScheme::Light => ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_LIGHT,
        ColorScheme::Dark => ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK,
    };
    true
}

/// XTWINOPS size reports, from the grid and the pixel size the terminal was last given.
unsafe extern "C" fn size(
    terminal: ffi::GhosttyTerminal,
    _userdata: *mut c_void,
    out: *mut ffi::GhosttySizeReportSize,
) -> bool {
    // SAFETY: `out` is libghostty's, writable for this call.
    let Some(out) = (unsafe { out.as_mut() }) else { return false };
    let mut columns: u16 = 0;
    let mut rows: u16 = 0;
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    // SAFETY: each out pointer is to a local of the type documented for its data kind.
    unsafe {
        ffi::ghostty_terminal_get(
            terminal,
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS,
            (&raw mut columns).cast(),
        );
        ffi::ghostty_terminal_get(
            terminal,
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS,
            (&raw mut rows).cast(),
        );
        ffi::ghostty_terminal_get(
            terminal,
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_WIDTH_PX,
            (&raw mut width).cast(),
        );
        ffi::ghostty_terminal_get(
            terminal,
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_HEIGHT_PX,
            (&raw mut height).cast(),
        );
    }
    if columns == 0 || rows == 0 || width == 0 || height == 0 {
        // No pixel size yet: better no answer than one claiming zero-pixel cells.
        return false;
    }
    *out = ffi::GhosttySizeReportSize {
        rows,
        columns,
        cell_width: width / u32::from(columns),
        cell_height: height / u32::from(rows),
    };
    true
}
