//! The message service against `corpus/conformance/messaging.json`: each case is a script of
//! steps by named callers, and each step's result is rendered as one line, so a case reads as a
//! transcript of what the service said.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use conformance::{Conformance, fields};
use muster_msg::{
    Action, Activity, Answering, Caller, Change, Changed, Draft, Inbox, Liveness, Memory,
    Messaging, Notice, Page, Participant, Policy, Posted, Presence, Reach, Refusal, Ringable, Via,
    What,
};
use serde_json::{Value, json};

/// Every inbox answers except those whose session has died, and a pane has an agent in it while
/// a step has said so. A pane may also be open with no agent in it, new or not.
#[derive(Default)]
struct Sessions {
    dead: RefCell<BTreeSet<String>>,
    agents: RefCell<BTreeMap<String, Activity>>,
    /// Agents whose prompt the host cannot read.
    unreadable: RefCell<BTreeSet<String>>,
    /// Open panes with no agent, and whether each is new.
    open: RefCell<BTreeMap<String, bool>>,
    /// What each pane is called, when a step said.
    names: RefCell<BTreeMap<String, String>>,
    /// Each pane's callsign, when a step gave it one apart from what it is called.
    callsigns: RefCell<BTreeMap<String, String>>,
    /// What each pane was called before, when a step said.
    formerly: RefCell<BTreeMap<String, Vec<String>>>,
    /// What each pane shows as its title, when a step said.
    titles: RefCell<BTreeMap<String, String>>,
    /// Whether a window is attending, which wakes the human.
    attended: Cell<bool>,
}

impl Sessions {
    fn answers(&self, inbox: &Inbox) -> bool {
        !self.dead.borrow().contains(&inbox.socket)
    }
}

impl Presence for Sessions {
    /// An agent in a pane is there while its pane has an agent, whatever its inbox says, as in
    /// the daemon: the pane is what the daemon watches.
    fn alive(&self, participant: &Participant) -> bool {
        match (&participant.pane, &participant.inbox) {
            (Some(pane), _) => self.agent_in(pane),
            (None, Some(inbox)) => self.answers(inbox),
            (None, None) => true,
        }
    }

    fn activity(&self, participant: &Participant) -> Option<Activity> {
        participant.pane.as_ref().and_then(|pane| self.agents.borrow().get(pane).copied())
    }

    fn agent_in(&self, pane: &str) -> bool {
        self.agents.borrow().contains_key(pane)
    }

    fn has_pane(&self, pane: &str) -> bool {
        self.agent_in(pane) || self.open.borrow().contains_key(pane)
    }

    fn name_of(&self, pane: &str) -> Option<String> {
        self.callsigns.borrow().get(pane).or(self.names.borrow().get(pane)).cloned()
    }

    fn pane_named(&self, name: &str) -> Option<String> {
        let find = |held: &BTreeMap<String, String>| {
            held.iter().find(|(_, called)| *called == name).map(|(pane, _)| pane.clone())
        };
        find(&self.callsigns.borrow()).or_else(|| find(&self.names.borrow()))
    }

    fn given_name(&self, pane: &str) -> Option<String> {
        self.names.borrow().get(pane).cloned()
    }

    fn panes_once_named(&self, name: &str) -> Vec<String> {
        let formerly = self.formerly.borrow();
        let once = formerly.iter().filter(|(_, before)| before.iter().any(|was| was == name));
        once.map(|(pane, _)| pane.clone()).collect()
    }

    fn titles(&self) -> Vec<(String, String)> {
        self.titles.borrow().iter().map(|(pane, title)| (pane.clone(), title.clone())).collect()
    }

    fn attended(&self) -> bool {
        self.attended.get()
    }

    fn doorbell(&self, pane: &str) -> Ringable {
        if self.agent_in(pane) && self.unreadable.borrow().contains(pane) {
            Ringable::NoPrompt
        } else if self.agent_in(pane) {
            Ringable::Rings
        } else if self.open.borrow().get(pane) == Some(&true) {
            Ringable::AgentToCome
        } else {
            Ringable::NoAgent
        }
    }
}

fn socket(session: &str) -> String {
    format!("/tmp/cc-socks/{session}.sock")
}

fn caller(step: &Value) -> Caller {
    let text = |key: &str| step.get(key).and_then(Value::as_str).map(str::to_string);
    Caller {
        as_name: text("as"),
        inbox: text("session").map(|session| Inbox {
            socket: socket(&session),
            inode: step.get("inode").and_then(Value::as_u64).unwrap_or(1),
        }),
        pane: text("pane"),
        directory: text("directory"),
        at_ms: 0,
    }
}

