//! What the daemon's tests share: the daemon, and requests spelled so a test reads as what it
//! asks for.

#![allow(dead_code, unreachable_pub, unused_imports)]

use std::path::Path;

pub use muster_daemon_proto as proto;
pub use muster_harness::{Asked, Control, Daemon, until, until_file, until_some};
use proto::request::Service;
use proto::{pane_request, placement, session_request, tab_request};

/// A daemon built from this commit, on a scratch root of its own.
pub fn daemon() -> Daemon {
    Daemon::start(env!("CARGO_BIN_EXE_muster-daemon"))
}

pub fn daemon_with(environment: &[(&str, &str)]) -> Daemon {
    Daemon::start_with(env!("CARGO_BIN_EXE_muster-daemon"), environment)
}

pub fn daemon_holding(descriptor: i32) -> Daemon {
    Daemon::start_holding(env!("CARGO_BIN_EXE_muster-daemon"), descriptor)
}

pub fn session(request: session_request::Request) -> Service {
    Service::Session(proto::SessionRequest { request: Some(request) })
}

pub fn tab(request: tab_request::Request) -> Service {
    Service::Tab(proto::TabRequest { request: Some(request) })
}

pub fn pane(request: pane_request::Request) -> Service {
    Service::Pane(proto::PaneRequest { request: Some(request) })
}

pub fn snapshot_request() -> Service {
    session(session_request::Request::Snapshot(session_request::Snapshot {}))
}

pub fn subscribe_request() -> Service {
    session(session_request::Request::Subscribe(session_request::Subscribe {}))
}

pub fn in_new_tab(tab: &str) -> proto::Placement {
    proto::Placement {
        r#where: Some(placement::Where::NewTab(placement::NewTab {
            tab: tab.to_string(),
            label: None,
        })),
    }
}

pub fn beside(pane: &str, side: proto::Side) -> proto::Placement {
    proto::Placement {
        r#where: Some(placement::Where::Beside(placement::Beside {
            pane: pane.to_string(),
            side: side.into(),
            ratio: None,
        })),
    }
}

/// A create with nothing but a name and a place.
pub fn create(name: &str, placement: proto::Placement) -> pane_request::Create {
    pane_request::Create {
        pane: name.to_string(),
        placement: Some(placement),
        ..pane_request::Create::default()
    }
}

pub fn create_request(create: pane_request::Create) -> Service {
    pane(pane_request::Request::Create(create))
}

pub fn close_request(name: &str) -> Service {
    pane(pane_request::Request::Close(pane_request::Close { pane: name.to_string() }))
}

/// Asks and insists on an outcome, naming the reason when it differs.
#[track_caller]
pub fn expect(control: &mut Control, service: Service, outcome: proto::Outcome) -> Asked {
    let asked = control.ask(service);
    assert_eq!(
        asked.outcome(),
        outcome,
        "the daemon answered {:?}: {}",
        asked.outcome(),
        asked.answer.reason
    );
    asked
}

/// Makes a pane and insists it was made.
#[track_caller]
pub fn make(control: &mut Control, create: pane_request::Create) -> Asked {
    expect(control, create_request(create), proto::Outcome::Done)
}

pub fn snapshot(control: &mut Control) -> proto::Snapshot {
    match expect(control, snapshot_request(), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Snapshot(snapshot)) => snapshot,
        other => panic!("a snapshot request answered with {other:?}"),
    }
}

/// A tab's tree as text: `[a|b .50]` for columns, `[a/b .50]` for rows.
pub fn shape(node: &proto::Node) -> String {
    match node.node.as_ref().expect("a node holds something") {
        proto::node::Node::Pane(name) => name.clone(),
        proto::node::Node::Split(split) => {
            let divider = if split.axis() == proto::Axis::Columns { "|" } else { "/" };
            format!(
                "[{}{divider}{} {:.2}]",
                shape(split.first.as_ref().expect("a split has a first child")),
                shape(split.second.as_ref().expect("a split has a second child")),
                split.ratio
            )
        }
    }
}

