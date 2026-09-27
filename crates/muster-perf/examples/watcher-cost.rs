//! What following a daemon with fifteen panes costs this process, against following one pane.
//!
//! Under herdr this was the price of agent state: its per-pane status event took a pane id and
//! nothing session-wide carried the same thing, so an overview of N panes was N held-open
//! connections and N threads. muster-daemon's subscription carries every pane's state on one
//! connection (MIP-3, section 9), so the per-pane column should now read zero threads and zero
//! descriptors, with memory growing only by what the mirror holds per pane. This is the check
//! that it does.
//!
//! What it leaves out is what the window binds per pane beside the follower, the socket a
//! pane's bridge reports on, which is the seam's and is budgeted as `pane.channel` in
//! `src/main.rs`.
//!
//! An example rather than a budget entry, because a resident-memory reading is too noisy to
//! fail a build over. The gate compiles this (`cargo build --workspace --all-targets`) so it
//! cannot rot, and never runs it.
//!
//! Run it:
//!
//! ```text
//! cargo build --release -p muster-daemon
//! cargo run --release -p muster-perf --example watcher-cost
//! ```
//!
//! Release because a debug binary's memory says nothing about the one anybody ships, and the
//! daemon is built first because the harness starts the one beside this example.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::mirror::Mirror;
use muster_daemon_client::follow::{Follower, Following, Notice};
use muster_daemon_proto as proto;
use muster_harness::requests::{beside, create, create_request, in_new_tab};
use muster_harness::{Control, Daemon, until};

/// The window Muster budgets for.
const BUDGETED_PANES: usize = 15;

/// How long the process is watched doing nothing, to answer the scheduling half.
///
/// Threads blocked in `read` should cost no CPU at all. Long enough that a per-thread
/// wakeup storm would show up as milliseconds rather than rounding.
const IDLE_WINDOW: Duration = Duration::from_secs(3);

fn main() {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    let first = "p0";
    if !made(&mut control, create(first, in_new_tab("t1"))) {
        eprintln!(
            "watcher-cost: the daemon would not make a first pane, so there is nothing to measure."
        );
        std::process::exit(1);
    }

    let mirror = Arc::new(Mutex::new(Mirror::new()));
    let bootstraps = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&bootstraps);
    let _follower = Follower::start(
        Following {
            socket: daemon.socket_path().to_path_buf(),
            client: "watcher-cost".to_string(),
            daemon: "local".to_string(),
            remote: false,
        },
        Arc::clone(&mirror),
        Arc::new(move |notice| {
            if matches!(notice, Notice::Bootstrapped { .. }) {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }),
    )
    .unwrap_or_else(|error| {
        eprintln!("watcher-cost: could not start following the daemon: {error}");
        std::process::exit(1);
    });

    until("the first bootstrap", || bootstraps.load(Ordering::Relaxed) > 0, ());
    until("the first pane to reach the mirror", || panes(&mirror) == 1, ());
    settle();
    let one = Stats::read();

    let reached = grow_to(&mut control, &mirror, BUDGETED_PANES);
    settle();
    let many = Stats::read();

    let idle = idle_cpu();

    report(&one, &many, reached, idle);
}

/// Asks for a pane, and says whether the daemon made it.
fn made(control: &mut Control, pane: proto::pane_request::Create) -> bool {
    control.ask(create_request(pane)).outcome() == proto::Outcome::Done
}

/// Splits until the session holds `target` panes, or until the daemon will not split again.
///
/// Reports what it reached rather than insisting: a daemon may refuse a split once panes would
/// be too small to hold a terminal, and a rig that panicked there would turn a fact about the
/// daemon into a broken tool.
fn grow_to(control: &mut Control, mirror: &Arc<Mutex<Mirror>>, target: usize) -> usize {
    while panes(mirror) < target {
        let before = panes(mirror);
        let name = format!("p{before}");
        let previous = format!("p{}", before - 1);
        if !made(control, create(&name, beside(&previous, proto::Side::Right))) {
            eprintln!(
                "watcher-cost: the daemon refused a split at {before} panes; reporting that."
            );
            break;
        }
        // One at a time, so the count below is the count the follower was measured at.
        until("the new pane to reach the mirror", || panes(mirror) > before, ());
    }
    panes(mirror)
}

fn panes(mirror: &Arc<Mutex<Mirror>>) -> usize {
    mirror.lock().expect("the mirror lock was poisoned").panes().count()
}

/// Long enough for anything the follower starts in reaction to a pane to have started.
///
/// A sleep rather than a poll, because there should be nothing to wait for: a follower that
/// started something per pane is exactly what this looks for, and it cannot poll for a thing
/// it expects not to exist.
fn settle() {
    std::thread::sleep(Duration::from_millis(500));
}

/// User plus system CPU this process burns while nothing at all is happening.
///
/// The scheduling half of the question. Threads parked in a blocking `read` should cost
/// nothing; anything else here means the shape is polling when it looks like it is waiting.
fn idle_cpu() -> Duration {
    let before = cpu_used();
    std::thread::sleep(IDLE_WINDOW);
    cpu_used().saturating_sub(before)
}

fn cpu_used() -> Duration {
    // SAFETY: `getrusage` writes a plain `rusage` and reads nothing else. The struct is
    // zeroed first so a failed call yields zero rather than uninitialized memory, and the
    // pointer is to a live local for the whole call.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) != 0 {
            return Duration::ZERO;
        }
        usage
    };
    let seconds = |time: libc::timeval| {
        Duration::from_secs(time.tv_sec.max(0).unsigned_abs())
            + Duration::from_micros(u64::from(time.tv_usec.max(0).unsigned_abs()))
    };
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

