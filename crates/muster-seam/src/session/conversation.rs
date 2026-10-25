//! A group's conversation, as the app keeps it (mip/0009-group-conversation.md, sections 1-4 and
//! 7): which window shows which group in place of its panes, one copy of each shown group's log
//! fetched and followed as the human and shared by every window showing it, and each window's
//! message box with where a post from it stands.
//!
//! The rules - what folds, where the unread line goes, whom a draft addresses - are the core's
//! (`muster_core::conversation`). This asks the daemons, keeps what they answered, and tells the
//! shell.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use muster_core::attention::GroupKey;
use muster_core::composition::{DaemonId, PaneKey};
use muster_core::conversation::{self, Named};
use muster_core::conversation::{
    Draft, Edit, Group, Member, ThreadView, Unread, What, draft, members, thread,
};
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::intent::Refusal;
use muster_core::mirror::backend::PaneId;
use muster_core::posting::{self, Sent};
use muster_daemon_client::control::Control;
use muster_daemon_client::convert::messaging;
use muster_daemon_proto::msg_answer;
use muster_daemon_proto::msg_request::{self, Request as Asked};

use super::{SESSION, Session, WindowId, answer_as_human, answered_as_human, save};
use crate::proto::{self, Event, Response, event, response};
use crate::{convert, ffi};

/// How many entries a page of a group's history holds.
const PAGE: u32 = 200;

/// How long the thread following a group waits on its follow at a time, between looking at
/// whether anybody still shows the group and whether a member moved.
const SLICE: Duration = Duration::from_millis(250);

/// How long it waits before asking again a daemon that could not answer.
const RETRY: Duration = Duration::from_secs(1);

/// How long a draft waits, unchanged, before it is written down: typing is a burst of edits, and
/// the arrangement is one file.
const DRAFT_QUIET: Duration = Duration::from_secs(1);

/// One window's view of the conversation it shows.
#[derive(Debug, Clone)]
pub(crate) struct Showing {
    pub(crate) group: GroupKey,
    view: ThreadView,
    /// The window's last post here that was kept, and whom it woke, said under it.
    posted: Option<(u64, String)>,
}

/// Where a post from a message box stands, with what settling an unknown one from the log needs.
#[derive(Debug, Clone)]
pub(crate) struct Sending {
    sent: Sent,
    /// The draft as it was sent, which a kept post clears only while the box still holds it.
    draft: Draft,
    /// The last entry the conversation held when it was sent.
    after: u64,
}

/// What the app holds about a group some window shows.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    group: Group,
    /// Whether the first page has arrived.
    loaded: bool,
    /// Why the log could not be fetched, while it cannot.
    failed: Option<String>,
    /// A member's pane changed, so why it has not read may have too: `Who` is asked again.
    members_moved: bool,
    /// Which fetch of the group's log this is. A log let go and shown again at once is fetched
    /// afresh, and the thread still following the one before must leave the new one alone.
    generation: u64,
}

/// One follow thread's group: the log of one generation, which the thread fetches, follows and
/// writes into, and which it ends with.
#[derive(Debug, Clone)]
struct Following {
    group: GroupKey,
    generation: u64,
}

impl Following {
    /// The group's log while it is still the generation this thread fetched.
    fn shared<'a>(&self, session: &'a mut Session) -> Option<&'a mut Shared> {
        session
            .conversations
            .get_mut(&self.group)
            .filter(|shared| shared.generation == self.generation)
    }

    /// Whoever follows the group's log now, if any window shows it.
    fn now(group: &GroupKey) -> Option<Following> {
        let session = poison::lock(&SESSION, "session");
        let generation = session.conversations.get(group)?.generation;
        Some(Following { group: group.clone(), generation })
    }
}

impl Session {
    /// Whether the human is reading `group` in its conversation: shown in the window in front,
    /// while that window is focused (MIP-9, section 2). A second window showing it while another
    /// is in front reads nothing until it comes to the front.
    pub(super) fn looking_at_conversation(&self, group: &GroupKey) -> bool {
        let front = &self.windows[self.front];
        front.is_open()
            && self.attention.focused()
            && front.conversation.as_ref().is_some_and(|shown| &shown.group == group)
    }