/// The shape of the tab named `tab`, from a fresh snapshot.
pub fn tab_shape(control: &mut Control, tab: &str) -> String {
    let snapshot = snapshot(control);
    let found = snapshot.tabs.iter().find(|candidate| candidate.tab == tab);
    shape(found.unwrap_or_else(|| panic!("no tab {tab} in {snapshot:?}")).root.as_ref().unwrap())
}

/// An event as `kind:name`, which is what most ordering assertions care about.
pub fn named(event: &proto::Event) -> String {
    use proto::event::Event as E;
    match event.event.as_ref().expect("an event holds something") {
        E::PaneOpened(opened) => format!("pane_opened:{}", opened.pane.as_ref().unwrap().pane),
        E::PaneChanged(changed) => format!("pane_changed:{}", changed.pane.as_ref().unwrap().pane),
        E::PaneClosed(closed) => format!("pane_closed:{}", closed.pane),
        E::TabOpened(opened) => format!("tab_opened:{}", opened.tab.as_ref().unwrap().tab),
        E::TabChanged(changed) => format!("tab_changed:{}", changed.tab.as_ref().unwrap().tab),
        E::TabClosed(closed) => format!("tab_closed:{}", closed.tab),
        E::SettingsChanged(_) => "settings_changed".to_string(),
        E::PaneEffect(effect) => format!("pane_effect:{}", effect.pane),
        E::PasteHeld(held) => format!("paste_held:{}", held.pane),
    }
}

pub fn names(events: &[proto::Event]) -> Vec<String> {
    events.iter().map(named).collect()
}

/// What a file a pane was asked to write says, once it says something.
pub fn written(path: &Path) -> String {
    until_file(path, &format!("a pane to write {}", path.display()));
    // A redirect creates the file before the command has finished writing it, so wait for the
    // line to be complete.
    until_some(&format!("{} to end in a newline", path.display()), || {
        std::fs::read_to_string(path).ok().filter(|text| text.ends_with('\n'))
    })
}

/// A path as the kernel names it, so `/tmp` and `/private/tmp` compare equal on macOS.
pub fn canonical(path: &Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// What `ps` says a process is, or nothing once it has gone and been reaped.
pub fn process_state(pid: &str) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=,args=", "-p", pid.trim()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn read_request(name: &str, first_row: u64, rows: u32) -> Service {
    pane(pane_request::Request::Read(pane_request::Read {
        pane: name.to_string(),
        first_row,
        rows,
    }))
}

/// A page of a pane's text, insisting it was read.
#[track_caller]
pub fn read_text(control: &mut Control, name: &str, first_row: u64, rows: u32) -> proto::PaneText {
    match expect(control, read_request(name, first_row, rows), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Text(text)) => text,
        other => panic!("a read answered with {other:?}"),
    }
}

/// Every row of a pane's text, once it contains `needle`.
pub fn until_text(control: &mut Control, name: &str, needle: &str) -> String {
    until_some(&format!("pane {name} to show {needle:?}"), || {
        Some(read_text(control, name, 0, 0).text).filter(|text| text.contains(needle))
    })
}

/// A command that puts its terminal in raw mode, writes `query` (a printf format), and saves
/// whatever comes back on its input to `out` - which is how a test sees the daemon answer a
/// program's query as the program would.
pub fn answer_to(query: &str, out: &Path) -> String {
    format!(
        "stty raw -echo min 0 time 50; printf '{query}'; dd bs=4096 count=1 of={} 2>/dev/null",
        out.display()
    )
}

/// What a file holds once something has been written to it.
pub fn bytes_in(path: &Path) -> Vec<u8> {
    until_some(&format!("something written to {}", path.display()), || {
        std::fs::read(path).ok().filter(|bytes| !bytes.is_empty())
    })
}