/// What this process is holding, as the OS sees it.
///
/// Every field optional, because the tool behind it may be missing and a zero would read as
/// a measurement rather than as a gap. A rig that quietly reports 0 open descriptors is
/// worse than one that says it could not look.
#[derive(Debug, Default)]
struct Stats {
    rss_kb: Option<u64>,
    threads: Option<u64>,
    fds: Option<u64>,
}

impl Stats {
    fn read() -> Stats {
        Stats { rss_kb: rss_kb(), threads: threads(), fds: fds() }
    }
}

fn rss_kb() -> Option<u64> {
    // ps reports RSS in kilobytes on both platforms, which is the one resource question
    // that does not need a per-OS answer.
    run("ps", &["-o", "rss=", "-p", &std::process::id().to_string()])?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn threads() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|count| count.trim().parse().ok())
}

#[cfg(not(target_os = "linux"))]
fn threads() -> Option<u64> {
    // `ps -M` prints one line per thread under a header line.
    let listing = run("ps", &["-M", "-p", &std::process::id().to_string()])?;
    without_header(&listing)
}

#[cfg(target_os = "linux")]
fn fds() -> Option<u64> {
    u64::try_from(std::fs::read_dir("/proc/self/fd").ok()?.count()).ok()
}

#[cfg(not(target_os = "linux"))]
fn fds() -> Option<u64> {
    let listing = run("lsof", &["-p", &std::process::id().to_string()])?;
    without_header(&listing)
}

/// How many rows a `ps`- or `lsof`-style listing has under its header.
///
/// `None` for a listing with no rows at all, which is the shape a tool that failed to look
/// takes - and reporting that as zero would read as a measurement.
#[cfg(not(target_os = "linux"))]
fn without_header(listing: &str) -> Option<u64> {
    let lines = listing.lines().filter(|line| !line.trim().is_empty()).count();
    u64::try_from(lines.checked_sub(1)?).ok().filter(|rows| *rows > 0)
}

/// The ceiling the descriptor count should be read against.
///
/// A number with no denominator decides nothing, and this one has two denominators. A
/// developer's shell usually has a raised soft limit, so reading only our own would report
/// a ceiling the shipped app never sees: a GUI-launched process inherits launchd's, which
/// on macOS defaults to 256. Both are printed, because the second is the one that binds.
fn fd_limit() -> Option<u64> {
    // SAFETY: `getrlimit` writes a plain `rlimit` through the pointer and reads nothing
    // else. Zeroed first, and the pointer is to a live local for the whole call.
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        (libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) == 0).then_some(limit.rlim_cur)
    }
}

/// The soft limit launchd hands a process it starts, which is what Muster actually runs under.
///
/// macOS only - elsewhere a GUI session is not launchd's, and this process's own limit is
/// already the honest answer.
#[cfg(target_os = "macos")]
fn launchd_fd_limit() -> Option<u64> {
    // `launchctl limit maxfiles` prints "\tmaxfiles    256    unlimited".
    let listing = run("launchctl", &["limit", "maxfiles"])?;
    listing.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(target_os = "macos"))]
fn launchd_fd_limit() -> Option<u64> {
    None
}

fn run(command: &str, arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command).args(arguments).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn report(one: &Stats, many: &Stats, panes: usize, idle: Duration) {
    let added = u64::try_from(panes.saturating_sub(1)).unwrap_or(0);
    println!("following a daemon: what {panes} panes cost this process\n");
    println!("{:<14} {:>12} {:>12} {:>12} {:>14}", "", "1 pane", "N panes", "delta", "per pane");

    row("rss", one.rss_kb, many.rss_kb, added, "KB");
    row("threads", one.threads, many.threads, added, "");
    row("fds", one.fds, many.fds, added, "");

    println!();
    match fd_limit() {
        Some(limit) => println!("descriptor limit, this process (RLIMIT_NOFILE soft): {limit}"),
        None => println!("descriptor limit, this process: could not read"),
    }
    match launchd_fd_limit() {
        Some(limit) => println!("descriptor limit, a GUI-launched app inherits:      {limit}"),
        None => println!("descriptor limit, a GUI-launched app inherits:      not applicable here"),
    }
    println!(
        "idle CPU over {}s following {panes} panes: {:.1} ms",
        IDLE_WINDOW.as_secs(),
        idle.as_secs_f64() * 1000.0
    );

    if panes < BUDGETED_PANES {
        println!(
            "\nNote: reached {panes} of the {BUDGETED_PANES} panes this window is budgeted \
             for - the daemon refused to split further. The per-pane column still holds; the \
             totals are for {panes}."
        );
    }
}

fn row(name: &str, one: Option<u64>, many: Option<u64>, added: u64, unit: &str) {
    let (Some(one), Some(many)) = (one, many) else {
        println!("{name:<14} {:>12}", "unavailable");
        return;
    };
    let delta = many.saturating_sub(one);
    let per_pane = if added == 0 { 0.0 } else { exact(delta) / exact(added) };
    println!("{name:<14} {one:>12} {many:>12} {delta:>12} {per_pane:>13.1}{unit}");
}

/// Lossless for every count this rig produces.
///
/// Saturating rather than rounding, because a value too large for 32 bits is not a
/// descriptor count that needs averaging - it is a bug, and a number that goes obviously
/// wrong is easier to notice than one that quietly loses its low bits.
fn exact(value: u64) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}