fn notice(notice: &Notice) -> String {
    format!(
        "{} #{}-{} x{} to-you:{}{} from {}{}",
        notice.group,
        notice.first,
        notice.last,
        notice.count,
        notice.to_you,
        if notice.urgent > 0 { format!(" urgent:{}", notice.urgent) } else { String::new() },
        notice.from.join(","),
        if notice.again { " again" } else { "" }
    )
}

fn activity(activity: Activity) -> &'static str {
    match activity {
        Activity::Working => "working",
        Activity::Blocked => "blocked",
        Activity::Idle => "idle",
        Activity::Waiting => "waiting",
        Activity::Stuck => "stuck",
    }
}

/// Where a wake went, when it went to a pane or the windows rather than an inbox.
fn rung(via: &Via) -> String {
    match via {
        Via::Inbox(_) => String::new(),
        Via::Pane(pane) => format!(" rung {pane}"),
        Via::Human => " for the windows".to_string(),
    }
}

fn refused(refusal: &Refusal) -> String {
    let detail = match refusal {
        Refusal::BadName { name } | Refusal::NoSuchParticipant { name } => name.clone(),
        Refusal::WhichParticipant { name, candidates }
        | Refusal::Shadowed { name, candidates }
        | Refusal::Renamed { name, panes: candidates }
        | Refusal::Titled { name, panes: candidates } => {
            format!("{name} {}", candidates.join(","))
        }
        Refusal::Unreachable { group, machine }
        | Refusal::Unanswered { group, machine }
        | Refusal::KeptElsewhere { group, machine } => {
            format!("{group} {machine}")
        }
        Refusal::Unchecked { group, machines } => format!("{group} {}", machines.join(",")),
        Refusal::HumanElsewhere { machine, calls_us } => format!("{machine} {calls_us}"),
        Refusal::NotAParticipant { name } => {
            name.clone().unwrap_or_else(|| "this session".to_string())
        }
        Refusal::NameInUse { name, inbox } => {
            format!("{name} at {}", inbox.as_deref().unwrap_or("no inbox"))
        }
        Refusal::NameOfPane { name, pane } => format!("{name} {pane}"),
        Refusal::NoSuchGroup { group }
        | Refusal::PairTooLong { group }
        | Refusal::GroupExists { group } => group.clone(),
        Refusal::GroupNameClash { group, existing } => format!("{group} {existing}"),
        Refusal::NotAMember { name, group, permitted: Some(permitted) } => {
            format!("{name} {group}, added by {}", permitted.join(","))
        }
        Refusal::NotAMember { name, group, permitted: None }
        | Refusal::AddresseeNotInGroup { name, group } => format!("{name} {group}"),
        Refusal::WhichGroup { candidates } => candidates.join(","),
        Refusal::Unread { group, count } => format!("{group} {count}"),
        Refusal::BodyTooLarge { bytes } => bytes.to_string(),
        Refusal::NotAllowed { addressee, group, allowed } => {
            format!("{addressee} {group} (may address {})", allowed.join(","))
        }
        Refusal::NotUrgent { group, urgent } => format!("{group} (only {})", urgent.join(",")),
        Refusal::NotPermitted { name, group, action, permitted } => {
            format!("{name} {} {group} (only {})", action_word(*action), permitted.join(","))
        }
        Refusal::TooManyAnswers { count } => count.to_string(),
        Refusal::AnswerTooLong { answer } => format!("{} bytes", answer.len()),
        Refusal::BadAnswer { answer } => format!("{answer:?}"),
        Refusal::NoSuchQuestion { group, question } => format!("{group} #{question}"),
        Refusal::NotOffered { group, question, answer, answers } => {
            format!("{group} #{question} {answer:?} (offers {})", answers.join("/"))
        }
        Refusal::NotAskedOf { name, group, question } => format!("{name} {group} #{question}"),
        Refusal::AlreadyAnswered { group, question, by, answer } => {
            format!("{group} #{question} by {by}: {answer}")
        }
        Refusal::Store { error } => error.clone(),
        Refusal::AddressedSelf
        | Refusal::NoGroup
        | Refusal::EmptyBody
        | Refusal::AnswerOffersAnswers => String::new(),
    };
    format!("refused {} {detail}", refusal.code()).trim_end().to_string()
}

