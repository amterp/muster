//! A group shared by two machines (MIP-4, section 11): two services, a laptop's and a devenv's,
//! joined by a wire that does what their daemons do with each call - send it, and settle the
//! answer - and that can be cut.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use muster_msg::{
    Action, Call, Caller, HUMAN, Inbox, Liveness, Memory, Messaging, Participant, Peer, Policy,
    Posted, Presence, Reach, Refusal, Reply, Route, Settled, Tell, Via, Wake, What,
};

#[derive(Default)]
struct Sessions {
    dead: RefCell<BTreeSet<String>>,
    /// A window attends the laptop's daemon, which is what wakes the human there. The devenv's
    /// has no human of its own in these cases, so one flag serves both.
    attended: Cell<bool>,
    /// Panes an agent was found in, on either machine.
    agents: RefCell<BTreeSet<String>>,
}

impl Presence for Sessions {
    fn alive(&self, participant: &Participant) -> bool {
        participant.inbox.as_ref().is_none_or(|inbox| !self.dead.borrow().contains(&inbox.socket))
    }

    fn attended(&self) -> bool {
        self.attended.get()
    }

    fn agent_in(&self, pane: &str) -> bool {
        self.agents.borrow().contains(pane)
    }
}

fn session(name: &str) -> Caller {
    Caller {
        inbox: Some(Inbox { socket: format!("/tmp/cc-socks/{name}.sock"), inode: 1 }),
        ..Caller::default()
    }
}

/// An agent the daemon found in `pane`, which it rings there.
fn in_pane(pane: &str) -> Caller {
    Caller { pane: Some(pane.to_string()), ..Caller::default() }
}

/// A person's shell: no agent's address, so the human.
fn human() -> Caller {
    Caller::default()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Laptop,
    Devenv,
}

impl Side {
    fn other(self) -> Side {
        match self {
            Side::Laptop => Side::Devenv,
            Side::Devenv => Side::Laptop,
        }
    }

    /// The other machine, as this one's link knows it: the laptop calls the devenv by its
    /// window's name for it, and the devenv calls the laptop by the host name it was told.
    fn peer(self) -> Peer {
        match self {
            Side::Laptop => Peer { name: "devenv".to_string(), calls_us: "lap".to_string() },
            Side::Devenv => Peer { name: "lap".to_string(), calls_us: "devenv".to_string() },
        }
    }
}

struct Wire {
    laptop: Messaging<Memory>,
    devenv: Messaging<Memory>,
    sessions: Sessions,
    up: bool,
    /// Entries a home sends on to the other machine are lost, as they are when the link drops
    /// between an append and the send.
    losing: bool,
    now: u64,
    /// Every wake either machine made, for its daemon to deliver.
    wakes: Vec<(Side, Wake)>,
}

impl Wire {
    fn new() -> Wire {
        let mut wire = Wire {
            laptop: Messaging::new(Memory::default()),
            devenv: Messaging::new(Memory::default()),
            sessions: Sessions::default(),
            up: false,
            losing: false,
            now: 0,
            wakes: Vec::new(),
        };
        wire.mend();
        wire
    }

    fn service(&mut self, side: Side) -> &mut Messaging<Memory> {
        match side {
            Side::Laptop => &mut self.laptop,
            Side::Devenv => &mut self.devenv,
        }
    }

    fn tick(&mut self) -> u64 {
        self.now += 1;
        self.now
    }

    fn cut(&mut self) {
        self.up = false;
        self.laptop.unlinked("devenv");
        self.devenv.unlinked("lap");
    }

    /// The link comes up, and each side refetches the replicas it holds, as a daemon does.
    fn mend(&mut self) {
        self.up = true;
        self.laptop.linked("devenv");
        self.devenv.linked("lap");
        for side in [Side::Laptop, Side::Devenv] {
            let machine = side.peer().name;
            for (group, head) in self.service(side).replicas_of(&machine) {
                let call = Call::Since { group, after: head };
                self.send(side, &call).expect("the link is up");
            }
        }
    }

