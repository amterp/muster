//! Muster's protocol for muster-daemon, shared by the daemon and every client of it.
//!
//! The daemon and the app are separately built programs that routinely differ in version, since
//! an app adopts whichever daemon is running (MIP-3, section 9). So this crate holds what the
//! two must agree on and nothing else: the generated messages (`proto/muster_daemon.proto`,
//! whose header is the protocol's documentation), the version rule, where an install's daemon
//! listens, how long a daemon just started is given, the handshake that opens every connection,
//! how the messaging commands are spelled, how a pane is asked for when no window places it, how
//! a pane's newest rows are read through pages, what state a pane's record shows its agent in,
//! how compaction thresholds are written in a message,
//! and how many closed panes a daemon remembers.

pub mod compaction;
pub mod connection;
pub mod install;
pub mod launch;
pub mod making;
pub mod messaging;
mod pane_state;
pub mod pane_text;
pub mod version;

pub use pane_state::PaneState;

/// The most closed panes a daemon remembers for a reopen. A close past it forgets the oldest and
/// sends no event for it, so a client keeping the list keeps no more than this either.
pub const CLOSED_PANES_KEPT: usize = 64;

include!(concat!(env!("OUT_DIR"), "/muster.daemon.rs"));

impl Pane {
    /// What the pane is called: the name somebody gave it, or failing that its callsign. Here so that every reader of a record derives it one way, since a name is an
    /// address and two readers disagreeing would send a request to the wrong pane. None only
    /// from a daemon before 1.5 for a pane nobody named.
    pub fn name(&self) -> Option<&str> {
        self.label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .or_else(|| self.callsign.as_deref().filter(|word| !word.is_empty()))
    }
}

/// What a request asks, as `pane.read` or `session.subscribe`: the name a log gives it on both
/// sides of the socket, so a line in the app and a line in the daemon about one request read
/// alike. Nothing of what the request carries, which can be what somebody typed.
pub fn service_name(service: &request::Service) -> &'static str {
    use request::Service;
    match service {
        Service::Session(asked) => match &asked.request {
            Some(session_request::Request::Snapshot(_)) => "session.snapshot",
            Some(session_request::Request::Subscribe(_)) => "session.subscribe",
            Some(session_request::Request::SetShell(_)) => "session.set_shell",
            Some(session_request::Request::SetScrollback(_)) => "session.set_scrollback",
            Some(session_request::Request::SetPalette(_)) => "session.set_palette",
            Some(session_request::Request::SendManifests(_)) => "session.send_manifests",
            Some(session_request::Request::Stop(_)) => "session.stop",
            Some(session_request::Request::SetClipboardWrite(_)) => "session.set_clipboard_write",
            Some(session_request::Request::SetCursor(_)) => "session.set_cursor",
            Some(session_request::Request::FollowLog(_)) => "session.follow_log",
            Some(session_request::Request::Replace(_)) => "session.replace",
            Some(session_request::Request::SetScrollMultiplier(_)) => {
                "session.set_scroll_multiplier"
            }
            Some(session_request::Request::ReadGrids(_)) => "session.read_grids",
            Some(session_request::Request::SetNameSessions(_)) => "session.set_name_sessions",
            Some(session_request::Request::SetHumanName(_)) => "session.set_human_name",
            Some(session_request::Request::SetCompactAt(_)) => "session.set_compact_at",
            Some(session_request::Request::SetResumeAgents(_)) => "session.set_resume_agents",
            Some(session_request::Request::ReportPresence(_)) => "session.report_presence",
            Some(session_request::Request::Shown(_)) => "session.shown",
            Some(session_request::Request::SetReopenClosed(_)) => "session.set_reopen_closed",
            Some(session_request::Request::SetCompactionRules(_)) => "session.set_compaction_rules",
            Some(session_request::Request::SetCallsignsElsewhere(_)) => {
                "session.set_callsigns_elsewhere"
            }
            None => "session",
        },
        Service::Tab(asked) => match &asked.request {
            Some(tab_request::Request::Close(_)) => "tab.close",
            Some(tab_request::Request::Rename(_)) => "tab.rename",
            Some(tab_request::Request::SetSplitRatio(_)) => "tab.set_split_ratio",
            Some(tab_request::Request::SetHidden(_)) => "tab.set_hidden",
            Some(tab_request::Request::SetTree(_)) => "tab.set_tree",
            None => "tab",
        },
        Service::Pane(asked) => match &asked.request {
            Some(pane_request::Request::Create(_)) => "pane.create",
            Some(pane_request::Request::Close(_)) => "pane.close",
            Some(pane_request::Request::Resize(_)) => "pane.resize",
            Some(pane_request::Request::Zoom(_)) => "pane.zoom",
            Some(pane_request::Request::Swap(_)) => "pane.swap",
            Some(pane_request::Request::Move(_)) => "pane.move",
            Some(pane_request::Request::Rename(_)) => "pane.rename",
            Some(pane_request::Request::Read(_)) => "pane.read",
            Some(pane_request::Request::Report(_)) => "pane.report",
            Some(pane_request::Request::Seen(_)) => "pane.seen",
            Some(pane_request::Request::Compact(_)) => "pane.compact",
            Some(pane_request::Request::Quiet(_)) => "pane.quiet",
            Some(pane_request::Request::SetCompaction(_)) => "pane.set_compaction",
            Some(pane_request::Request::MakerClosed(_)) => "pane.maker_closed",
            Some(pane_request::Request::Restart(_)) => "pane.restart",
            Some(pane_request::Request::StoreImage(_)) => "pane.store_image",
            Some(pane_request::Request::Reopen(_)) => "pane.reopen",
            Some(pane_request::Request::Send(_)) => "pane.send",
            Some(pane_request::Request::Clear(_)) => "pane.clear",
            Some(pane_request::Request::Unseen(_)) => "pane.unseen",
            None => "pane",
        },
        Service::Msg(asked) => match &asked.request {
            Some(msg_request::Request::Join(_)) => "msg.join",
            Some(msg_request::Request::Leave(_)) => "msg.leave",
            Some(msg_request::Request::Who(_)) => "msg.who",
            Some(msg_request::Request::Post(_)) => "msg.post",
            Some(msg_request::Request::Read(_)) => "msg.read",
            Some(msg_request::Request::Log(_)) => "msg.log",
            Some(msg_request::Request::Wait(_)) => "msg.wait",
            Some(msg_request::Request::Groups(_)) => "msg.groups",
            Some(msg_request::Request::GroupNew(_)) => "msg.group_new",
            Some(msg_request::Request::GroupSet(_)) => "msg.group_set",
            Some(msg_request::Request::GroupMembers(_)) => "msg.group_members",
            Some(msg_request::Request::Pause(_)) => "msg.pause",
            Some(msg_request::Request::Resume(_)) => "msg.resume",
            Some(msg_request::Request::Peer(_)) => "msg.peer",
            Some(msg_request::Request::GroupDelete(_)) => "msg.group_delete",
            Some(msg_request::Request::Ack(_)) => "msg.ack",
            None => "msg",
        },
    }
}
