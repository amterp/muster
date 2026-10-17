//! Muster's vocabulary, on its own, so that both ends of it can be built separately.
//!
//! The schema was generated inside `muster-seam` until the CLI existed, which was fine while
//! the only caller was in the same process. It is not fine now: the seam links `muster-vt`,
//! which links libghostty-vt as a dylib, so a client that reached the schema through the seam
//! would be a binary that will not start on a machine without that library - and the CLI is
//! the one part of Muster somebody copies onto a server.
//!
//! So this crate holds the generated types and nothing else that could drag a dependency in.
//! It is what the app and the CLI agree on, which is exactly the set of things two separately
//! built programs must not disagree about: the messages, and the framing that carries them.
//!
//! Deliberately not here: the names of the variables a pane is handed. Those are set by
//! `muster-daemon-client`, which does not link this crate, and
//! `crates/muster-cli/src/environment.rs` carries the CLI's own spelling and a test that the
//! two still agree.

pub mod frame;

include!(concat!(env!("OUT_DIR"), "/muster.rs"));

/// Whether a request only asks a question.
///
/// Here rather than beside either reader, because two things decide on it and they are separate
/// programs. The window uses it to leave an armed numbered chord alone: a question does not spend
/// what the first press armed, and anything that changes something does. The CLI uses it to decide
/// whether a command may be asked of every window at once: naming no window is a real problem for
/// a write and no problem at all for a read. Two copies of the list would be two chances for
/// "this only reads" to mean two things.
///
/// Deliberately a short allowlist with everything else falling through, so a request added later
/// defaults to being a change - which is the safe direction on both sides: an over-eager disarm
/// costs a chord and a stuck one is a window whose numbers lie, and a write sent to every window
/// is a pane made in one nobody meant.
///
/// One entry is here for a reason neither reader would guess: the shell logs through the core,
/// often, so a run log that counted as a change would disarm a numbered chord in every build
/// anybody was watching.
pub fn only_reads(payload: &request::Payload) -> bool {
    matches!(
        payload,
        request::Payload::LogRecord(_)
            | request::Payload::ReadBindings(_)
            | request::Payload::ReadWindow(_)
            | request::Payload::ReadAppearance(_)
            | request::Payload::ReadWindowFrame(_)
            | request::Payload::ReportFontFamily(_)
            | request::Payload::ReadPane(_)
            | request::Payload::ReadDaemons(_)
            | request::Payload::WatchPanes(_)
            | request::Payload::ReadAsking(_)
    )
}

/// What a request names as the thing it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Names<'a> {
    Tab(&'a str),
    Pane(&'a str),
}

/// The tab or pane a request that changes something names, when it names one.
///
/// Here for the reason [`only_reads`] is: two programs decide on it. A window carries a request
/// about another window's tab to that window, because a tab belongs to exactly one window (kan
/// a_2Mhi0EZlv) - and so the CLI can send a request that names its target to any window at all,
/// where one that names nothing means "the keyboard's pane" and has to reach the right window.
///
/// Nothing for a request naming nothing, which means this window's keyboard. For a move, the
/// destination, because that window is the one that has to show the pane arrive.
pub fn names(payload: &request::Payload) -> Option<Names<'_>> {
    fn tab(id: &str) -> Option<Names<'_>> {
        (!id.is_empty()).then_some(Names::Tab(id))
    }
    fn pane(id: &str) -> Option<Names<'_>> {
        (!id.is_empty()).then_some(Names::Pane(id))
    }
    use request::Payload;
    match payload {
        Payload::FocusTab(focus) => tab(&focus.tab_id),
        Payload::CloseTab(close) => tab(&close.tab_id),
        Payload::RenameTab(rename) => tab(&rename.tab_id),
        Payload::SetSplitRatio(set) => tab(&set.tab_id),
        Payload::ArrangePane(arrange) => tab(&arrange.tab_id)
            .or_else(|| pane(&arrange.onto_pane_id))
            .or_else(|| if arrange.new_tab { pane(&arrange.pane_id) } else { None }),
        Payload::FocusPane(focus) => pane(&focus.pane_id),
        Payload::ClosePane(close) => pane(&close.pane_id),
        Payload::SplitPane(split) => pane(&split.pane_id),
        Payload::CreateTab(create) => pane(&create.pane_id),
        Payload::ZoomPane(zoom) => pane(&zoom.pane_id),
        Payload::ResizePane(resize) => pane(&resize.pane_id),
        Payload::RenamePane(rename) => pane(&rename.pane_id),
        Payload::CompactPane(compact) => pane(&compact.pane_id),
        Payload::RestartAgent(restart) => pane(&restart.pane_id),
        Payload::HidePane(hide) => pane(&hide.pane_id),
        // Held by no window while it is hidden, so it is shown in whichever window asked.
        Payload::ShowPane(show) => pane(&show.pane_id),
        // The window holding the maker, whose helpers come forward beside it.
        Payload::ShowHelpers(show) => pane(&show.pane_id),
        Payload::HideHelpers(hide) => pane(&hide.pane_id),
        Payload::QuietPane(quiet) => pane(&quiet.pane_id),
        Payload::SendToPane(send) => pane(&send.pane_id),
        Payload::ReattachPane(reattach) => pane(&reattach.pane_id),
        Payload::EqualizePanes(even) => pane(&even.pane_id),
        _ => None,
    }
}