    /// The open windows showing `group`'s conversation.
    fn showing(&self, group: &GroupKey) -> Vec<WindowId> {
        self.windows
            .iter()
            .filter(|(_, window)| {
                window.is_open()
                    && window.conversation.as_ref().is_some_and(|shown| &shown.group == group)
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// Gives each window showing `group` with no draft for it the one a conversation opens with,
    /// once there is a log to open it from.
    fn open_drafts(&mut self, group: &GroupKey) {
        let Some(shared) = self.conversations.get(group).filter(|shared| shared.loaded) else {
            return;
        };
        let opening = draft::opening(&shared.group);
        for window in self.windows.values_mut() {
            let shows = window.conversation.as_ref().is_some_and(|shown| &shown.group == group);
            if shows && !window.drafts.contains_key(group) {
                window.drafts.insert(group.clone(), opening.clone());
            }
        }
    }
}

/// Shows `group`'s conversation in place of `window`'s panes. An empty daemon is the first one on
/// this machine.
pub(crate) fn show(window: WindowId, daemon: &str, group: &str) -> Response {
    if group.is_empty() {
        return Response::failure(
            "no group was named, so no conversation was shown: `muster msg open --group G`",
        );
    }
    let daemon = match daemon {
        "" => match super::first_local_daemon(window) {
            Some(daemon) => daemon,
            None => {
                return Response::failure(
                    "this window is attached to no daemon on this machine, so no conversation \
                     was shown; name one with --daemon",
                );
            }
        },
        named => DaemonId::new(named),
    };
    let (key, name, start) = {
        let mut session = poison::lock(&SESSION, "session");
        let Some(backend) = session.backends.get(&daemon) else {
            return Response::failure(format!(
                "this window is attached to no daemon called {daemon}, so {group}'s conversation \
                 was not shown"
            ));
        };
        let mirror = poison::lock(&backend.mirror, "mirror");
        let known = mirror.human_notices().map(|(group, _)| group.as_str());
        let group = match conversation::named(group, known) {
            Named::Group(group) => group,
            Named::Unknown(known) => return unknown_group(group, &daemon, &known),
            Named::Several(several) => {
                return Response::failure(format!(
                    "{group} is the name of a group on more than one machine ({}), so no \
                     conversation was shown; name the one you mean",
                    several.join(", ")
                ));
            }
        };
        let notice = mirror.human_notice(&group).cloned();
        drop(mirror);
        let key = GroupKey { daemon, group };
        // Where the unread messages begin, taken before showing it reads them.
        let unread_from =
            notice.filter(|notice| notice.count > 0 && notice.first > 0).map(|notice| notice.first);
        let shows = session.windows[window].conversation.as_ref().map(|shown| shown.group.clone());
        if shows.as_ref() != Some(&key) {
            session.windows[window].conversation = Some(Showing {
                group: key.clone(),
                view: ThreadView { unread_from, ..ThreadView::default() },
                posted: None,
            });
            if let Some(left) = shows {
                forget_unless_shown(&mut session, &left);
            }
        }
        let start = (!session.conversations.contains_key(&key)).then(|| {
            static GENERATIONS: AtomicU64 = AtomicU64::new(0);
            let generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
            session.conversations.insert(key.clone(), Shared { generation, ..Shared::default() });
            Following { group: key.clone(), generation }
        });
        session.open_drafts(&key);
        let name = session.windows[window].name.to_string();
        (key, name, start)
    };
    log::info(
        "conversation.shown",
        fields! { "daemon" => key.daemon.to_string(), "group" => &key.group, "window" => &name },
    );
    if let Some(following) = start {
        follow(&following);
    }
    super::publish("conversation");
    // Read for the human if they are reading it now, which also takes its banner and its ask
    // down.
    super::human_noticed(&key.daemon, &key.group);
    announce(&[window]);
    Response { payload: Some(response::Payload::Opened(proto::Opened { window: name })) }
}

fn unknown_group(group: &str, daemon: &DaemonId, known: &[String]) -> Response {
    let known = if known.is_empty() {
        "you are in no group there".to_string()
    } else {
        format!("the groups you are in there are {}", known.join(", "))
    };
    Response::failure(format!(
        "you are in no group called {group} on {daemon}, so no conversation was shown; {known}"
    ))
}

/// Returns `window` to its panes, with the keyboard where it was. Says whether it was showing a
/// conversation.
pub(crate) fn leave(window: WindowId) -> bool {
    let left = {
        let mut session = poison::lock(&SESSION, "session");
        let left = session.windows[window].conversation.take();
        if let Some(shown) = &left {
            forget_unless_shown(&mut session, &shown.group);
        }
        left
    };
    let Some(left) = left else { return false };
    log::info(
        "conversation.left",
        fields! { "daemon" => left.group.daemon.to_string(), "group" => left.group.group },
    );
    super::publish("conversation");
    announce(&[window]);
    true
}

/// Lets go of a group's log once no window shows it. The thread following it sees that and ends.
fn forget_unless_shown(session: &mut Session, group: &GroupKey) {
    if session.showing(group).is_empty() {
        session.conversations.remove(group);
    }
}

/// Fetches `group`'s log and follows it, off the caller's thread, for as long as a window shows
/// it.
fn follow(following: &Following) {
    let spawned = std::thread::Builder::new().name("muster-conversation".into()).spawn({
        let following = following.clone();
        move || follow_log(&following)
    });
    if let Err(error) = spawned {
        failed(following, &format!("no thread could be started to fetch it ({error})"));
    }
}

fn follow_log(following: &Following) {
    let group = &following.group;
    loop {
        if !shown(following) {
            return;
        }
        match first_page(following) {
            Ok(()) => break,
            Err(why) => {
                failed(following, &why);
                std::thread::sleep(RETRY);
            }
        }
    }
    refresh_policy(following);
    refresh_members(following);
    announce_group(group, Parts::ALL);
    let mut connection: Option<Control> = None;
    let mut waiting = None;
    loop {
        if !shown(following) {
            return;
        }
        if members_moved(following) {
            refresh_members(following);
            announce_group(group, Parts::MEMBERS);
        }
        if waiting.is_none() {
            if connection.is_none() {
                match connect(group) {
                    Ok(opened) => connection = Some(opened),
                    Err(why) => {
                        again(group, &why);
                        continue;
                    }
                }
            }
            let log = msg_request::Log {
                group: group.group.clone(),
                since: head(following),
                follow: true,
                ..Default::default()
            };
            waiting = connection.as_ref().map(|opened| opened.as_human(Asked::Log(log)));
        }
        let Some(pending) = waiting.take() else { continue };
        match pending.poll(SLICE) {
            None => waiting = Some(pending),
            Some(Err(ended)) => {
                connection = None;
                again(group, &ended.to_string());
            }
            Some(Ok(answer)) => match answered_as_human(answer) {
                Ok(answer) => arrived(following, &answer),
                Err(refusal) => again(group, refusal.detail()),
            },
        }
    }
}

/// A connection of its own to the group's daemon, for its follow. The daemon answers one
/// connection's requests in order, and a follow is answered only once an entry arrives, so on the
/// window's own connection it would hold up every request after it: a post, a read, a pause.
fn connect(group: &GroupKey) -> Result<Control, String> {
    let socket = poison::lock(&SESSION, "session")
        .daemon_socket(&group.daemon)
        .ok_or_else(|| format!("this window is no longer attached to {}", group.daemon))?;
    Control::open(std::path::Path::new(&socket), "muster-conversation", |_, _| {})
        .map_err(|error| error.to_string())
}

/// Whether some open window still shows the group, in the log this thread fetched. Its log is
/// let go once none does, including once the last window showing it closed.
fn shown(following: &Following) -> bool {
    let mut session = poison::lock(&SESSION, "session");
    forget_unless_shown(&mut session, &following.group);
    following.shared(&mut session).is_some()
}

fn head(following: &Following) -> u64 {
    let mut session = poison::lock(&SESSION, "session");
    following.shared(&mut session).map_or(0, |shared| shared.group.log.head())
}

fn members_moved(following: &Following) -> bool {
    let mut session = poison::lock(&SESSION, "session");
    following.shared(&mut session).is_some_and(|shared| std::mem::take(&mut shared.members_moved))
}

/// A follow that could not be answered: the daemon went, or a handover ended it. Asked again from
/// the last entry held once the daemon answers.
fn again(group: &GroupKey, why: &str) {
    log::info(
        "conversation.follow_again",
        fields! { "daemon" => group.daemon.to_string(), "group" => &group.group, "why" => why },
    );
    std::thread::sleep(RETRY);
}

fn failed(following: &Following, why: &str) {
    let group = &following.group;
    log::warn(
        "conversation.fetch_failed",
        fields! {
            "daemon" => group.daemon.to_string(),
            "group" => &group.group,
            "error" => why,
            "impact" => "the conversation shows no messages, and says why, until the log can be \
                         fetched; it keeps asking",
            "check" => "whether `muster msg log --group` answers for that group on that daemon",
        },
    );
    if let Some(shared) = following.shared(&mut poison::lock(&SESSION, "session")) {
        shared.failed = Some(why.to_string());
    }
    announce_group(group, Parts::ALL);
}

/// The newest page of the group's log. A daemon before 1.8 does not page, and answers the whole
/// log: what a window showing a group kept there gets (MIP-9, section 7).
fn first_page(following: &Following) -> Result<(), String> {
    let group = &following.group;
    let log =
        msg_request::Log { group: group.group.clone(), newest: Some(PAGE), ..Default::default() };
    let answer = answer_as_human(&group.daemon, Asked::Log(log))
        .map_err(|refusal| refusal.detail().to_string())?;
    let held = messaging::log_page(&answer, &group.group);
    let count = held.entries.len();
    {
        let mut session = poison::lock(&SESSION, "session");
        let Some(shared) = following.shared(&mut session) else { return Ok(()) };
        shared.group.called.extend(answer.called.clone());
        shared.group.log.behind = held.behind;
        shared.group.log.seen(held.last);
        shared.group.log.page(held.entries, held.earlier);
        shared.loaded = true;
        shared.failed = None;
        session.open_drafts(group);
    }
    log::info(
        "conversation.fetched",
        fields! {
            "daemon" => group.daemon.to_string(),
            "group" => &group.group,
            "entries" => count,
            "earlier" => held.earlier,
        },
    );
    Ok(())
}

/// Entries that followed the last one held.
fn arrived(following: &Following, answer: &muster_daemon_proto::MsgAnswer) {
    let group = &following.group;
    let held = messaging::log_page(answer, &group.group);
    let changed_group = held.entries.iter().any(|entry| !matches!(entry.what, What::Message(_)));
    // Not read here for the human: what wakes them arrives as the daemon's notice too, and
    // `human_noticed` reads it while they look. A message that wakes nobody asks for nothing.
    let settled = {
        let mut session = poison::lock(&SESSION, "session");
        let Some(shared) = following.shared(&mut session) else { return };
        shared.group.called.extend(answer.called.clone());
        shared.group.log.behind = held.behind;
        shared.group.log.seen(held.last);
        let new = shared.group.log.followed(held.entries);
        if new.is_empty() {
            return;
        }
        settle(&mut session, group)
    };
    if changed_group {
        refresh_policy(following);
    }
    refresh_members(following);
    for window in settled {
        save_soon(window);
    }
    announce_group(group, Parts::ALL);
}

/// Settles each window's unknown post to `group` that the log now shows was kept: its draft
/// clears, as a kept post's does. Says which windows' drafts changed.
fn settle(session: &mut Session, group: &GroupKey) -> Vec<WindowId> {
    let Some(shared) = session.conversations.get(group) else { return Vec::new() };
    let opening = draft::opening(&shared.group);
    let mut settled = Vec::new();
    for id in session.windows.ids() {
        let window = &mut session.windows[id];
        let Some(sending) = window.sending.get(group) else { continue };
        if !matches!(sending.sent, Sent::Unknown { .. }) {
            continue;
        }
        let Some(seq) = posting::settles(
            shared.group.log.logged(),
            sending.after,
            &sending.draft.to,
            &sending.draft.body(),
        ) else {
            continue;
        };
        let said = posting::KEPT_UNANSWERED.to_string();
        let sent = sending.draft.clone();
        window.sending.insert(
            group.clone(),
            Sending {
                sent: Sent::Posted { seq, said: said.clone(), heard: true },
                draft: sent.clone(),
                after: seq,
            },
        );
        clear_if_unchanged(window, group, &sent, &opening);
        if let Some(shown) = window.conversation.as_mut().filter(|shown| &shown.group == group) {
            shown.posted = Some((seq, said));
        }
        settled.push(id);
    }
    settled
}

/// Puts the opening draft in place of the one a kept post was sent from, unless the human has
/// written something else in the box since: that is theirs, and was never sent.
fn clear_if_unchanged(window: &mut super::Window, group: &GroupKey, sent: &Draft, opening: &Draft) {
    let unchanged = window.drafts.get(group).is_none_or(|held| {
        held.text == sent.text
            && held.to == sent.to
            && held.urgent == sent.urgent
            && held.picked == sent.picked
    });
    if unchanged {
        window.drafts.insert(group.clone(), opening.clone());
    }
}

/// Asks the group's daemon for its policy, which says whom an unaddressed post wakes and whether
/// the group is paused.
fn refresh_policy(following: &Following) {
    let group = &following.group;
    let answer = match answer_as_human(&group.daemon, Asked::Groups(msg_request::Groups {})) {
        Ok(answer) => answer,
        Err(refusal) => {
            log::info(
                "conversation.policy_unknown",
                fields! {
                    "daemon" => group.daemon.to_string(),
                    "group" => &group.group,
                    "why" => refusal.detail(),
                },
            );
            return;
        }
    };
    let Some(msg_answer::Answer::Groups(groups)) = answer.answer else { return };
    let Some(policy) = groups
        .groups
        .into_iter()
        .find(|held| held.name == group.group)
        .and_then(|held| held.policy)
    else {
        return;
    };
    let policy = messaging::policy(policy);
    if let Some(shared) = following.shared(&mut poison::lock(&SESSION, "session")) {
        shared.group.policy = Some(policy);
    }
}

/// Asks the group's daemon who is in it, and why each member has not read what waits on it.
fn refresh_members(following: &Following) {
    let group = &following.group;
    let who = msg_request::Who { group: Some(group.group.clone()) };
    let answer = match answer_as_human(&group.daemon, Asked::Who(who)) {
        Ok(answer) => answer,
        Err(refusal) => {
            log::info(
                "conversation.members_unknown",
                fields! {
                    "daemon" => group.daemon.to_string(),
                    "group" => &group.group,
                    "why" => refusal.detail(),
                },
            );
            return;
        }
    };
    let called = answer.called.clone();
    let Some(msg_answer::Answer::Members(listed)) = answer.answer else { return };
    let mut session = poison::lock(&SESSION, "session");
    let panes: Vec<Option<PaneKey>> = listed
        .members
        .iter()
        .map(|member| member.pane.as_deref().and_then(|pane| pane_of(&session, group, pane)))
        .collect();
    let Some(shared) = following.shared(&mut session) else { return };
    shared.group.called.extend(called);
    let members = listed
        .members
        .into_iter()
        .zip(panes)
        .map(|(member, pane)| {
            let liveness = messaging::liveness(member.liveness());
            let unread = member.unread.into_iter().find(|unread| unread.group == group.group).map(
                |unread| Unread {
                    first: unread.first,
                    from: unread
                        .from
                        .iter()
                        .map(|from| shared.group.shown(from))
                        .collect::<Vec<_>>()
                        .join(", "),
                    why: unread.why,
                },
            );
            Member { liveness, name: member.name, pane, unread }
        })
        .collect();
    shared.group.members = Some(members);
}

/// The pane a member is in, when this window follows it: on the group's daemon, or for a member
/// on another machine, whichever attached daemon holds it. A pane's name is unique across
/// machines.
fn pane_of(session: &Session, group: &GroupKey, pane: &str) -> Option<PaneKey> {
    let id = PaneId::new(pane.split('@').next().unwrap_or(pane));
    let holds = |daemon: &DaemonId| {
        session.backends.get(daemon).is_some_and(|backend| {
            poison::lock(&backend.mirror, "mirror").panes().any(|held| held.id == id)
        })
    };
    if holds(&group.daemon) {
        return Some(PaneKey::new(&group.daemon, &id));
    }
    session.backends.keys().find(|daemon| holds(daemon)).map(|daemon| PaneKey::new(daemon, &id))
}

/// Fetches the page of `group`'s log before the earliest entry held, for every window showing it.
pub(crate) fn load_earlier(daemon: &str, group: &str) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let before = {
        let session = poison::lock(&SESSION, "session");
        let Some(shared) = session.conversations.get(&key) else {
            return Response::failure(format!(
                "no window shows {group}'s conversation, so there is nothing to page"
            ));
        };
        if !shared.group.log.earlier() {
            return Response::noted("the conversation already holds the group's whole log");
        }
        shared.group.log.earliest()
    };
    let log = msg_request::Log {
        group: key.group.clone(),
        newest: Some(PAGE),
        before,
        ..Default::default()
    };
    let answer = match answer_as_human(&key.daemon, Asked::Log(log)) {
        Ok(answer) => answer,
        Err(refusal) => return refused(&refusal),
    };
    let held = messaging::log_page(&answer, &key.group);
    if let Some(shared) = poison::lock(&SESSION, "session").conversations.get_mut(&key) {
        shared.group.called.extend(answer.called.clone());
        shared.group.log.page(held.entries, held.earlier);
    }
    announce_group(&key, Parts::ALL);
    Response::ok()
}

/// Pauses `group` or resumes it, as the human.
pub(crate) fn pause(daemon: &str, group: &str, paused: bool) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let asked = if paused {
        Asked::Pause(msg_request::Pause { group: key.group.clone() })
    } else {
        Asked::Resume(msg_request::Resume { group: key.group.clone() })
    };
    if let Err(refusal) = answer_as_human(&key.daemon, asked) {
        return refused(&refusal);
    }
    if let Some(following) = Following::now(&key) {
        refresh_policy(&following);
        announce_group(&key, Parts::ALL);
    }
    Response::ok()
}

fn refused(refusal: &Refusal) -> Response {
    match refusal {
        Refusal::Unanswered { detail, .. } => Response::unanswered(detail.clone()),
        refusal => Response::failure(refusal.detail().to_string()),
    }
}

/// Opens the fold whose first message is `first`, for `window` until it leaves.
pub(crate) fn open_fold(window: WindowId, daemon: &str, group: &str, first: u64) -> Response {
    viewing(window, daemon, group, |view| {
        view.open_folds.insert(first);
    })
}

/// Shows message `seq` whole in `window`.
pub(crate) fn show_whole(window: WindowId, daemon: &str, group: &str, seq: u64) -> Response {
    viewing(window, daemon, group, |view| {
        view.whole.insert(seq);
    })
}

fn viewing(
    window: WindowId,
    daemon: &str,
    group: &str,
    change: impl FnOnce(&mut ThreadView),
) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    {
        let mut session = poison::lock(&SESSION, "session");
        match session.windows[window].conversation.as_mut() {
            Some(shown) if shown.group == key => change(&mut shown.view),
            _ => {
                return Response::failure(format!(
                    "this window is not showing {group}'s conversation, so nothing in it was \
                     opened"
                ));
            }
        }
    }
    announce(&[window]);
    Response::ok()
}