/// Events from a subscribed connection until `enough` says the ones gathered suffice.
pub fn events_until(
    control: &mut Control,
    what: &str,
    mut enough: impl FnMut(&[proto::Event]) -> bool,
) -> Vec<proto::Event> {
    let deadline = std::time::Instant::now() + muster_harness::PATIENCE;
    let mut events = Vec::new();
    while !enough(&events) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!left.is_zero(), "{what}: not within the suite's patience; heard {events:?}");
        if let Some(proto::control_message::Message::Event(event)) = control.next_message(left) {
            events.push(event);
        }
    }
    events
}

pub use muster_harness::Stream;
use proto::stream_message::Message as Streamed;

/// What a bridge's surface would show: a terminal fed every replay and every byte of output
/// its stream carries, and what else the stream said.
pub struct Surface {
    pub terminal: muster_vt::Terminal,
    /// Output fed and not yet acknowledged.
    pub unacknowledged: u64,
    /// Output fed since attaching.
    pub output: u64,
    pub attached: Option<u64>,
    pub replays: usize,
    pub behind: usize,
    pub detached: Option<proto::DetachReason>,
    pub refused: Option<String>,
    /// The daemon hung up.
    pub ended: bool,
}

impl Surface {
    pub fn new(cols: u16, rows: u16) -> Surface {
        let mut options = muster_vt::TerminalOptions::new(cols, rows);
        options.scrollback_bytes = Some(64 << 20);
        Surface {
            terminal: muster_vt::Terminal::with_options(options).expect("a terminal"),
            unacknowledged: 0,
            output: 0,
            attached: None,
            replays: 0,
            behind: 0,
            detached: None,
            refused: None,
            ended: false,
        }
    }

    pub fn take(&mut self, message: Option<Streamed>) {
        match message {
            Some(Streamed::Attached(attached)) => self.attached = Some(attached.offset),
            Some(Streamed::Replay(bytes)) => {
                self.replays += 1;
                self.terminal.write(&bytes);
            }
            Some(Streamed::Output(bytes)) => {
                self.unacknowledged += bytes.len() as u64;
                self.output += bytes.len() as u64;
                self.terminal.write(&bytes);
            }
            Some(Streamed::Behind(_)) => self.behind += 1,
            Some(Streamed::Detached(detached)) => self.detached = Some(detached.reason()),
            Some(Streamed::Refused(refused)) => self.refused = Some(refused.reason),
            None => self.ended = true,
        }
    }

    /// Everything the surface holds, history first, as text.
    pub fn text(&self) -> String {
        self.terminal.screen_text(0, u32::MAX)
    }

    /// What the surface shows, without its history: cheap enough to check after every message.
    pub fn screen(&self) -> String {
        self.terminal.text(0, self.terminal.rows().saturating_sub(1))
    }

    /// Takes messages until `done` says so, acknowledging output as it goes when `credit`.
    pub fn follow(
        &mut self,
        stream: &mut Stream,
        what: &str,
        credit: bool,
        mut done: impl FnMut(&Surface) -> bool,
    ) {
        let deadline = std::time::Instant::now() + muster_harness::PATIENCE;
        while !done(self) {
            if credit && self.unacknowledged > 0 {
                stream.credit(self.unacknowledged);
                self.unacknowledged = 0;
            }
            assert!(
                !self.ended,
                "{what}: the daemon hung up; the surface shows {:?}",
                self.screen()
            );
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !left.is_zero(),
                "{what}: not within the suite's patience; {:?}",
                self.screen()
            );
            if let Some(message) = stream.next_within(left) {
                self.take(message);
            }
        }
    }
}

/// A stream attached to `name`.
pub fn attached(daemon: &Daemon, name: &str, takeover: bool) -> Stream {
    let mut stream = Stream::connect(daemon.socket_path());
    stream.attach(name, None, takeover);
    stream
}
