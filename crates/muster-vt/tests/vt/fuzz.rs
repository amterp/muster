//! Mutated pane output through everything the daemon asks of its terminal (MIP-3 section 13).
//!
//! Every pane's output is untrusted and parsed inside one daemon, so a crash in the parser ends
//! every agent on the machine. This feeds that parser recorded and authored terminal output,
//! mutated, in random chunks, with the resizes, replays, catch-ups, clears and formats the daemon
//! interleaves with output, and passes when nothing crashes. What a replay should contain is the
//! replay conformance's to judge, not this.
//!
//! Deterministic: a fixed seed and count, so the gate runs the same cases every time.
//! `MUSTER_FUZZ_SEED` and `MUSTER_FUZZ_ITERATIONS` run others, or more. Each case has a seed of
//! its own, made from the run's seed and the case's number, which decides both its mutations and
//! how it is run - grid, chunks, and what is interleaved - so a case is its input and its seed
//! alone. One that crashes the process is written to `target/fuzz-crash.bin`, with its seed in
//! `target/fuzz-crash.seed`; both placed in `corpus/fuzz/` under one name run, as they crashed,
//! before the mutated cases from then on.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use muster_vt::{Format, FormatOptions, Mode, Screen, ScreenFormatOptions, Terminal};
use muster_vt::{ScreenExtras, TerminalOptions};

const SEED: u64 = 0x6d75_7374_6572;
const ITERATIONS: u64 = 15_000;

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Sequences a mutation splices in: the families where a terminal parser keeps state across
/// bytes or allocates on the sender's say-so.
const DICTIONARY: &[&[u8]] = &[
    b"\x1b[",
    b"\x1b[999999999;999999999H",
    b"\x1b[1;2;3;4;5;6;7;8;9;10;11;12;13;14;15;16;17;18;19;20;21;22;23;24;25;26;27;28;29;30m",
    b"\x1b[38;2;255;0;0m",
    b"\x1b[48;5;300m",
    b"\x1b[4:3m",
    b"\x1b[?1049h",
    b"\x1b[?1049l",
    b"\x1b[?47h",
    b"\x1b[?2004h",
    b"\x1b[?1000;1006h",
    b"\x1b[?2026h",
    b"\x1b[?25l",
    b"\x1b[3;10r",
    b"\x1b[r",
    b"\x1b[5S",
    b"\x1b[5T",
    b"\x1b[10L",
    b"\x1b[10M",
    b"\x1b[100@",
    b"\x1b[100P",
    b"\x1b[2J",
    b"\x1b[3J",
    b"\x1b[K",
    b"\x1b7",
    b"\x1b8",
    b"\x1bc",
    b"\x1b[!p",
    b"\x1bD",
    b"\x1bM",
    b"\x1b#8",
    b"\x1b(0",
    b"\x1b(B",
    b"\x1b[>1u",
    b"\x1b[<u",
    b"\x1b[=5;1u",
    b"\x1b[?u",
    b"\x1b[6n",
    b"\x1b[c",
    b"\x1b[>q",
    b"\x1b[18t",
    b"\x1b[22;0t",
    b"\x1b[23;0t",
    b"\x1b[2 q",
    b"\x1b]0;title\x07",
    b"\x1b]2;",
    b"\x1b]7;file://host/tmp\x1b\\",
    b"\x1b]8;;https://example.com\x1b\\",
    b"\x1b]8;id=x;https://example.com\x1b\\link\x1b]8;;\x1b\\",
    b"\x1b]8;;\x1b\\",
    b"\x1b]52;c;aGVsbG8=\x07",
    b"\x1b]52;c;?\x07",
    b"\x1b]4;1;rgb:ff/00/00\x07",
    b"\x1b]4;1;?\x07",
    b"\x1b]10;?\x07",
    b"\x1b]11;rgb:00/00/00\x1b\\",
    b"\x1b]104\x07",
    b"\x1b]133;A\x07",
    b"\x1b]133;B\x07",
    b"\x1b]133;C\x07",
    b"\x1b]133;D;0\x07",
    b"\x1b]9;4;1;50\x07",
    b"\x1b]777;notify;t;b\x07",
    b"\x1bP+q544e\x1b\\",
    b"\x1bP$qm\x1b\\",
    b"\x1bP",
    b"\x1b\\",
    b"\x1b_Ga=t,f=24,s=2,v=1,i=1,m=1;AAAA\x1b\\",
    b"\x1b_Gm=1;AAAA\x1b\\",
    b"\x1b_Gm=0;AAAA\x1b\\",
    b"\x1b_Ga=T,f=32,s=1,v=1,i=2;AAAAAA==\x1b\\",
    b"\x1b_Ga=p,i=2,c=10,r=5\x1b\\",
    b"\x1b_Ga=d\x1b\\",
    b"\x1b_Ga=q,i=3;\x1b\\",
    b"\x1b_G",
    "字".as_bytes(),
    "e\u{301}".as_bytes(),
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".as_bytes(),
    "\u{FE0F}".as_bytes(),
    b"\xff\xfe\xc3",
    b"\r\n",
    b"\t",
    b"\x08",
    b"\x0e",
    b"\x0f",
    b"\x18",
    b"\x1a",
    b"\x9b",
    b"\x90",
];