/// Takes the message box as the shell holds it after the person changed it, and answers with the
/// draft as the core keeps it. Never announced: the shell already holds what it sent, and an
/// event carrying it back could land after the next keystroke and undo it.
pub(crate) fn save_draft(window: WindowId, daemon: &str, group: &str, edit: Edit) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let answer = {
        let mut session = poison::lock(&SESSION, "session");
        let Session { conversations, windows, .. } = &mut *session;
        let unheard = Group::default();
        let group = conversations.get(&key).map_or(&unheard, |shared| &shared.group);
        let before =
            windows[window].drafts.get(&key).cloned().unwrap_or_else(|| draft::opening(group));
        let edited = draft::edited(&before, edit, group);
        let answer =
            convert::message_box_draft(&edited.draft, edited.cursor, &edited.completions, group);
        let held = &mut windows[window];
        held.drafts.insert(key.clone(), edited.draft);
        if held.conversation.as_ref().is_some_and(|shown| shown.group == key) {
            let told = message_box(held, group);
            held.message_box_sent = Some(told);
        }
        answer
    };
    save_soon(window);
    Response { payload: Some(response::Payload::Draft(answer)) }
}

/// Picks answer `answer` of question `question` for `window`'s message box in `group`, or takes
/// the pick back, and answers with the draft (MIP-9, section 6). Nothing is sent.
pub(crate) fn pick(
    window: WindowId,
    daemon: &str,
    group: &str,
    question: u64,
    answer: Option<u32>,
) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let answer = answer.map(|answer| usize::try_from(answer).unwrap_or(usize::MAX));
    let answered = {
        let mut session = poison::lock(&SESSION, "session");
        let Session { conversations, windows, .. } = &mut *session;
        let unheard = Group::default();
        let group = conversations.get(&key).map_or(&unheard, |shared| &shared.group);
        let before =
            windows[window].drafts.get(&key).cloned().unwrap_or_else(|| draft::opening(group));
        let picked = match draft::pick(&before, question, answer, group) {
            Ok(picked) => picked,
            Err(why) => return Response::failure(why),
        };
        let cursor = picked.text.chars().count();
        let answered = convert::message_box_draft(&picked, cursor, &[], group);
        let held = &mut windows[window];
        held.drafts.insert(key.clone(), picked);
        if held.conversation.as_ref().is_some_and(|shown| shown.group == key) {
            let told = message_box(held, group);
            held.message_box_sent = Some(told);
        }
        answered
    };
    save_soon(window);
    Response { payload: Some(response::Payload::Draft(answered)) }
}

