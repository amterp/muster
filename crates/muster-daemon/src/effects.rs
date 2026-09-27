//! What a pane's output asks of the world outside it, and how that becomes events.
//!
//! libghostty reports effects in the middle of a write, on the thread parsing the pane's output
//! and under the pane's lock. That thread must never wait for the session lock, which requests
//! hold, so it hands what it heard to the publisher: one thread that takes the session lock and
//! turns each report into an event. A report the publisher has no room for is dropped with a
//! warning rather than stalling the pane it came from.

use std::path::PathBuf;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_vt::{ClipboardLocation, Effect, Progress};
use proto::pane_effect::Effect as Shown;

use crate::session::Shared;

/// How many reports may wait for the publisher. Titles and bells arrive at the rate programs
/// change them; filling this means the session lock is held far longer than any request should
/// hold it.
const REPORTS_DEPTH: usize = 4096;

/// An effect, owned, so it can leave the write that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Happened {
    /// Bytes answering a query, for the pane's input.
    Reply(Vec<u8>),
    Title(String),
    /// OSC 7's URL, as the program reported it.
    Pwd(String),
    Shown(Shown),
    /// What the program now says with XTSHIFTESCAPE.
    ShiftCapture(Option<bool>),
}

impl Happened {
    pub(crate) fn from_effect(effect: &Effect<'_>) -> Happened {
        match effect {
            Effect::Reply(bytes) => Happened::Reply(bytes.to_vec()),
            Effect::Title(title) => Happened::Title((*title).to_string()),
            Effect::Pwd(url) => Happened::Pwd((*url).to_string()),
            Effect::Bell => Happened::Shown(Shown::Bell(proto::pane_effect::Bell {})),
            Effect::Notification { title, body } => {
                Happened::Shown(Shown::Notification(proto::pane_effect::Notification {
                    title: (*title).to_string(),
                    body: (*body).to_string(),
                }))
            }
            Effect::Progress(progress) => Happened::Shown(Shown::Progress(progress_of(*progress))),
            Effect::ClipboardWrite { location, contents } => {
                // A program may offer several representations; the app writes text.
                let text = contents
                    .iter()
                    .find(|content| content.mime.starts_with("text/plain"))
                    .or_else(|| contents.first())
                    .map(|content| String::from_utf8_lossy(content.data).into_owned())
                    .unwrap_or_default();
                let clipboard = match location {
                    ClipboardLocation::Standard => proto::ClipboardKind::Standard,
                    ClipboardLocation::Selection => proto::ClipboardKind::Selection,
                    ClipboardLocation::Primary => proto::ClipboardKind::Primary,
                };
                Happened::Shown(Shown::ClipboardWrite(proto::pane_effect::ClipboardWrite {
                    clipboard: clipboard.into(),
                    text,
                }))
            }
        }
    }
}

fn progress_of(progress: Progress) -> proto::pane_effect::Progress {
    let (state, percent) = match progress {
        Progress::Remove => (proto::ProgressState::Remove, None),
        Progress::Set(percent) => (proto::ProgressState::Set, Some(percent)),
        Progress::Error(percent) => (proto::ProgressState::Error, percent),
        Progress::Indeterminate => (proto::ProgressState::Indeterminate, None),
        Progress::Pause(percent) => (proto::ProgressState::Pause, percent),
    };
    proto::pane_effect::Progress { state: state.into(), percent: percent.map(u32::from) }
}

/// The directory an OSC 7 URL names, when it names one on this machine.
///
/// A program reports `file://host/path`. A host other than this one is a shell on another
/// machine - ssh inside the pane - whose path means nothing here, so it is ignored, as Ghostty
/// ignores it.
pub(crate) fn local_directory(url: &str, this_host: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let (host, path) = rest.split_at(slash);
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case(this_host)
        || this_host.split_once('.').is_some_and(|(short, _)| host.eq_ignore_ascii_case(short));
    if !local {
        return None;
    }
    let decoded = percent_decode(path)?;
    let path =
        PathBuf::from(<std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(decoded));
    path.is_absolute().then_some(path)
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(decoded)
}

/// This machine's name, as OSC 7 URLs from a shell here spell their host.
pub(crate) fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname NUL-terminates what fits.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// What a pane tells the session, by the serial that tells its process apart from a later
/// pane given the same name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Report {
    pub(crate) serial: u64,
    pub(crate) what: Reported,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reported {
    Title(String),
    Cwd(PathBuf),
    Shown(Shown),
    /// A paste with a newline, to a program that did not ask for bracketed paste.
    PasteHeld(String),
    /// What agent detection now says the pane is running, and its state: whether that is the
    /// agent's own report, and whether the screen rules can no longer read it.
    Agent {
        agent: Option<String>,
        state: proto::AgentState,
        reported: bool,
        unreadable: bool,
    },
    /// What the program says with XTSHIFTESCAPE, for the pane's record.
    ShiftCapture(Option<bool>),
    /// Answered once every report queued before it has been applied ([`Reports::settle`]).
    Settled(Settle),
}

/// The answer a [`Reported::Settled`] is waiting for.
#[derive(Debug, Clone)]
pub(crate) struct Settle(SyncSender<()>);

/// Never equal: each settle waits on its own answer.
impl PartialEq for Settle {
    fn eq(&self, _: &Settle) -> bool {
        false
    }
}

/// Where panes send their reports.
#[derive(Debug, Clone)]
pub(crate) struct Reports {
    sender: SyncSender<Report>,
}

static OVERRUN_WARNED: AtomicBool = AtomicBool::new(false);