/// splitmix64: enough for picking mutations, and nothing new to depend on.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound.max(1) as u64).expect("below a usize")
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

fn environment(name: &str) -> Option<u64> {
    let value = std::env::var(name).ok()?;
    let parsed =
        value.strip_prefix("0x").map_or_else(|| value.parse(), |hex| u64::from_str_radix(hex, 16));
    Some(parsed.unwrap_or_else(|_| panic!("{name}={value} is not a number")))
}

/// Recorded output from herdr's frames, and every string the replay and catch-up cases feed.
fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for name in ["frame-001-attach.ansi", "frames-after-mode-set.ansi"] {
        let path = Path::new(REPO).join("corpus/herdr-0.8.0/frames").join(name);
        seeds.push(
            std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
        );
    }
    for file in ["replay.json", "catch_up.json"] {
        let path = Path::new(REPO).join("corpus/conformance").join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let document: serde_json::Value = serde_json::from_str(&text).unwrap();
        for case in document["cases"].as_array().expect("cases") {
            let given = case["given"].as_object().expect("given");
            let fed: Vec<u8> = ["stale", "feed", "after"]
                .iter()
                .filter_map(|key| given.get(*key)?.as_str())
                .flat_map(|text| text.bytes())
                .collect();
            if !fed.is_empty() {
                seeds.push(fed);
            }
        }
    }
    seeds
}

/// The cases saved in `corpus/fuzz/`: each `<name>.bin` and the seed in `<name>.seed` beside it.
fn crashes() -> Vec<(PathBuf, Vec<u8>, u64)> {
    let dir = Path::new(REPO).join("corpus/fuzz");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut found: Vec<_> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bin"))
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            let seed_file = path.with_extension("seed");
            let text = std::fs::read_to_string(&seed_file).unwrap_or_else(|error| {
                panic!(
                    "{}: {error}. A saved case runs as it crashed only with the seed written \
                     beside it, target/fuzz-crash.seed, placed here under the same name.",
                    seed_file.display()
                )
            });
            let hex = text.trim().trim_start_matches("0x");
            let seed = u64::from_str_radix(hex, 16)
                .unwrap_or_else(|_| panic!("{}: {text:?} is not a seed", seed_file.display()));
            (path, bytes, seed)
        })
        .collect();
    found.sort();
    found
}

/// The seed of the run's `index`th case.
fn case_seed(seed: u64, index: u64) -> u64 {
    Rng(seed ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15)).next()
}