/// Every pane a request names that somebody may have typed, which may be a pane's name (`otter`,
/// `otter@devenv`) rather than its id, for the window to read before anything else does.
///
/// Here beside [`names`] for the same reason it is: a pane field this list missed would take an
/// id and refuse a name, and nothing would say so. What a shell sends on its own - a keystroke,
/// a click, a bridge - names a pane by the id it was handed, and is not listed, so the commonest
/// requests by far pay one match for this and nothing more.
pub fn typed_panes(payload: &mut request::Payload) -> Vec<&mut String> {
    use request::Payload;
    match payload {
        Payload::ArrangePane(arrange) => vec![&mut arrange.pane_id, &mut arrange.onto_pane_id],
        Payload::WatchPanes(watch) => watch.pane_ids.iter_mut().collect(),
        Payload::FocusPane(focus) => vec![&mut focus.pane_id],
        Payload::ClosePane(close) => vec![&mut close.pane_id],
        Payload::SplitPane(split) => vec![&mut split.pane_id, &mut split.maker],
        Payload::CreateTab(create) => vec![&mut create.pane_id],
        Payload::ZoomPane(zoom) => vec![&mut zoom.pane_id],
        Payload::ResizePane(resize) => vec![&mut resize.pane_id],
        Payload::RenamePane(rename) => vec![&mut rename.pane_id],
        Payload::CompactPane(compact) => vec![&mut compact.pane_id],
        Payload::RestartAgent(restart) => vec![&mut restart.pane_id],
        Payload::HidePane(hide) => vec![&mut hide.pane_id],
        Payload::ShowPane(show) => vec![&mut show.pane_id],
        Payload::ShowHelpers(show) => vec![&mut show.pane_id],
        Payload::HideHelpers(hide) => vec![&mut hide.pane_id],
        Payload::QuietPane(quiet) => vec![&mut quiet.pane_id],
        Payload::SendToPane(send) => vec![&mut send.pane_id],
        Payload::ReadPane(read) => vec![&mut read.pane_id],
        Payload::ReattachPane(reattach) => vec![&mut reattach.pane_id],
        Payload::EqualizePanes(even) => vec![&mut even.pane_id],
        _ => Vec::new(),
    }
}

/// Whether any open window can carry out this request, because it says everything it is about.
///
/// What lets the CLI, outside any pane and with several windows open, send a request to whichever
/// window answers first. A request naming its tab or pane is carried to the window holding it;
/// a tab move naming both the tab and where it goes is a write to a record every window shares.
/// Anything else means "this window" somewhere in it, and has to reach the right one.
pub fn any_window_will_do(payload: &request::Payload) -> bool {
    names(payload).is_some()
        || matches!(payload, request::Payload::MoveTab(moved)
            if !moved.tab_id.is_empty() && !moved.window.is_empty())
}

impl Request {
    /// A request for whichever window the core resolves it to, from no pane in particular.
    pub fn new(payload: request::Payload) -> Request {
        Request { payload: Some(payload), ..Request::default() }
    }

    /// The same request, for one window by name.
    #[must_use]
    pub fn for_window(mut self, window: impl Into<String>) -> Request {
        self.window = window.into();
        self
    }
}

impl Event {
    /// An event about the whole app rather than one window.
    pub fn new(payload: event::Payload) -> Event {
        Event { payload: Some(payload), ..Event::default() }
    }

    /// The same event, for one window by name.
    #[must_use]
    pub fn for_window(mut self, window: impl Into<String>) -> Event {
        self.window = window.into();
        self
    }
}

impl Response {
    /// Nothing to report, which is what most requests answer.
    ///
    /// Public because the endpoint's callers are in another crate now. It stays a constructor
    /// rather than something each caller spells out, so "success" has one spelling.
    pub fn ok() -> Response {
        Response { payload: Some(response::Payload::Ok(Ok::default())) }
    }

    /// A change made, with something its asker should know about it.
    pub fn noted(note: impl Into<String>) -> Response {
        Response { payload: Some(response::Payload::Ok(Ok { note: note.into() })) }
    }

    /// A refusal, written for whoever finds it in a log rather than for a caller that
    /// might branch on it.
    pub fn failure(reason: impl Into<String>) -> Response {
        Response { payload: Some(response::Payload::Failure(Failure { reason: reason.into() })) }
    }

    /// A change a daemon was asked for and never answered about, which may have happened - or a
    /// wait on a pane whose daemon stopped answering, whose pane may have got there unheard.
    pub fn unanswered(reason: impl Into<String>) -> Response {
        Response {
            payload: Some(response::Payload::Unanswered(Unanswered {
                reason: reason.into(),
                ..Unanswered::default()
            })),
        }
    }
}