impl Reports {
    pub(crate) fn channel() -> (Reports, Receiver<Report>) {
        Reports::with_depth(REPORTS_DEPTH)
    }

    pub(crate) fn with_depth(depth: usize) -> (Reports, Receiver<Report>) {
        let (sender, receiver) = mpsc::sync_channel(depth);
        (Reports { sender }, receiver)
    }

    /// Queues a report for the session. False when the queue was full and the report dropped,
    /// so a caller holding state the session must end up with can send it again. A daemon that
    /// is stopping counts as told.
    pub(crate) fn send(&self, serial: u64, what: Reported) -> bool {
        match self.sender.try_send(Report { serial, what }) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => true,
            Err(TrySendError::Full(_)) => {
                if !OVERRUN_WARNED.swap(true, Ordering::Relaxed) {
                    log::warn(
                        "daemon.reports.overrun",
                        fields! {
                            "queued" => REPORTS_DEPTH,
                            "impact" => "a pane's title, bell or notification was dropped; \
                                         a directory or agent state is sent again shortly",
                            "check" => "whether a request is holding the session lock for \
                                        seconds, which is a bug",
                        },
                    );
                }
                false
            }
        }
    }

    /// Waits, at most `within`, until the session has applied every report queued before this
    /// call, waiting for room in the queue rather than dropping. The caller may hold no lock the
    /// publisher takes, the session's above all, or it waits out the deadline for nothing. True
    /// when a publisher that has gone applies nothing more either.
    pub(crate) fn settle(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let (settled, answer) = mpsc::sync_channel(1);
        let mut marker = Report { serial: 0, what: Reported::Settled(Settle(settled)) };
        loop {
            match self.sender.try_send(marker) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return true,
                Err(TrySendError::Full(unsent)) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    marker = unsent;
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        answer.recv_timeout(deadline.saturating_duration_since(Instant::now())).is_ok()
    }
}

/// Applies every report under the session lock, for as long as the daemon runs.
pub(crate) fn publish(receiver: &Receiver<Report>, shared: &Weak<Shared>) {
    publish_with(receiver, |report| {
        let Some(shared) = shared.upgrade() else { return false };
        shared.lock().reported(report);
        OVERRUN_WARNED.store(false, Ordering::Relaxed);
        true
    });
}

/// Hands each report to `apply` in order, answering a settle once everything before it is
/// applied, until `apply` says there is no session left to apply them to.
fn publish_with(receiver: &Receiver<Report>, mut apply: impl FnMut(Report) -> bool) {
    for report in receiver {
        if let Reported::Settled(Settle(settled)) = &report.what {
            let _ = settled.try_send(());
        } else if !apply(report) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settle_returns_once_every_report_before_it_is_applied() {
        let (reports, received) = Reports::with_depth(8);
        let applied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let applying = std::sync::Arc::clone(&applied);
        let publisher = std::thread::spawn(move || {
            publish_with(&received, |report| {
                std::thread::sleep(Duration::from_millis(20));
                applying.lock().unwrap().push(report.what);
                true
            });
        });
        for title in ["one", "two", "three"] {
            assert!(reports.send(1, Reported::Title(title.to_string())));
        }
        assert!(reports.settle(Duration::from_secs(5)));
        assert_eq!(applied.lock().unwrap().len(), 3);
        drop(reports);
        publisher.join().unwrap();
    }

    #[test]
    fn a_settle_gives_up_at_its_deadline_when_the_queue_stays_full() {
        let (reports, _received) = Reports::with_depth(1);
        assert!(reports.send(1, Reported::Title("fills the queue".to_string())));
        let (done, answer) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(reports.settle(Duration::from_millis(100)));
        });
        let settled = answer.recv_timeout(Duration::from_secs(2));
        assert_eq!(settled, Ok(false), "a full queue must not hold the caller past its deadline");
    }

    #[test]
    fn a_local_url_names_its_directory() {
        let dir = |url| local_directory(url, "studio.local");
        assert_eq!(dir("file:///Users/me"), Some(PathBuf::from("/Users/me")));
        assert_eq!(dir("file://localhost/tmp"), Some(PathBuf::from("/tmp")));
        assert_eq!(dir("file://studio.local/tmp"), Some(PathBuf::from("/tmp")));
        assert_eq!(dir("file://STUDIO/tmp"), Some(PathBuf::from("/tmp")), "the short name");
        assert_eq!(dir("file:///a%20b/%C3%A9"), Some(PathBuf::from("/a b/é")));
    }

    #[test]
    fn another_machine_or_a_malformed_url_names_nothing() {
        let dir = |url| local_directory(url, "studio.local");
        assert_eq!(dir("file://devbox/home/me"), None, "ssh inside the pane");
        assert_eq!(dir("http://localhost/tmp"), None);
        assert_eq!(dir("file://localhost"), None);
        assert_eq!(dir("file:///bad%2"), None);
        assert_eq!(dir("file:///bad%zz"), None);
    }

    #[test]
    fn a_clipboard_write_offers_its_text() {
        let contents = vec![
            muster_vt::ClipboardContent { mime: "image/png", data: b"\x89PNG" },
            muster_vt::ClipboardContent { mime: "text/plain;charset=utf-8", data: b"copied" },
        ];
        let effect = Effect::ClipboardWrite { location: ClipboardLocation::Standard, contents };
        let Happened::Shown(Shown::ClipboardWrite(write)) = Happened::from_effect(&effect) else {
            panic!("a clipboard write stays one");
        };
        assert_eq!(write.text, "copied");
        assert_eq!(write.clipboard(), proto::ClipboardKind::Standard);
    }
}