    /// Sends `call` from `side` to the other machine and settles the answer there.
    fn send(&mut self, side: Side, call: &Call) -> Result<Settled, Refusal> {
        assert!(self.up, "a daemon never calls over a link that is down");
        let now = self.tick();
        let from = side.other().peer();
        let answered = {
            let (home, sessions) = self.split(side.other());
            home.answer(&from, call.clone(), sessions, now)
        };
        self.wakes.extend(answered.wakes.into_iter().map(|wake| (side.other(), wake)));
        self.tell(side.other(), &answered.tell);
        let (asker, sessions) = self.split(side);
        let settle = asker.settle(&side.peer(), call, answered.reply, sessions, now);
        self.wakes.extend(settle.applied.wakes.into_iter().map(|wake| (side, wake)));
        settle.result
    }

    /// Sends a home's new entries on to the machines it named.
    fn tell(&mut self, home: Side, tell: &[Tell]) -> Vec<(String, Reach)> {
        let mut reached = Vec::new();
        if self.losing {
            return reached;
        }
        for tell in tell {
            let caught = self.service(home).since(&tell.group, tell.after).unwrap();
            let now = self.tick();
            let (replica, sessions) = self.split(home.other());
            let applied =
                replica.apply(&home.other().peer(), caught, sessions, now).expect("no gap");
            let peer = home.peer();
            reached.extend(applied.reached.into_iter().map(|(name, r)| (peer.inward(&name), r)));
            self.wakes.extend(applied.wakes.into_iter().map(|wake| (home.other(), wake)));
        }
        reached
    }

    fn split(&mut self, side: Side) -> (&mut Messaging<Memory>, &Sessions) {
        match side {
            Side::Laptop => (&mut self.laptop, &self.sessions),
            Side::Devenv => (&mut self.devenv, &self.sessions),
        }
    }

    fn join(&mut self, side: Side, caller: &Caller, name: Option<&str>, group: &str) -> String {
        let now = self.tick();
        let (service, sessions) = self.split(side);
        let mut route = service.route_join(caller, name, Some(group), sessions).unwrap();
        if let Route::Ask { group } = route {
            let found = self.send(side, &Call::Find { group: group.clone() }).unwrap();
            route = if found == Settled::Found(true) {
                let there = format!("{group}@{}", side.peer().name);
                let (service, sessions) = self.split(side);
                service.route_join(caller, name, Some(&there), sessions).unwrap()
            } else {
                Route::Here
            };
        }
        let (service, sessions) = self.split(side);
        match route {
            Route::Away(away) => match self.send(side, &away.call).unwrap() {
                Settled::Joined(joined) => joined.group.unwrap(),
                other => panic!("a join settles as joined: {other:?}"),
            },
            Route::Here => {
                let joined = service.join(caller, name, Some(group), sessions, now).unwrap();
                self.tell(side, &joined.tell);
                joined.group.unwrap()
            }
            Route::Ask { .. } => panic!("asked twice"),
        }
    }

    fn post(
        &mut self,
        side: Side,
        caller: &Caller,
        group: Option<&str>,
        to: &[&str],
        body: &str,
    ) -> Result<Posted, Refusal> {
        let now = self.tick();
        let to: Vec<String> = to.iter().map(|name| (*name).to_string()).collect();
        let (service, sessions) = self.split(side);
        match service.route_post(caller, group, &to, body, sessions)? {
            Route::Away(away) => match self.send(side, &away.call)? {
                Settled::Posted(posted) => Ok(posted),
                other => panic!("a post settles as posted: {other:?}"),
            },
            Route::Here => {
                let mut posted = service.post(caller, group, &to, body, sessions, now)?;
                self.wakes.extend(posted.wakes.iter().map(|wake| (side, wake.clone())));
                let reached = self.tell(side, &posted.tell);
                posted.reached.extend(reached);
                Ok(posted)
            }
            Route::Ask { .. } => panic!("a post never asks"),
        }
    }

    fn read(&mut self, side: Side, caller: &Caller, group: Option<&str>) -> Vec<String> {
        let (service, sessions) = self.split(side);
        let read = service.read(caller, group, sessions).unwrap();
        read.groups
            .iter()
            .flat_map(|(group, entries)| {
                entries.iter().filter_map(move |entry| match &entry.what {
                    What::Message { author, body, .. } => Some(format!("{group} {author}: {body}")),
                    _ => None,
                })
            })
            .collect()
    }