/// Posts `window`'s draft for `group` as the human (MIP-9, sections 3 and 4).
pub(crate) fn post(window: WindowId, daemon: &str, group: &str) -> Response {
    let key = GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() };
    let (draft, after) = {
        let mut session = poison::lock(&SESSION, "session");
        let after = session.conversations.get(&key).map_or(0, |shared| shared.group.log.head());
        let held = &mut session.windows[window];
        let Some(draft) = held.drafts.get(&key).filter(|draft| draft.sends()) else {
            return Response::failure(
                "the message box is empty and no answer is picked, so nothing was posted",
            );
        };
        if matches!(held.sending.get(&key), Some(Sending { sent: Sent::Pending { .. }, .. })) {
            return Response::failure(
                "a post from this message box is still on its way; it was not sent again",
            );
        }
        let draft = draft.clone();
        held.sending.insert(
            key.clone(),
            Sending {
                sent: Sent::Pending { text: draft.text.clone() },
                draft: draft.clone(),
                after,
            },
        );
        (draft, after)
    };
    announce(&[window]);
    let sent = super::posting::post_as_human(
        &key,
        &draft.to,
        &draft.body(),
        draft.picked.as_ref(),
        draft.urgent,
        after,
    );
    log::info(
        "conversation.posted",
        fields! {
            "daemon" => key.daemon.to_string(),
            "group" => &key.group,
            "outcome" => super::posting::outcome(&sent),
        },
    );
    let response = super::posting::response(&sent);
    let settled = {
        let mut session = poison::lock(&SESSION, "session");
        let Session { conversations, windows, .. } = &mut *session;
        let unheard = Group::default();
        let group = conversations.get(&key).map_or(&unheard, |shared| &shared.group);
        let held = &mut windows[window];
        if let Sent::Posted { seq, said, .. } = &sent {
            clear_if_unchanged(held, &key, &draft, &draft::opening(group));
            if let Some(shown) = held.conversation.as_mut().filter(|shown| shown.group == key) {
                shown.posted = Some((*seq, said.clone()));
            }
        }
        held.sending.insert(key.clone(), Sending { sent, draft, after });
        // The follow may have brought the post's entry while it was still pending, when there
        // was nothing to settle, and nothing would bring it again.
        settle(&mut session, &key)
    };
    let mut changed = settled;
    if !changed.contains(&window) {
        changed.push(window);
    }
    for each in &changed {
        save_soon(*each);
    }
    announce(&changed);
    response
}