fn mutated(rng: &mut Rng, seeds: &[Vec<u8>]) -> Vec<u8> {
    let mut input = seeds[rng.below(seeds.len())].clone();
    for _ in 0..=rng.below(8) {
        let at = rng.below(input.len() + 1);
        match rng.below(6) {
            0 if !input.is_empty() => {
                let at = at.min(input.len() - 1);
                input[at] ^= 1 << rng.below(8);
            }
            1 => {
                let byte = u8::try_from(rng.below(256)).unwrap();
                input.insert(at, byte);
            }
            2 if !input.is_empty() => {
                let end = (at + rng.below(32)).min(input.len());
                input.drain(at.min(end)..end);
            }
            3 => {
                let other = &seeds[rng.below(seeds.len())];
                let start = rng.below(other.len());
                let end = (start + rng.below(512)).min(other.len());
                input.splice(at..at, other[start..end].iter().copied());
            }
            4 => {
                // The same sequence many times over, which is how a sender asks for a large
                // allocation or a deep parser state.
                let entry = DICTIONARY[rng.below(DICTIONARY.len())];
                let times = 1 + rng.below(64);
                let repeated: Vec<u8> =
                    entry.iter().copied().cycle().take(entry.len() * times).collect();
                input.splice(at..at, repeated);
            }
            _ => {
                let entry = DICTIONARY[rng.below(DICTIONARY.len())];
                input.splice(at..at, entry.iter().copied());
            }
        }
    }
    input
}

/// A terminal set up as the daemon sets up a pane's, with scrollback small enough to wrap.
fn terminal(columns: u16, rows: u16) -> Terminal {
    let mut terminal = Terminal::with_options(TerminalOptions {
        cell_pixels: (8, 16),
        scrollback_bytes: Some(64 * 1024),
        kitty_image_bytes: Some(4 * 1024 * 1024),
        terminfo_name: Some("xterm-ghostty".to_string()),
        ..TerminalOptions::new(columns, rows)
    })
    .expect("a terminal");
    terminal.set_effect_handler(|_| {});
    terminal
}

/// Everything the daemon does with a pane's terminal between reads of its output, and a few
/// reads the app's requests make.
fn interleave(rng: &mut Rng, terminal: &mut Terminal) {
    match rng.below(10) {
        0 => {
            let columns = u16::try_from(1 + rng.below(250)).unwrap();
            let rows = u16::try_from(1 + rng.below(80)).unwrap();
            let _ = terminal.resize(columns, rows, (8, 16));
        }
        1 => {
            let _ = terminal.clear_screen();
        }
        2 => {
            let replay = terminal.replay();
            if rng.below(2) == 0 {
                terminal.drop_kitty_image_loading();
            }
            terminal.forget_kitty_images();
            let mut other = terminal_like(terminal);
            other.write(&replay);
        }
        3 => {
            let catch_up = terminal.catch_up();
            let mut other = terminal_like(terminal);
            other.write(&catch_up);
        }
        4 => {
            let _ = terminal.format(FormatOptions::vt());
            let _ = terminal.format(FormatOptions::plain());
        }
        5 => {
            let screen = if rng.chance(2) { Screen::Primary } else { Screen::Alternate };
            let _ = terminal.format_screen(
                screen,
                ScreenFormatOptions {
                    format: Format::Vt,
                    unwrap: rng.chance(2),
                    trim: rng.chance(2),
                    content: true,
                    trailing_blank_rows: rng.chance(2),
                    history: rng.chance(2),
                    extras: ScreenExtras::default(),
                },
            );
        }
        6 => {
            let rows = u32::try_from(terminal.total_rows()).unwrap_or(u32::MAX);
            let first = u32::try_from(rng.below(rows as usize + 2)).unwrap();
            let _ = terminal.screen_text(first, first.saturating_add(40));
        }
        7 => {
            let _ = terminal.cursor();
            let _ = terminal.title();
            let _ = terminal.pwd();
            let _ = terminal.mouse_tracking();
            let _ = terminal.kitty_keyboard_flags();
            let _ = terminal.kitty_image_loading();
            let _ = terminal.kitty_image_loading_bytes();
            for mode in Mode::all() {
                let _ = terminal.mode(mode);
            }
        }
        8 => {
            let _ = terminal.set_scrollback_bytes(1024 * (1 + rng.below(128)));
        }
        _ => {}
    }
}

fn terminal_like(terminal: &Terminal) -> Terminal {
    self::terminal(terminal.columns().max(1), terminal.rows().max(1))
}

fn run(rng: &mut Rng, input: &[u8]) {
    let columns = u16::try_from(1 + rng.below(200)).unwrap();
    let rows = u16::try_from(1 + rng.below(60)).unwrap();
    let mut terminal = terminal(columns, rows);
    let mut at = 0;
    while at < input.len() {
        let end = (at + 1 + rng.below(4096)).min(input.len());
        terminal.write(&input[at..end]);
        at = end;
        if rng.chance(3) {
            interleave(rng, &mut terminal);
        }
    }
    let mut replayed = terminal_like(&terminal);
    replayed.write(&terminal.replay());
    replayed.write(input);
    let _ = replayed.format(FormatOptions::vt());
}