    fn who(&mut self, side: Side, group: &str) -> Vec<(String, Liveness)> {
        let (service, sessions) = self.split(side);
        let mut members = service.who(Some(group), sessions).unwrap();
        let away = service.route_who(Some(group)).unwrap();
        for away in away {
            if let Settled::Members(heard) = self.send(side, &away.call).unwrap() {
                muster_msg::heard(&mut members, &away.machine, &heard);
            }
        }
        members.into_iter().map(|member| (member.name, member.liveness)).collect()
    }
}

fn woke(posted: &Posted) -> Vec<(&str, Reach)> {
    posted.reached.iter().map(|(name, reach)| (name.as_str(), *reach)).collect()
}

fn woken_names(posted: &Posted) -> Vec<&str> {
    posted.wakes.iter().map(|wake| wake.name.as_str()).collect()
}

/// A devenv agent joins a group made on the laptop by name alone, and each wakes the other.
#[test]
fn a_devenv_agent_joins_a_laptop_group_and_they_wake_each_other() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    assert_eq!(wire.join(Side::Laptop, &builder, Some("builder"), "review"), "review");
    assert_eq!(wire.join(Side::Devenv, &critic, Some("critic"), "review"), "review@lap");

    let posted = wire.post(Side::Devenv, &critic, None, &[], "the parser is done").unwrap();
    assert_eq!(posted.group, "review@lap");
    assert_eq!(woke(&posted), [("builder@lap", Reach::Woken)]);
    assert_eq!(
        wire.read(Side::Laptop, &builder, None),
        ["review critic@devenv: the parser is done"]
    );

    let answered = wire.post(Side::Laptop, &builder, None, &["critic"], "thanks").unwrap();
    assert_eq!(woke(&answered), [("critic@devenv", Reach::Woken)]);
    assert_eq!(wire.read(Side::Devenv, &critic, None), ["review@lap builder@lap: thanks"]);
}

/// The wake for a devenv member is made on the devenv, where its inbox is, and the laptop's
/// own wakes are the laptop's.
#[test]
fn each_machine_wakes_only_its_own_members() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    let posted = wire.post(Side::Laptop, &builder, Some("review"), &[], "one").unwrap();
    assert!(woken_names(&posted).is_empty(), "the laptop has nobody to wake: {posted:?}");
    let (devenv, sessions) = wire.split(Side::Devenv);
    let waited = devenv.wait(&critic, None, false, sessions).unwrap();
    assert!(matches!(waited, muster_msg::Waited::Ready(_)), "{waited:?}");
}

/// Each side names the other's members with the machine they are on.
#[test]
fn each_machine_names_the_other_from_where_it_stands() {
    let mut wire = Wire::new();
    wire.join(Side::Laptop, &session("builder"), Some("builder"), "review");
    wire.join(Side::Devenv, &session("critic"), Some("critic"), "review");

    let laptop = wire.who(Side::Laptop, "review");
    assert_eq!(
        laptop,
        [("builder".to_string(), Liveness::Alive), ("critic@devenv".to_string(), Liveness::Alive)]
    );
    let devenv = wire.who(Side::Devenv, "review");
    assert_eq!(
        devenv,
        [("builder@lap".to_string(), Liveness::Alive), ("critic".to_string(), Liveness::Alive)]
    );
}

/// The guard counts from the author's cursor at the group's home. A replica that missed an
/// entry lets the post through to the home, which refuses it and sends what was missed with
/// the refusal; once read, the post goes through.
#[test]
fn the_guard_holds_across_the_link() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    wire.losing = true;
    wire.post(Side::Laptop, &builder, None, &[], "rebase first").unwrap();
    wire.losing = false;

    let refused = wire.post(Side::Devenv, &critic, None, &[], "pushed");
    assert_eq!(refused.unwrap_err(), Refusal::Unread { group: "review@lap".to_string(), count: 1 });
    assert_eq!(wire.read(Side::Devenv, &critic, None), ["review@lap builder@lap: rebase first"]);
    wire.post(Side::Devenv, &critic, None, &[], "rebased, then pushed").unwrap();
}