/// Notes that a pane's agent changed state, so a group showing it as a member asks again why it
/// has not read: whether it is at a dialog, or at work, is what that says.
pub(crate) fn member_moved(pane: &PaneKey) {
    let mut session = poison::lock(&SESSION, "session");
    for shared in session.conversations.values_mut() {
        let member = shared.group.members.iter().flatten().any(|m| m.pane.as_ref() == Some(pane));
        if member {
            shared.members_moved = true;
        }
    }
}

/// Writes `window`'s drafts down once they have been still for a moment.
fn save_soon(window: WindowId) {
    static DUE: Mutex<(BTreeSet<WindowId>, Option<Instant>)> = Mutex::new((BTreeSet::new(), None));
    let starting = {
        let mut due = poison::lock(&DUE, "drafts");
        due.0.insert(window);
        let starting = due.1.is_none();
        due.1 = Some(Instant::now() + DRAFT_QUIET);
        starting
    };
    if !starting {
        return;
    }
    let spawned = std::thread::Builder::new().name("muster-drafts".into()).spawn(|| {
        let windows = loop {
            let wait = {
                let mut due = poison::lock(&DUE, "drafts");
                match due.1 {
                    Some(at) if at > Instant::now() => at - Instant::now(),
                    _ => {
                        due.1 = None;
                        break std::mem::take(&mut due.0);
                    }
                }
            };
            std::thread::sleep(wait);
        };
        let mut session = poison::lock(&SESSION, "session");
        for window in windows {
            save(&mut session, window);
        }
    });
    if let Err(error) = spawned {
        log::warn(
            "conversation.drafts_unsaved",
            fields! {
                "error" => error.to_string(),
                "impact" => "the drafts are written with the arrangement's next save instead, \
                             which every change to the window makes",
                "check" => "the process's thread limit",
            },
        );
        poison::lock(&DUE, "drafts").1 = None;
    }
}

