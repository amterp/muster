//! Two clocks, because the log answers two different questions.

/// Nanoseconds on a clock that only moves forward, shared by every process on the machine.
///
/// The log spans the app and one bridge per pane, and the questions worth timing cross
/// that boundary: a keystroke leaves the app, a frame carrying its echo arrives at a
/// bridge. Wall-clock timestamps cannot answer those. They resolve milliseconds, the hops
/// are tenths of one, and they are free to jump backwards when the system adjusts time.
///
/// `CLOCK_MONOTONIC` is counted from boot rather than per process, so two records written
/// by two processes subtract directly - which is the whole reason this is worth carrying
/// on every line. POSIX, so it holds wherever the core is asked to run.
///
/// Not `std::time::Instant`, which is opaque: two processes cannot subtract two of them.
pub fn monotonic_now() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes into a timespec we own and touches nothing else.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut t) } != 0 {
        return 0;
    }
    // Both are non-negative by definition: this clock counts from boot.
    t.tv_sec.cast_unsigned().wrapping_mul(1_000_000_000).wrapping_add(t.tv_nsec.cast_unsigned())
}

/// Nanoseconds from `start` until now, or 0 if the reading failed.
pub fn monotonic_since(start: u64) -> u64 {
    monotonic_now().saturating_sub(start)
}

/// Milliseconds since the epoch, for the human-readable half of a record.
pub fn wall_clock_millis() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        // Before 1970, which means the machine's clock is badly wrong. The record still
        // gets written: a log that refuses to say when is worse than one saying something
        // obviously absurd.
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// ISO 8601 in UTC, to the millisecond.
///
/// Hand-rolled rather than taken from a date library, because this string has to be
/// byte-identical to what Swift's `Date.ISO8601FormatStyle(includingFractionalSeconds:)`
/// produces: during the port both languages append to one log file, and a timeline whose
/// timestamps render two ways is not a timeline.
pub fn format_iso8601(millis: i64) -> String {
    let (days, millis_of_day) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let (year, month, day) = civil_from_days(days);
    let (second_of_day, milli) = (millis_of_day / 1000, millis_of_day % 1000);
    let (hour, minute, second) =
        (second_of_day / 3600, (second_of_day % 3600) / 60, second_of_day % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// Hinnant's days-to-civil, which is exact for any day this program can be handed and
/// needs no table. The epoch is shifted to March so that a leap day lands at the end of a
/// year and the month arithmetic stays branch-free.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