/// While the link is down a post to a group kept on the other machine fails at once, naming
/// it, and a group kept here posts as ever.
#[test]
fn a_post_to_a_group_on_an_unreachable_machine_fails_at_once() {
    let mut wire = Wire::new();
    let (builder, critic, scout) = (session("builder"), session("critic"), session("scout"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "local");
    wire.join(Side::Devenv, &scout, Some("scout"), "local");

    wire.cut();
    let refused = wire.post(Side::Devenv, &critic, Some("review"), &[], "anyone?");
    assert_eq!(
        refused.unwrap_err(),
        Refusal::Unreachable { group: "review@lap".to_string(), machine: "lap".to_string() }
    );
    let posted = wire.post(Side::Devenv, &critic, Some("local"), &[], "still here").unwrap();
    assert_eq!(woke(&posted), [("scout", Reach::Woken)]);
    assert_eq!(wire.devenv.behind("review@lap"), Some("lap"));
}

/// What was posted while the link was down reaches the replica when it comes back, and wakes
/// its member once.
#[test]
fn a_replica_catches_up_when_the_link_returns() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    wire.cut();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let posted = laptop.post(&builder, None, &[], "while you were away", sessions, 90).unwrap();
    assert_eq!(posted.tell.len(), 1, "the devenv is told when the link returns: {posted:?}");
    wire.mend();

    assert_eq!(wire.devenv.behind("review@lap"), None);
    assert_eq!(
        wire.read(Side::Devenv, &critic, None),
        ["review@lap builder@lap: while you were away"]
    );
}

/// `review` is the group kept here when there is one, and `review@lap` the laptop's.
#[test]
fn a_bare_group_name_is_the_one_kept_here() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review@lap");
    let (devenv, sessions) = wire.split(Side::Devenv);
    devenv.join(&critic, None, Some("review"), sessions, 50).unwrap();

    let here = wire.post(Side::Devenv, &critic, Some("review"), &[], "to the devenv's").unwrap();
    assert_eq!(here.group, "review");
    let there =
        wire.post(Side::Devenv, &critic, Some("review@lap"), &[], "to the laptop's").unwrap();
    assert_eq!(there.group, "review@lap");
    assert_eq!(wire.read(Side::Laptop, &builder, None), ["review critic@devenv: to the laptop's"]);
}

/// Someone on another machine is addressed within a group both are in; with none, the post is
/// refused rather than making a group the other never joined.
#[test]
fn a_post_to_someone_elsewhere_needs_a_group_in_common() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &session("scout"), Some("scout"), "other");

    let refused = wire.post(Side::Laptop, &session("scout"), None, &["critic@devenv"], "hi");
    assert_eq!(
        refused.unwrap_err(),
        Refusal::NoSuchParticipant { name: "critic@devenv".to_string() }
    );
    let refused = wire.post(Side::Laptop, &builder, None, &["critic", "scout"], "all of you");
    assert_eq!(refused.unwrap_err(), Refusal::NoSharedGroup { name: "critic@devenv".to_string() });
}

/// A laptop human in a group kept on the devenv is woken by the laptop, through the same
/// reach a post made on the laptop would use: the human's home is where the app runs.
#[test]
fn a_devenv_post_to_the_human_is_reached_on_the_laptop() {
    let mut wire = Wire::new();
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    assert_eq!(wire.join(Side::Laptop, &human(), None, "review"), "review@devenv");

    let posted = wire.post(Side::Devenv, &critic, None, &[HUMAN], "a question for you").unwrap();
    assert_eq!(woke(&posted), [("@human@lap", Reach::Waiting)]);
    let (laptop, sessions) = wire.split(Side::Laptop);
    let waited = laptop.wait(&human(), None, false, sessions).unwrap();
    let muster_msg::Waited::Ready(notices) = waited else { panic!("the human has one unread") };
    assert_eq!(notices[0].group, "review@devenv");
    assert_eq!(notices[0].to_you, 1);
}

