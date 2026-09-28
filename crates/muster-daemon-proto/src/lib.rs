//! Muster's protocol for muster-daemon, shared by the daemon and every client of it.
//!
//! The daemon and the app are separately built programs that routinely differ in version, since
//! an app adopts whichever daemon is running (MIP-3, section 9). So this crate holds what the
//! two must agree on and nothing else: the generated messages (`proto/muster_daemon.proto`,
//! whose header is the protocol's documentation), the version rule, where an install's daemon
//! listens, how long a daemon just started is given, and the handshake that opens every
//! connection.

pub mod connection;
pub mod install;
pub mod launch;
pub mod version;

include!(concat!(env!("OUT_DIR"), "/muster.daemon.rs"));

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
            None => "session",
        },
        Service::Tab(asked) => match &asked.request {
            Some(tab_request::Request::Close(_)) => "tab.close",
            Some(tab_request::Request::Rename(_)) => "tab.rename",
            Some(tab_request::Request::SetSplitRatio(_)) => "tab.set_split_ratio",
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
            None => "pane",
        },
    }
}