fn action_word(action: Action) -> &'static str {
    match action {
        Action::Join => "join",
        Action::Leave => "leave",
        Action::Add => "add to",
        Action::Remove => "remove from",
        Action::SetPolicy => "set the policy of",
        Action::Pause => "pause",
        Action::Resume => "resume",
        Action::Delete => "delete",
        Action::Make => "make",
        Action::Strand => "strand",
    }
}

fn entry(entry: &muster_msg::Entry) -> String {
    match &entry.what {
        What::Message { author, to, body, urgent, answers, answering } => {
            let to = if to.is_empty() { String::new() } else { format!("->{}", to.join(",")) };
            let urgent = if *urgent { " (urgent)" } else { "" };
            let answering = answering.as_ref().map_or(String::new(), |answering| {
                format!(" (answering #{}: {})", answering.question, answering.answer)
            });
            let answers = if answers.is_empty() {
                String::new()
            } else {
                format!(" [{}]", answers.join("/"))
            };
            format!("#{} {author}{to}{urgent}{answering}: {body}{answers}", entry.seq)
        }
        What::Created { by } => format!("#{} created by {by}", entry.seq),
        What::Joined { who } => format!("#{} {who} joined", entry.seq),
        What::Left { who } => format!("#{} {who} left", entry.seq),
        What::Changed { by, change, .. } => {
            let change = match change {
                Change::SetPolicy => "set the policy",
                Change::Paused => "paused",
                Change::Resumed => "resumed",
            };
            format!("#{} {by} {change}", entry.seq)
        }
    }
}