/// A replica is not kept, but the cursors on it are: after a restart the refetch counts only
/// what the participant had not read.
#[test]
fn a_cursor_on_a_group_kept_elsewhere_survives_a_restart() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.post(Side::Laptop, &builder, None, &[], "read this").unwrap();
    assert_eq!(wire.read(Side::Devenv, &critic, None).len(), 1);

    let saved = wire.devenv.store().saved.clone().unwrap();
    wire.devenv = Messaging::restore(Memory::default(), saved, BTreeMap::default());
    wire.cut();
    wire.mend();
    assert!(wire.read(Side::Devenv, &critic, None).is_empty(), "read before the restart");
    wire.post(Side::Devenv, &critic, None, &[], "nothing unread, so this goes").unwrap();
}

/// A replica is not kept, so a daemon that starts holds an empty one for each group kept
/// elsewhere that a cursor names. An agent in a pane woken for it and not yet reading neither
/// stops the daemon starting nor goes unwoken, and the group answers, empty and behind, until
/// the link returns: a bare join finds it rather than making a group of the same name here.
#[test]
fn a_group_kept_elsewhere_is_held_from_the_start_after_a_restart() {
    let mut wire = Wire::new();
    wire.sessions.agents.borrow_mut().insert("p1".to_string());
    let (builder, critic) = (in_pane("p1"), session("critic"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    assert_eq!(wire.join(Side::Laptop, &builder, Some("builder"), "review"), "review@devenv");
    let posted = wire.post(Side::Devenv, &critic, None, &[], "unread at the restart").unwrap();
    assert_eq!(woke(&posted), [("builder@lap", Reach::Woken)]);

    let saved = wire.laptop.store().saved.clone().unwrap();
    wire.laptop = Messaging::restore(Memory::default(), saved, BTreeMap::default());
    wire.cut();
    assert!(wire.laptop.outstanding().is_empty(), "nothing is rung before the refetch");
    let (laptop, sessions) = wire.split(Side::Laptop);
    assert!(laptop.went_idle("builder", sessions, 99).0.is_empty());
    assert_eq!(laptop.log("review@devenv", 0), Ok(Vec::new()));
    assert_eq!(laptop.behind("review"), Some("devenv"));
    assert_eq!(
        laptop.route_join(&builder, Some("builder"), Some("review"), sessions),
        Err(Refusal::Unreachable {
            group: "review@devenv".to_string(),
            machine: "devenv".to_string()
        })
    );

    wire.wakes.clear();
    wire.mend();
    let rung: Vec<u64> = wire
        .wakes
        .iter()
        .filter(|(side, wake)| *side == Side::Laptop && wake.name == "builder")
        .map(|(_, wake)| wake.notice.count)
        .collect();
    assert_eq!(rung, [1], "woken once, for what it had not read");
    assert_eq!(
        wire.read(Side::Laptop, &builder, None),
        ["review@devenv critic@devenv: unread at the restart"]
    );
}

/// The same entries twice change nothing; entries after a gap are refused with the head the
/// gap follows.
#[test]
fn applying_skips_what_it_has_and_reports_a_gap() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.post(Side::Laptop, &builder, None, &[], "one").unwrap();

    let whole = wire.laptop.since("review", 0).unwrap();
    let (devenv, sessions) = wire.split(Side::Devenv);
    let again = devenv.apply(&Side::Devenv.peer(), whole.clone(), sessions, 70).unwrap();
    assert!(again.reached.is_empty(), "{again:?}");

    let head = whole.entries.last().unwrap().seq;
    let mut ahead = whole;
    ahead.entries.iter_mut().for_each(|entry| entry.seq += head + 1);
    assert_eq!(devenv.apply(&Side::Devenv.peer(), ahead, sessions, 71), Err(head));
}

#[test]
fn a_name_crossing_the_link_is_turned_into_the_receivers_own() {
    let devenv_sees_laptop = Side::Devenv.peer();
    assert_eq!(devenv_sees_laptop.inward("builder"), "builder@lap");
    assert_eq!(devenv_sees_laptop.inward("critic@devenv"), "critic");
    assert_eq!(devenv_sees_laptop.inward("@human"), "@human@lap");
    assert_eq!(devenv_sees_laptop.inward("scout@other"), "scout@other");
    let _ = Reply::Found(true);
}

/// Whom the human's windows were told of, on each machine.
fn told_the_human(wire: &Wire) -> Vec<(Side, &str)> {
    wire.wakes
        .iter()
        .filter(|(_, wake)| wake.via == Via::Human)
        .map(|(side, wake)| (*side, wake.notice.group.as_str()))
        .collect()
}

/// A directed council on the laptop: members may address the director and the human only.
fn directed(membership: &[&str]) -> Policy {
    let set = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
    Policy {
        ring: BTreeMap::from([
            ("director".to_string(), set(&["*"])),
            ("*".to_string(), set(&["director"])),
        ]),
        allow: BTreeMap::from([
            ("director".to_string(), set(&["*"])),
            ("*".to_string(), set(&["director", HUMAN])),
        ]),
        membership: set(membership),
        paused: false,
    }
}

/// A post forwarded to the group's home runs through the home's policy, as one made there
/// does, and its refusal comes back in the author's names.
#[test]
fn a_forwarded_post_is_held_to_the_homes_policy() {
    let mut wire = Wire::new();
    let (director, builder, critic) = (session("director"), session("builder"), session("critic"));
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.join(&director, Some("director"), None, sessions, now).unwrap();
    laptop.group_new(&director, "council", Some(directed(&["*"])), sessions, now).unwrap();
    wire.join(Side::Laptop, &builder, Some("builder"), "council");
    assert_eq!(wire.join(Side::Devenv, &critic, Some("critic"), "council"), "council@lap");

    let refused = wire.post(Side::Devenv, &critic, None, &["builder"], "over the director");
    assert_eq!(
        refused.unwrap_err(),
        Refusal::NotAllowed {
            addressee: "builder@lap".to_string(),
            group: "council@lap".to_string(),
            allowed: vec!["director@lap".to_string(), HUMAN.to_string()],
        }
    );
    let posted = wire.post(Side::Devenv, &critic, None, &[], "done").unwrap();
    assert_eq!(woke(&posted), [("director@lap", Reach::Woken)], "the ring set binds too");
}

/// A group's membership rule binds a join forwarded from another machine.
#[test]
fn a_forwarded_join_is_held_to_the_homes_membership() {
    let mut wire = Wire::new();
    let director = session("director");
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.join(&director, Some("director"), None, sessions, now).unwrap();
    laptop.group_new(&director, "council", Some(directed(&["director"])), sessions, now).unwrap();

    let (devenv, sessions) = wire.split(Side::Devenv);
    let route =
        devenv.route_join(&session("critic"), Some("critic"), Some("council@lap"), sessions);
    let Route::Away(away) = route.unwrap() else { panic!("council is kept on the laptop") };
    assert_eq!(
        wire.send(Side::Devenv, &away.call).unwrap_err(),
        Refusal::NotPermitted {
            name: "critic".to_string(),
            group: "council@lap".to_string(),
            action: Action::Join,
            permitted: vec!["director@lap".to_string()],
        }
    );
}

/// A paused group wakes nobody on either machine, and resuming it wakes each machine's own
/// members for what they have unread.
#[test]
fn a_pause_at_the_home_holds_on_every_machine_until_it_is_resumed() {
    let mut wire = Wire::new();
    let (builder, critic, scout) = (session("builder"), session("critic"), session("scout"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Devenv, &scout, Some("scout"), "review");
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let paused = laptop.pause(&builder, "review", sessions, now).unwrap();
    wire.tell(Side::Laptop, &paused.tell);

    let posted = wire.post(Side::Devenv, &critic, None, &[], "while paused").unwrap();
    assert_eq!(woke(&posted), [("builder@lap", Reach::Paused), ("scout", Reach::Paused)]);
    wire.read(Side::Laptop, &builder, None);
    let posted = wire.post(Side::Laptop, &builder, None, &["scout"], "and this").unwrap();
    assert_eq!(woke(&posted), [("scout@devenv", Reach::Paused)]);

    let (devenv, sessions) = wire.split(Side::Devenv);
    let refused = devenv.pause(&critic, "review@lap", sessions, 99);
    assert_eq!(
        refused.unwrap_err(),
        Refusal::KeptElsewhere { group: "review@lap".to_string(), machine: "lap".to_string() }
    );

    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let resumed = laptop.resume(&builder, "review", sessions, now).unwrap();
    assert!(woke(&resumed).is_empty(), "the laptop's only member resumed it: {resumed:?}");
    let reached = wire.tell(Side::Laptop, &resumed.tell);
    assert_eq!(reached, [("scout@devenv".to_string(), Reach::Woken)]);
}

/// The human is homed where the app runs. A post made on the devenv, in a group kept there,
/// wakes the laptop's human through the laptop's windows, as a post made on the laptop does:
/// addressed, or rung by the default policy, which rings the human wherever it is homed.
#[test]
fn a_devenv_post_tells_the_laptops_windows_of_the_human() {
    let mut wire = Wire::new();
    wire.sessions.attended.set(true);
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &human(), None, "review");

    let posted = wire.post(Side::Devenv, &critic, None, &[HUMAN], "a question for you").unwrap();
    assert_eq!(woke(&posted), [("@human@lap", Reach::Woken)]);
    assert_eq!(told_the_human(&wire), [(Side::Laptop, "review@devenv")]);

    wire.read(Side::Laptop, &human(), None);
    wire.wakes.clear();
    let posted = wire.post(Side::Devenv, &critic, None, &[], "and to everyone").unwrap();
    assert_eq!(woke(&posted), [("@human@lap", Reach::Woken)]);
    assert_eq!(told_the_human(&wire), [(Side::Laptop, "review@devenv")]);
}

/// A devenv member's post to the human in a group kept on the laptop is appended there, and
/// the laptop tells its windows; the devenv, which is not the human's home, tells nobody.
#[test]
fn a_devenv_post_in_a_laptop_group_tells_the_laptops_windows() {
    let mut wire = Wire::new();
    wire.sessions.attended.set(true);
    let critic = session("critic");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    let posted = wire.post(Side::Devenv, &critic, None, &[HUMAN], "a question for you").unwrap();
    assert_eq!(woke(&posted), [("@human@lap", Reach::Woken)]);
    assert_eq!(told_the_human(&wire), [(Side::Laptop, "review")]);
}

/// The guard does not hold the human's post, on the laptop or at a home elsewhere: a person
/// reads the transcript as it arrives.
#[test]
fn the_human_is_exempt_from_the_guard_across_the_link() {
    let mut wire = Wire::new();
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.post(Side::Devenv, &critic, None, &[], "unread by the human").unwrap();

    let posted = wire.post(Side::Laptop, &human(), None, &["critic"], "go ahead").unwrap();
    assert_eq!(posted.group, "review@devenv");
    assert_eq!(woke(&posted), [("critic@devenv", Reach::Woken)]);
}

/// Refetching a replica from nothing, after a restart, replays its log. A member who left is not
/// reached for what came while it was a member, and the human who left is told nothing waits,
/// so a banner from before the restart comes down rather than going up again.
#[test]
fn a_replayed_replica_reaches_nobody_who_left() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.post(Side::Devenv, &critic, None, &[], "for everyone").unwrap();
    wire.post(Side::Devenv, &critic, None, &[HUMAN], "a question for you").unwrap();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let away = laptop.route_leave(&human(), Some("review"), sessions).unwrap();
    for away in away {
        wire.send(Side::Laptop, &away.call).unwrap();
    }

    let saved = wire.laptop.store().saved.clone().unwrap();
    wire.laptop = Messaging::restore(Memory::default(), saved, BTreeMap::default());
    wire.cut();
    wire.wakes.clear();
    wire.mend();
    let told: Vec<(&str, u64)> = wire
        .wakes
        .iter()
        .filter(|(side, wake)| *side == Side::Laptop && wake.via == Via::Human)
        .map(|(_, wake)| (wake.notice.group.as_str(), wake.notice.count))
        .collect();
    assert_eq!(told, [("review@devenv", 0)], "the human is told nothing waits, and no more");
    // Once at most, for the whole replay: not at all while it still counts as woken from
    // before the restart.
    let builder_woken =
        wire.wakes.iter().filter(|(side, wake)| *side == Side::Laptop && wake.name == "builder");
    assert!(builder_woken.count() <= 1, "builder woken per message: {:?}", wire.wakes);
}

