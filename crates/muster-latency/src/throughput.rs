//! `muster-latency --throughput`: how long a pane with nothing attached takes to drain a flood,
//! beside a bare PTY read loop draining the same one.
//!
//! The flood is `yes | head -c <bytes>`, whose own time is bounded by the reader: once the PTY's
//! buffer is full it waits for room. Timed as wall and as the reader's CPU time, best of the runs,
//! because on a busy machine a run can land on slower cores and the best is the one that did not.

use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::daemon::Daemon;
use crate::surface;

const DRAINED: &str = "flood-drained";

#[derive(Clone, Copy)]
struct Drain {
    wall: Duration,
    cpu: Duration,
}

pub(crate) fn run(binary: &Path, bytes: u64, runs: usize) {
    let flood = format!("yes | head -c {bytes}");
    let (mut bare, mut daemon) = (Vec::new(), Vec::new());
    for run in 1..=runs {
        bare.push(bare_drain(&flood));
        daemon.push(daemon_drain(binary, &flood));
        let (b, d) = (bare[run - 1], daemon[run - 1]);
        eprintln!(
            "run {run}: bare {:.2?} wall {:.2?} cpu, daemon {:.2?} wall {:.2?} cpu",
            b.wall, b.cpu, d.wall, d.cpu
        );
    }
    let best = |drains: &[Drain], pick: fn(&Drain) -> Duration| {
        drains.iter().map(pick).min().unwrap_or_default().as_secs_f64()
    };
    let rows = [("bare PTY read loop", &bare), ("muster-daemon, no bridge", &daemon)];
    println!("{bytes} bytes of `yes`, best of {runs}:");
    for (name, drains) in rows {
        println!(
            "  {name:<26} {:>6.2} s wall  {:>6.2} s cpu",
            best(drains, |d| d.wall),
            best(drains, |d| d.cpu)
        );
    }
    println!(
        "  daemon / bare              {:>6.2}x wall {:>6.2}x cpu",
        best(&daemon, |d| d.wall) / best(&bare, |d| d.wall),
        best(&daemon, |d| d.cpu) / best(&bare, |d| d.cpu)
    );
}

fn bare_drain(flood: &str) -> Drain {
    let started = (Instant::now(), own_cpu());
    let (mut master, _child) = surface::on_pty(Command::new("sh").args(["-c", flood]));
    let mut buffer = vec![0u8; 64 * 1024];
    // EIO once every process holding the replica has closed it.
    while master.read(&mut buffer).is_ok_and(|read| read > 0) {}
    Drain { wall: started.0.elapsed(), cpu: own_cpu().saturating_sub(started.1) }
}

fn daemon_drain(binary: &Path, flood: &str) -> Drain {
    let mut daemon = Daemon::spawn(binary);
    let pid = daemon.pid().expect("this run started the daemon");
    let started = (Instant::now(), cpu_of(pid));
    let pane = daemon.pane(&format!("{flood}; echo {DRAINED}; sleep 600"));
    // Read rarely, so the reads cost the reader next to nothing.
    while !daemon.screen(&pane).iter().any(|row| row.trim() == DRAINED) {
        std::thread::sleep(Duration::from_millis(50));
    }
    Drain { wall: started.0.elapsed(), cpu: cpu_of(pid).saturating_sub(started.1) }
}

fn own_cpu() -> Duration {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the struct it is given.
    let usage = unsafe {
        libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr());
        usage.assume_init()
    };
    let time = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec.cast_unsigned())
            + Duration::from_micros(t.tv_usec.cast_unsigned().into())
    };
    time(usage.ru_utime) + time(usage.ru_stime)
}

/// Another process's CPU time, as `ps` reports it: `[[dd-]hh:]mm:ss[.ss]`.
fn cpu_of(pid: u32) -> Duration {
    let output = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).output();
    let text = output.map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let text = text.unwrap_or_default();
    let seconds = text
        .rsplit(['-', ':'])
        .zip([1.0, 60.0, 3600.0, 86400.0])
        .map(|(part, unit)| part.parse::<f64>().unwrap_or(0.0) * unit)
        .sum::<f64>();
    Duration::from_secs_f64(seconds)
}