/// Which of a conversation's three events to settle: each changes at its own pace, and settling
/// one is never a reason to rebuild another.
#[derive(Debug, Clone, Copy)]
struct Parts {
    thread: bool,
    members: bool,
    message_box: bool,
}

impl Parts {
    const ALL: Parts = Parts { thread: true, members: true, message_box: true };
    const MEMBERS: Parts = Parts { thread: false, members: true, message_box: false };
}

/// Tells the shell what changed of every window showing `group`.
fn announce_group(group: &GroupKey, parts: Parts) {
    let windows = poison::lock(&SESSION, "session").showing(group);
    announce_parts(&windows, parts);
}

/// Tells the shell what each of these windows' conversation, members and message box are, where
/// they changed since it was last told.
pub(crate) fn announce(windows: &[WindowId]) {
    announce_parts(windows, Parts::ALL);
}

/// Settled and sent under one lock of its own, as the banners are: the handler's thread and each
/// group's following thread call this, and one that settled first and sent second would leave the
/// shell drawing an older thread.
fn announce_parts(windows: &[WindowId], parts: Parts) {
    static TELLING: Mutex<()> = Mutex::new(());
    let _telling = poison::lock(&TELLING, "conversation");
    let mut events = Vec::new();
    {
        let mut session = poison::lock(&SESSION, "session");
        let Session { conversations, windows: held, .. } = &mut *session;
        let unheard = Shared::default();
        for &id in windows {
            let window = &mut held[id];
            let name = window.name.to_string();
            let shown = window
                .conversation
                .as_ref()
                .map(|shown| (shown, conversations.get(&shown.group).unwrap_or(&unheard)));
            if parts.thread {
                let thread = shown
                    .map_or_else(proto::ConversationChanged::default, |(shown, shared)| {
                        conversation(shown, shared)
                    });
                if window.conversation_sent.as_ref() != Some(&thread) {
                    window.conversation_sent = Some(thread.clone());
                    events.push(
                        Event::new(event::Payload::ConversationChanged(thread)).for_window(&name),
                    );
                }
            }
            if parts.members {
                let members = shown.map_or_else(
                    proto::ConversationMembersChanged::default,
                    |(shown, shared)| {
                        convert::conversation_members(&shown.group, &members::rows(&shared.group))
                    },
                );
                if window.members_sent.as_ref() != Some(&members) {
                    window.members_sent = Some(members.clone());
                    events.push(
                        Event::new(event::Payload::ConversationMembersChanged(members))
                            .for_window(&name),
                    );
                }
            }
            if parts.message_box {
                let group = shown.map_or(&unheard.group, |(_, shared)| &shared.group);
                let told = message_box(window, group);
                if window.message_box_sent.as_ref() != Some(&told) {
                    window.message_box_sent = Some(told.clone());
                    events.push(
                        Event::new(event::Payload::MessageBoxChanged(told)).for_window(&name),
                    );
                }
            }
        }
    }
    for event in events {
        ffi::emit(&event);
    }
}