/// A join by a bare name that nothing here holds, while a machine this one has linked to cannot
/// be reached: that machine may keep a group by the name, so a new one here would shadow it.
/// A machine never linked to says nothing, and a join names a group elsewhere in full to go
/// there, or `group new` makes one here.
#[test]
fn a_bare_name_join_is_not_made_a_group_here_while_a_known_machine_is_down() {
    let mut wire = Wire::new();
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.cut();

    let builder = session("builder");
    let (laptop, sessions) = wire.split(Side::Laptop);
    let route = laptop.route_join(&builder, Some("builder"), Some("review"), sessions);
    assert!(!matches!(route, Ok(Route::Here)), "would shadow review@devenv: {route:?}");
    let Err(Refusal::Unchecked { group, machines }) = route else { panic!("{route:?}") };
    assert_eq!((group.as_str(), machines), ("review", vec!["devenv".to_string()]));

    let made = laptop.group_new(&builder, "review", None, sessions, 1);
    assert!(made.is_ok(), "group new makes it here regardless: {made:?}");
}

/// A group's policy, members and pause are changed on its home. Named here by its bare name, as
/// every other verb takes it, a group kept elsewhere is refused as such, naming where.
#[test]
fn a_change_to_a_group_kept_elsewhere_by_its_bare_name_says_where_it_is_kept() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");

    let (laptop, sessions) = wire.split(Side::Laptop);
    let paused = laptop.pause(&builder, "review", sessions, 1);
    assert_eq!(
        paused.map(|_| ()),
        Err(Refusal::KeptElsewhere {
            group: "review@devenv".to_string(),
            machine: "devenv".to_string()
        })
    );
}