fn list(entries: &[muster_msg::Entry]) -> String {
    entries.iter().map(entry).collect::<Vec<_>>().join(" | ")
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// Delivers a post's wakes the way the daemon does - to every inbox that has not died, marking
/// the others gone, and leaving a pane whose agent is still to come to ring once it is - and
/// says what the post did for each participant it was meant to wake.
fn delivered(service: &mut Messaging<Memory>, sessions: &Sessions, posted: &Posted) -> String {
    let mut failed = BTreeSet::new();
    let mut deferred = BTreeSet::new();
    for wake in &posted.wakes {
        let reached = match &wake.via {
            Via::Inbox(inbox) => sessions.answers(inbox),
            Via::Pane(pane) if sessions.doorbell(pane) == Ringable::AgentToCome => {
                deferred.insert(wake.name.clone());
                continue;
            }
            Via::Pane(pane) => sessions.agent_in(pane),
            // Told to whichever windows attend, now or later: nothing to fail.
            Via::Human => continue,
        };
        service.delivered(wake, reached).unwrap();
        if !reached {
            failed.insert(wake.name.clone());
        }
    }
    let mut parts = vec![format!("{} posted #{} to {}", posted.author, posted.seq, posted.group)];
    reached(posted, &failed, &deferred, &mut parts);
    parts.join("; ")
}

/// What a post or a resume did for each participant it was meant to wake, one part per kind.
fn reached(
    posted: &Posted,
    failed: &BTreeSet<String>,
    deferred: &BTreeSet<String>,
    parts: &mut Vec<String>,
) {
    for (label, wanted) in [
        ("woke", Reach::Woken),
        ("deferred", Reach::Deferred),
        ("already woken", Reach::AlreadyWoken),
        ("waiting", Reach::Waiting),
        ("gone", Reach::Gone),
        ("no agent", Reach::NoAgent),
        ("no doorbell", Reach::NoDoorbell),
        ("paused", Reach::Paused),
        ("unreachable", Reach::Unreachable),
    ] {
        let names: Vec<String> = posted
            .reached
            .iter()
            .filter(|(name, reach)| {
                let reach = if failed.contains(name) {
                    Reach::Gone
                } else if deferred.contains(name) {
                    Reach::Deferred
                } else {
                    *reach
                };
                reach == wanted
            })
            .map(|(name, _)| {
                let told = posted
                    .wakes
                    .iter()
                    .find(|wake| wake.name == *name && !failed.contains(name))
                    .map(|wake| (&wake.notice, rung(&wake.via)));
                let answered = posted
                    .answered
                    .iter()
                    .find(|answered| answered.name == *name)
                    .map(|answered| (&answered.notice, String::new()));
                match told.or(answered) {
                    Some((told, rung)) => format!("{name} [{}]{rung}", notice(told)),
                    None => name.clone(),
                }
            })
            .collect();
        if !names.is_empty() {
            parts.push(format!("{label} {}", names.join(", ")));
        }
    }
}

/// Delivers a resume's wakes as a post's are, and says whom it woke.
fn resumed(service: &mut Messaging<Memory>, sessions: &Sessions, posted: &Posted) -> String {
    let line = delivered(service, sessions, posted);
    let head = format!("{} posted #{} to {}", posted.author, posted.seq, posted.group);
    line.replacen(
        &head,
        &format!("{} resumed {} (#{})", posted.author, posted.group, posted.seq),
        1,
    )
}

fn changed(changed: &Changed, what: &str) -> String {
    let mut line = match changed.seq {
        Some(seq) => format!("{} {what} {} (#{seq})", changed.by, changed.group),
        None => format!("{} {what} {}, which changed nothing", changed.by, changed.group),
    };
    if !changed.added.is_empty() {
        let _ = write!(line, "; added {}", changed.added.join(","));
    }
    if !changed.removed.is_empty() {
        let _ = write!(line, "; removed {}", changed.removed.join(","));
    }
    if !changed.ended.is_empty() {
        let ended: Vec<String> = changed.ended.iter().map(ToString::to_string).collect();
        let _ = write!(line, "; ended wait {}", ended.join(","));
    }
    line
}

fn policy(step: &Value) -> Option<Policy> {
    step.get("policy").map(|policy| {
        serde_json::from_value(policy.clone()).expect("messaging.json: a policy the service reads")
    })
}

/// Opens a pane with no agent in it, `new` when one is likely starting there, or closes it.
fn pane(sessions: &Sessions, step: &Value) -> String {
    let text = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or_default();
    let pane = text("pane").to_string();
    match text("state") {
        "new" => sessions.open.borrow_mut().insert(pane.clone(), true),
        "old" => sessions.open.borrow_mut().insert(pane.clone(), false),
        // The pane itself is gone, with its agent and its name, which another may take.
        "closed" => {
            sessions.agents.borrow_mut().remove(&pane);
            sessions.names.borrow_mut().remove(&pane);
            sessions.callsigns.borrow_mut().remove(&pane);
            sessions.formerly.borrow_mut().remove(&pane);
            sessions.titles.borrow_mut().remove(&pane);
            sessions.open.borrow_mut().remove(&pane)
        }
        _ => sessions.open.borrow_mut().remove(&pane),
    };
    called(sessions, &pane, step);
    format!("{pane} {}", text("state"))
}

/// A step's `called`, what its pane goes by from then on, its `callsign`, when it has one apart
/// from that, its `formerly`, what it was called before, and its `title`, what the pane's
/// program has written to its title bar.
fn called(sessions: &Sessions, pane: &str, step: &Value) {
    if let Some(called) = step.get("called").and_then(Value::as_str) {
        sessions.names.borrow_mut().insert(pane.to_string(), called.to_string());
    }
    if let Some(callsign) = step.get("callsign").and_then(Value::as_str) {
        sessions.callsigns.borrow_mut().insert(pane.to_string(), callsign.to_string());
    }
    if let Some(formerly) = step.get("formerly").and_then(Value::as_array) {
        let formerly = formerly.iter().filter_map(Value::as_str).map(str::to_string).collect();
        sessions.formerly.borrow_mut().insert(pane.to_string(), formerly);
    }
    if let Some(title) = step.get("title").and_then(Value::as_str) {
        sessions.titles.borrow_mut().insert(pane.to_string(), title.to_string());
    }
}

/// Says what a pane's agent is doing, or with `gone` that the pane has none. `unreadable` makes
/// it an idle agent whose prompt the host cannot read.
fn agent(sessions: &Sessions, step: &Value) -> String {
    let text = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or_default();
    let pane = text("pane").to_string();
    let state = match text("state") {
        "working" => Some(Activity::Working),
        "blocked" => Some(Activity::Blocked),
        "idle" => Some(Activity::Idle),
        "waiting" => Some(Activity::Waiting),
        "stuck" => Some(Activity::Stuck),
        "unreadable" => {
            sessions.unreadable.borrow_mut().insert(pane.clone());
            Some(Activity::Idle)
        }
        _ => None,
    };
    match state {
        Some(state) => sessions.agents.borrow_mut().insert(pane.clone(), state),
        None => sessions.agents.borrow_mut().remove(&pane),
    };
    called(sessions, &pane, step);
    format!("{pane} {}", text("state"))
}

/// Tells the service `name`'s agent went idle, and says whom that woke again.
fn idle(service: &mut Messaging<Memory>, sessions: &Sessions, name: &str, now: u64) -> String {
    let (wakes, _) = service.went_idle(name, sessions, now);
    let told: Vec<String> = wakes
        .iter()
        .map(|wake| format!("{} [{}]{}", wake.name, notice(&wake.notice), rung(&wake.via)))
        .collect();
    if told.is_empty() {
        "rewoke nobody".to_string()
    } else {
        format!("rewoke {}", told.join(", "))
    }
}

/// The host stops and starts again from what its store kept: the logs it appended, and the
/// state it last saved.
fn restart(service: &mut Messaging<Memory>) -> String {
    let store = service.store();
    let logs = store.logs();
    let saved = store.saved.clone().unwrap_or_default();
    let kept = Memory {
        appended: store.appended.clone(),
        saved: store.saved.clone(),
        kept: logs.clone(),
        ..Memory::default()
    };
    *service = Messaging::restore(kept, saved, logs);
    "restarted".to_string()
}

/// A window starts attending, or with `off` stops.
fn attend(sessions: &Sessions, attending: bool) -> String {
    sessions.attended.set(attending);
    if attending { "attended" } else { "unattended" }.to_string()
}

/// The messages listed as waiting for the human in a group are the ones its notice counts, so a
/// window's notice and an API client's list of what waits never disagree.
fn agrees_with_what_waits(service: &Messaging<Memory>, notice: &Notice) {
    let waiting = service.human_waiting(&notice.group);
    let count = |pick: fn(&muster_msg::Waiting) -> bool| {
        waiting.iter().filter(|each| pick(each)).count() as u64
    };
    let listed = (
        waiting.len() as u64,
        waiting.first().map_or(0, |first| first.seq),
        waiting.last().map_or(0, |last| last.seq),
        count(|each| each.addressed),
        count(|each| each.urgent),
    );
    let counted = (notice.count, notice.first, notice.last, notice.to_you, notice.urgent);
    assert_eq!(listed, counted, "in {}: listed {waiting:?}, counted {notice:?}", notice.group);
}

/// What the windows would be told waits for the human: the groups where anything does, of
/// those the human is in.
fn waits_for_the_human(service: &Messaging<Memory>) -> String {
    let mut notices = service.human_notices();
    for notice in &notices {
        agrees_with_what_waits(service, notice);
    }
    notices.retain(|notice| notice.count > 0);
    if notices.is_empty() {
        return "nothing waits for @human".to_string();
    }
    format!("@human: {}", notices.iter().map(notice).collect::<Vec<_>>().join("; "))
}

/// A step that makes or changes a group, or lists them.
/// A page of a group's log: after `since`, below `before`, and only the `newest` so many,
/// marked when entries it matched come before it.
fn log_step(service: &Messaging<Memory>, step: &Value) -> Result<String, Refusal> {
    let number = |key: &str| step.get(key).and_then(Value::as_u64);
    let page = Page {
        since: number("since").unwrap_or(0),
        before: number("before"),
        newest: number("newest").map(|newest| u32::try_from(newest).unwrap()),
    };
    let group = step.get("group").and_then(Value::as_str).unwrap_or_default();
    service.log_page(group, &page).map(|paged| {
        let earlier = if paged.earlier { "(earlier) " } else { "" };
        format!("{earlier}{}", list(&paged.entries))
    })
}

fn group_step(
    service: &mut Messaging<Memory>,
    sessions: &Sessions,
    step: &Value,
    who: &Caller,
    now: u64,
) -> Result<String, Refusal> {
    let text = |key: &str| step.get(key).and_then(Value::as_str);
    let group = text("group").unwrap_or_default();
    match text("do").unwrap_or_default() {
        "group new" => service
            .group_new(who, group, policy(step), sessions, now)
            .map(|made| changed(&made, "made")),
        "group set" => service
            .group_set(who, group, policy(step).unwrap_or_default(), sessions, now)
            .map(|set| changed(&set, "set the policy of")),
        "members" => service
            .group_members(
                who,
                group,
                &strings(step.get("add")),
                &strings(step.get("remove")),
                &[],
                sessions,
                now,
            )
            .map(|members| changed(&members, "changed the members of")),
        "pause" => {
            service.pause(who, group, sessions, now).map(|paused| changed(&paused, "paused"))
        }
        "resume" => service
            .resume(who, group, sessions, now)
            .map(|posted| resumed(service, sessions, &posted)),
        "group delete" => service.group_delete(who, group, sessions).map(|deleted| {
            let ended: Vec<String> =
                deleted.ended.iter().map(|ticket| format!(", ended wait {ticket}")).collect();
            let ended = ended.concat();
            format!(
                "{} deleted {} ({} entries); let go {}{ended}",
                deleted.by,
                deleted.group,
                deleted.entries,
                deleted.let_go.join(",")
            )
        }),
        "groups" => Ok(service
            .groups()
            .iter()
            .map(|group| {
                let paused = if group.policy.paused { " paused" } else { "" };
                format!("{} [{}]{paused}", group.name, group.members.join(","))
            })
            .collect::<Vec<_>>()
            .join(", ")),
        other => unreachable!("{other} is not a group step"),
    }
}

fn joined_text(joined: &muster_msg::Joined) -> String {
    let mut line = match &joined.group {
        Some(group) => format!("{} joined {group}", joined.name),
        None => format!("{} registered", joined.name),
    };
    if joined.created {
        line.push_str(" (created)");
    }
    let others: Vec<&str> = joined
        .groups
        .iter()
        .filter(|group| Some(*group) != joined.group.as_ref())
        .map(String::as_str)
        .collect();
    match (joined.took_over, others.as_slice()) {
        (false, _) => {}
        (true, []) => line.push_str(" (took over)"),
        (true, others) => {
            let _ = write!(line, " (took over; in {})", others.join(","));
        }
    }
    line
}

fn left_text(left: &muster_msg::Left) -> String {
    let line = if left.stopped && left.groups.is_empty() {
        format!("{} stopped", left.name)
    } else if left.stopped {
        format!("{} stopped (left {})", left.name, left.groups.join(","))
    } else {
        format!("{} left {}", left.name, left.groups.join(","))
    };
    match left.ended {
        Some(ticket) => format!("{line}, ended wait {ticket}"),
        None => line,
    }
}

fn waited_text(waited: &muster_msg::Waited) -> String {
    match waited {
        muster_msg::Waited::Ready(notices) => {
            format!("ready {}", notices.iter().map(notice).collect::<Vec<_>>().join("; "))
        }
        muster_msg::Waited::Waiting { ticket, superseded: None } => format!("waiting {ticket}"),
        muster_msg::Waited::Waiting { ticket, superseded: Some(older) } => {
            format!("waiting {ticket}, ended {older}")
        }
    }
}

/// Runs one step and says what came of it in one line.
fn step(service: &mut Messaging<Memory>, sessions: &Sessions, step: &Value, now: u64) -> String {
    let text = |key: &str| step.get(key).and_then(Value::as_str);
    // A step may say when it happens, to put minutes between two; otherwise steps are a
    // millisecond apart.
    let now = step.get("at_ms").and_then(Value::as_u64).unwrap_or(now);
    let mut who = caller(step);
    who.at_ms = now;
    let action = text("do").unwrap_or_default();
    let result = match action {
        "join" => service
            .join(&who, text("name"), text("group"), sessions, now)
            .and_then(|joined| {
                if step.get("pull").and_then(Value::as_bool) == Some(true) {
                    service.pulls(&joined.name)?;
                }
                Ok(joined)
            })
            .map(|joined| joined_text(&joined)),
        "leave" => service.leave(&who, text("group"), sessions, now).map(|left| left_text(&left)),
        "post" => {
            let to = strings(step.get("to"));
            let body = text("body").unwrap_or_default();
            let urgent = step.get("urgent").and_then(Value::as_bool).unwrap_or(false);
            let answers = strings(step.get("answers"));
            let answering = step.get("answering").map(|answering| Answering {
                question: answering["question"].as_u64().unwrap(),
                answer: answering["answer"].as_str().unwrap().to_string(),
            });
            let draft = Draft {
                group: text("group"),
                to: &to,
                body,
                urgent,
                answers: &answers,
                answering: answering.as_ref(),
                ..Draft::default()
            };
            service
                .post_draft(&who, &draft, sessions, now)
                .map(|posted| delivered(service, sessions, &posted))
        }
        "read" => service.read(&who, text("group"), sessions).map(|read| {
            let groups: Vec<String> = read
                .groups
                .iter()
                .filter(|(_, entries)| !entries.is_empty())
                .map(|(group, entries)| format!("{group}: {}", list(entries)))
                .collect();
            if groups.is_empty() {
                format!("{} read nothing", read.name)
            } else {
                format!("{} read {}", read.name, groups.join("; "))
            }
        }),
        "wait" => {
            let due = step.get("due").and_then(Value::as_bool) == Some(true);
            service.wait(&who, text("group"), due, sessions).map(|waited| waited_text(&waited))
        }
        "who" => service.who(text("group"), sessions).map(|members| {
            members
                .iter()
                .map(|member| {
                    let liveness = match member.liveness {
                        Liveness::Alive => "alive",
                        Liveness::Gone => "gone",
                        Liveness::Human => "human",
                        Liveness::Unreachable => "unreachable",
                    };
                    match member.activity {
                        Some(doing) => format!("{} {liveness} ({})", member.name, activity(doing)),
                        None => format!("{} {liveness}", member.name),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "log" => log_step(service, step),
        "group new" | "group set" | "group delete" | "members" | "pause" | "resume" | "groups" => {
            group_step(service, sessions, step, &who, now)
        }
        "agent" => Ok(agent(sessions, step)),
        "pane" => Ok(pane(sessions, step)),
        "called" => {
            let called = service.called(sessions);
            let each: Vec<String> =
                called.iter().map(|(name, called)| format!("{name}={called}")).collect();
            Ok(format!("called {}", each.join(", ")))
        }
        "idle" => Ok(idle(service, sessions, text("name").unwrap_or_default(), now)),
        "attend" => Ok(attend(sessions, text("state") != Some("off"))),
        "restart" => Ok(restart(service)),
        "human" => Ok(waits_for_the_human(service)),
        "dies" => {
            sessions.dead.borrow_mut().insert(socket(text("session").unwrap_or_default()));
            Ok("died".to_string())
        }
        other => panic!("messaging.json: a step does {other:?}, which the driver does not know"),
    };
    result.unwrap_or_else(|refusal| refused(&refusal))
}

#[test]
fn messaging_conformance() {
    let corpus = Conformance::load("messaging.json");
    let ran = corpus.run(|given| {
        let mut service = Messaging::new(Memory::default());
        let sessions = Sessions::default();
        let steps = given.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        let said: Vec<Value> = steps
            .iter()
            .enumerate()
            .map(|(at, each)| json!(step(&mut service, &sessions, each, at as u64 + 1)))
            .collect();
        Ok(fields([("said", Some(Value::Array(said)))]))
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// MIP-4 section 15 holds that each failure of council v1 that policy or delivery answers is a
/// named case, so the file's `v1` rows are checked against the MIP's own table: a row added
/// there, or a case renamed here, fails until somebody says what answers it.
#[test]
fn every_failure_of_council_v1_is_answered_by_a_case_or_says_where_it_is() {
    let root = conformance::repo_root();
    let corpus: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("corpus/conformance/messaging.json")).unwrap(),
    )
    .unwrap();
    let mip = std::fs::read_to_string(root.join("docs/mip/0004-agent-messaging.md")).unwrap();
    let table: Vec<&str> = mip
        .lines()
        .skip_while(|line| !line.starts_with("| Observed in v1 |"))
        .skip(2)
        .take_while(|line| line.starts_with('|'))
        .filter_map(|line| line.split(" | ").next())
        .map(|cell| cell.trim_start_matches("| "))
        .collect();
    assert!(table.len() > 10, "found no table of what council v1 got wrong in MIP-4");

    let cases: BTreeSet<&str> = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|case| case["name"].as_str())
        .collect();
    let rows = corpus["v1"]["rows"].as_array().expect("messaging.json has no v1 rows");
    let mut listed = Vec::new();
    for row in rows {
        let failure = row["failure"].as_str().unwrap();
        listed.push(failure);
        match (row["case"].as_str(), row["elsewhere"].as_str()) {
            (Some(case), None) => assert!(
                cases.contains(case),
                "v1 row {failure:?} names the case {case:?}, which messaging.json does not have"
            ),
            (None, Some(_)) => {}
            _ => panic!("v1 row {failure:?} must name one `case` or say `elsewhere`, not both"),
        }
    }
    assert_eq!(
        listed, table,
        "messaging.json's v1 rows and MIP-4's table of what council v1 got wrong disagree"
    );
}