/// Where the case being run lives, for the crash handler to write out.
static CASE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static CASE_LENGTH: AtomicUsize = AtomicUsize::new(0);
static CASE_SEED: AtomicU64 = AtomicU64::new(0);
static CRASH_FILES: OnceLock<(CString, CString)> = OnceLock::new();

extern "C" fn on_crash(signal: libc::c_int) {
    // Only async-signal-safe calls from here: open, write, close, signal, raise.
    let case = CASE.load(Ordering::SeqCst);
    if let (Some((input, seed)), false) = (CRASH_FILES.get(), case.is_null()) {
        let length = CASE_LENGTH.load(Ordering::SeqCst);
        let mut hex = *b"0x0000000000000000\n";
        let value = CASE_SEED.load(Ordering::SeqCst);
        for (at, digit) in hex[2..18].iter_mut().enumerate() {
            let nibble = (value >> (60 - 4 * at)) & 0xf;
            *digit = b"0123456789abcdef"[usize::try_from(nibble).unwrap_or(0)];
        }
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
        // SAFETY: the pointer and length describe the case `fuzz` is running, which it keeps
        // alive until it has cleared them; the paths are NUL-terminated strings set once.
        unsafe {
            for (path, bytes, count) in [(input, case, length), (seed, hex.as_mut_ptr(), hex.len())]
            {
                let file = libc::open(path.as_ptr(), flags, 0o644);
                if file >= 0 {
                    libc::write(file, bytes.cast(), count);
                    libc::close(file);
                }
            }
            let said = b"\nfuzz: the case that crashed the terminal is in target/fuzz-crash.bin, \
                         its seed in target/fuzz-crash.seed\n";
            libc::write(2, said.as_ptr().cast(), said.len());
        }
    }
    // SAFETY: as above.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Runs one case, from its seed, with the crash handler told where it is, and untold before it
/// is freed.
fn watched(seed: u64, input: &[u8]) {
    CASE_SEED.store(seed, Ordering::SeqCst);
    CASE_LENGTH.store(input.len(), Ordering::SeqCst);
    CASE.store(input.as_ptr().cast_mut(), Ordering::SeqCst);
    run(&mut Rng(seed), input);
    CASE.store(std::ptr::null_mut(), Ordering::SeqCst);
}

/// The signals a crash in the parser arrives as.
const CRASHES: [libc::c_int; 5] =
    [libc::SIGABRT, libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGTRAP];

#[test]
fn mutated_output_never_crashes_the_terminal() {
    let target = Path::new(REPO).join("target");
    let path = |name: &str| CString::new(target.join(name).as_os_str().as_encoded_bytes()).unwrap();
    CRASH_FILES.set((path("fuzz-crash.bin"), path("fuzz-crash.seed"))).unwrap();
    // SAFETY: the handler makes only async-signal-safe calls. The handlers found are put back
    // at the end, so a crash in whatever runs in this process afterwards is not reported as this.
    let previous = CRASHES
        .map(|signal| unsafe { libc::signal(signal, on_crash as *const () as libc::sighandler_t) });

    let seed = environment("MUSTER_FUZZ_SEED").unwrap_or(SEED);
    let iterations = environment("MUSTER_FUZZ_ITERATIONS").unwrap_or(ITERATIONS);

    for (path, input, case) in crashes() {
        eprintln!("fuzz: rerunning {}", path.display());
        watched(case, &input);
    }

    let seeds = seeds();
    let started = std::time::Instant::now();
    for index in 0..iterations {
        let case = case_seed(seed, index);
        let input = mutated(&mut Rng(case), &seeds);
        watched(case, &input);
    }
    for (signal, handler) in CRASHES.into_iter().zip(previous) {
        // SAFETY: restores what was installed before.
        unsafe { libc::signal(signal, handler) };
    }
    eprintln!(
        "fuzz: {iterations} cases from seed {seed:#x} in {:.1?}; none crashed",
        started.elapsed()
    );
}