fn conversation(shown: &Showing, shared: &Shared) -> proto::ConversationChanged {
    let group = &shared.group;
    let view = ThreadView { utc_offset_s: utc_offset_s(), ..shown.view.clone() };
    let rows = thread::rows(group, &view);
    let mut changed = convert::conversation(
        &shown.group,
        &members::header(&shown.group, group),
        &rows,
        shown.posted.as_ref(),
    );
    changed.earlier = group.log.earlier();
    changed.loading = !shared.loaded;
    changed.failed = shared.failed.clone().unwrap_or_default();
    changed
}

fn message_box(window: &super::Window, group: &Group) -> proto::MessageBoxChanged {
    let Some(shown) = &window.conversation else { return proto::MessageBoxChanged::default() };
    let key = &shown.group;
    proto::MessageBoxChanged {
        daemon_id: key.daemon.to_string(),
        group: key.group.clone(),
        draft: window.drafts.get(key).map(|held: &Draft| {
            convert::message_box_draft(held, held.text.chars().count(), &[], group)
        }),
        sent: window.sending.get(key).map(|sending| convert::sent(&sending.sent)),
    }
}

/// This machine's offset from UTC now, in seconds, which the thread's dates are in: the window is
/// on this machine, whichever machine keeps the group. Read at every rebuild, so a change to
/// daylight saving time moves the dividers with it.
fn utc_offset_s() -> i32 {
    // SAFETY: `time` with a null pointer only returns the time, and `localtime_r` writes into the
    // `tm` it is handed and nothing else, which is why it is the reentrant one.
    let local = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&raw const now, &raw mut local).is_null()).then_some(local)
    };
    local.and_then(|local| i32::try_from(local.tm_gmtoff).ok()).unwrap_or(0)
}