/// Entries that would leave a gap are not taken, and neither is the policy they came with: a
/// pause ahead of the entries that explain it would hold wakes for no reason anybody can read.
#[test]
fn a_batch_refused_for_a_gap_leaves_the_replicas_policy_alone() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    let mut ahead = wire.laptop.since("review", 0).unwrap();
    let head = ahead.entries.last().unwrap().seq;
    ahead.entries.iter_mut().for_each(|entry| entry.seq += head + 1);
    ahead.policy.paused = true;
    let (devenv, sessions) = wire.split(Side::Devenv);
    assert_eq!(devenv.apply(&Side::Devenv.peer(), ahead, sessions, 90), Err(head));
    let replica = devenv.groups().into_iter().find(|group| group.name == "review@lap").unwrap();
    assert!(!replica.policy.paused, "took the policy of a batch it refused");
}

/// What a link carries in one frame, which the reader refuses past this size and ends the link.
const FRAME: usize = 16 << 20;

/// A log bigger than a frame is fetched a page at a time: sent whole, the frame is refused, the
/// link ends, and the refetch when it comes back asks for the same again.
#[test]
fn a_log_bigger_than_a_frame_is_fetched_in_pages() {
    let mut wire = Wire::new();
    let builder = session("builder");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    let page = "x".repeat(muster_msg::LARGEST_BODY - 1024);
    for _ in 0..18 {
        wire.post(Side::Laptop, &builder, None, &[], &page).unwrap();
    }

    let caught = wire.laptop.since("review", 0).unwrap();
    let bytes: usize = caught
        .entries
        .iter()
        .map(|entry| match &entry.what {
            What::Message { body, .. } => body.len(),
            _ => 0,
        })
        .sum();
    assert!(bytes < FRAME, "one answer holds {bytes} bytes of bodies");

    // Asked again from each page's last entry, the pages together are the whole log, and the
    // replica they are applied to holds every message.
    let (mut after, mut pages, mut more) = (0, 0, true);
    while more {
        let page = wire.laptop.since("review", after).unwrap();
        let (devenv, sessions) = wire.split(Side::Devenv);
        let applied = devenv.apply(&Side::Devenv.peer(), page.clone(), sessions, 99).unwrap();
        after = page.entries.last().unwrap().seq;
        assert_eq!(applied.more, page.more.then_some(after));
        more = page.more;
        pages += 1;
    }
    assert!(pages > 1, "one page for the whole log");
    let replica = wire.devenv.log("review@lap", 0).unwrap();
    let messages = replica.iter().filter(|entry| matches!(entry.what, What::Message { .. }));
    assert_eq!(messages.count(), 18);
}
