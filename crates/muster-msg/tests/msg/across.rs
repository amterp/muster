//! A group shared by two machines (MIP-4, section 11): two services, a laptop's and a devenv's,
//! joined by a wire that does what their daemons do with each call - send it, and settle the
//! answer - and that can be cut.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use muster_msg::{
    Action, Call, Caller, Caught, Draft, Entry, Found, HUMAN, Inbox, Liveness, Member, Memory,
    Messaging, Participant, Peer, Policy, Posted, Presence, Reach, Refusal, Reply, Route, Settled,
    Tell, Via, Wake, What,
};

#[derive(Default)]
struct Sessions {
    dead: RefCell<BTreeSet<String>>,
    /// A window attends the laptop's daemon, which is what wakes the human there. The devenv's
    /// has no human of its own in these cases, so one flag serves both.
    attended: Cell<bool>,
    /// Panes an agent was found in, on either machine.
    agents: RefCell<BTreeSet<String>>,
    /// Panes open on each machine.
    panes: RefCell<BTreeMap<String, Side>>,
    /// The machine whose daemon is asking, which [`Wire::split`] sets.
    asking: Cell<Option<Side>>,
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

    fn has_pane(&self, pane: &str) -> bool {
        self.panes.borrow().get(pane).is_some_and(|side| Some(*side) == self.asking.get())
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
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
    /// Waits a replica's entries ended, on either machine.
    ended: Vec<(Side, u64)>,
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
            ended: Vec::new(),
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
        self.laptop.linked(&Side::Laptop.peer());
        self.devenv.linked(&Side::Devenv.peer());
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
        self.settle(side, call).0
    }

    /// [`Self::send`], with what the entries its answer carried did on `side`.
    fn settle(
        &mut self,
        side: Side,
        call: &Call,
    ) -> (Result<Settled, Refusal>, muster_msg::Applied) {
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
        let mut applied = settle.applied;
        self.wakes.extend(applied.wakes.drain(..).map(|wake| (side, wake)));
        self.ended.extend(applied.ended.drain(..).map(|ticket| (side, ticket)));
        (settle.result, applied)
    }

    /// Sends a home's new entries on to the machines it named. A machine that holds too little
    /// of the group to take them - none of it, when its first member was just added - fetches
    /// what it lacks, as its daemon does.
    fn tell(&mut self, home: Side, tell: &[Tell]) -> Vec<(String, Reach)> {
        let mut reached = Vec::new();
        if self.losing {
            return reached;
        }
        for tell in tell {
            let caught = self.service(home).since(&tell.group, tell.after).unwrap();
            let now = self.tick();
            let (replica, sessions) = self.split(home.other());
            let applied = match replica.apply(&home.other().peer(), caught, sessions, now) {
                Ok(applied) => applied,
                Err(head) => {
                    let call = Call::Since { group: tell.group.clone(), after: head };
                    let (_, applied) = self.settle(home.other(), &call);
                    applied
                }
            };
            let peer = home.peer();
            reached.extend(applied.reached.into_iter().map(|(name, r)| (peer.inward(&name), r)));
            self.wakes.extend(applied.wakes.into_iter().map(|wake| (home.other(), wake)));
            self.ended.extend(applied.ended.into_iter().map(|ticket| (home.other(), ticket)));
        }
        reached
    }

    fn split(&mut self, side: Side) -> (&mut Messaging<Memory>, &Sessions) {
        self.sessions.asking.set(Some(side));
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
        let joined = match route {
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
        };
        self.fold(side);
        joined
    }

    /// Tells the homes of the groups a join folded a pane into a name in, as a daemon does once
    /// the join is answered.
    fn fold(&mut self, side: Side) {
        let folded = self.service(side).take_folded();
        self.tell(side, &folded.tell);
        for away in folded.away {
            self.send(side, &away.call).expect("the home takes the name in place of the pane");
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
        self.post_urgently(side, caller, group, to, body, false)
    }

    fn post_urgently(
        &mut self,
        side: Side,
        caller: &Caller,
        group: Option<&str>,
        to: &[&str],
        body: &str,
        urgent: bool,
    ) -> Result<Posted, Refusal> {
        let now = self.tick();
        let to: Vec<String> = to.iter().map(|name| (*name).to_string()).collect();
        let found = self.found(side, &to);
        let (service, sessions) = self.split(side);
        let draft = Draft { group, to: &to, found: &found, body, urgent };
        match service.route_post(caller, &draft, sessions)? {
            Route::Away(away) => match self.send(side, &away.call)? {
                Settled::Posted(posted) => Ok(posted),
                other => panic!("a post settles as posted: {other:?}"),
            },
            Route::Here => {
                let mut posted = service.post_draft(caller, &draft, sessions, now)?;
                self.wakes.extend(posted.wakes.iter().map(|wake| (side, wake.clone())));
                let reached = self.tell(side, &posted.tell);
                posted.reached.extend(reached);
                Ok(posted)
            }
            Route::Ask { .. } => panic!("a post never asks"),
        }
    }

    /// What the other machine says each of `names` means there, as a daemon asks when a name
    /// means nobody here. Asked of every name, which is the same answer for those known here.
    fn found(&mut self, side: Side, names: &[String]) -> Vec<Found> {
        if !self.up {
            return Vec::new();
        }
        let mut found = Vec::new();
        for name in names {
            let base = muster_msg::split_machine(name).map_or(name.as_str(), |(base, _)| base);
            let call = Call::Whom { name: base.to_string() };
            if call.check().is_err() {
                continue;
            }
            if let Ok(Settled::Named(Some(there))) = self.send(side, &call) {
                found.push(Found { name: name.clone(), there });
            }
        }
        found
    }

    /// Opens `pane` on `side`, with an agent in it.
    fn pane(&self, side: Side, pane: &str) {
        self.sessions.panes.borrow_mut().insert(pane.to_string(), side);
        self.sessions.agents.borrow_mut().insert(pane.to_string());
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

/// An urgent post stays urgent across the link both ways: replicated, it wakes a member on the
/// far machine woken already, and forwarded to the group's home, one there.
#[test]
fn an_urgent_post_wakes_members_woken_already_on_either_machine() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    let first = wire.post(Side::Laptop, &builder, Some("review"), &[], "one").unwrap();
    assert_eq!(woke(&first), [("critic@devenv", Reach::Woken)]);
    let second = wire.post(Side::Laptop, &builder, Some("review"), &[], "two").unwrap();
    assert_eq!(woke(&second), [("critic@devenv", Reach::AlreadyWoken)]);
    let urgent =
        wire.post_urgently(Side::Laptop, &builder, Some("review"), &[], "now", true).unwrap();
    assert_eq!(woke(&urgent), [("critic@devenv", Reach::Woken)], "replicated urgent");

    wire.read(Side::Devenv, &critic, None);
    wire.post(Side::Devenv, &critic, None, &[], "three").unwrap();
    let again = wire.post(Side::Devenv, &critic, None, &[], "four").unwrap();
    assert_eq!(woke(&again), [("builder@lap", Reach::AlreadyWoken)]);
    let urgent = wire.post_urgently(Side::Devenv, &critic, None, &[], "now", true).unwrap();
    assert_eq!(woke(&urgent), [("builder@lap", Reach::Woken)], "forwarded urgent");
    assert_eq!(
        wire.read(Side::Laptop, &builder, None).last().map(String::as_str),
        Some("review critic@devenv: now")
    );
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

/// A refetch that took a post before the home sent it on does not leave the home's answer saying
/// it reached nobody: the replica says whom it reached for entries it already took.
#[test]
fn a_post_a_refetch_took_first_still_says_whom_it_reached() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let posted = laptop.post(&builder, None, &[], "rebase first", sessions, now).unwrap();
    // The devenv's link-up refetch, run late, fetches the post before the laptop sends it on.
    for (group, head) in wire.devenv.replicas_of("lap") {
        wire.send(Side::Devenv, &Call::Since { group, after: head }).expect("the link is up");
    }
    let reached = wire.tell(Side::Laptop, &posted.tell);

    assert_eq!(
        reached,
        [("critic@devenv".to_string(), Reach::Woken)],
        "the home's answer does not say the critic was reached"
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
/// A post to someone on another machine, sharing no group with them, makes the group of
/// exactly them on the author's machine, as it does on one; the other machine holds it as a
/// replica and wakes its own member.
#[test]
fn a_post_to_someone_elsewhere_makes_the_group_of_them_here() {
    let mut wire = Wire::new();
    let (builder, critic, scout) = (session("builder"), session("critic"), session("scout"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "far");
    wire.join(Side::Laptop, &scout, Some("scout"), "near");

    let posted = wire.post(Side::Laptop, &scout, None, &["critic"], "hi").unwrap();
    assert_eq!(
        (posted.group.as_str(), woke(&posted)),
        ("critic+scout", vec![("critic@devenv", Reach::Woken)])
    );
    assert_eq!(wire.read(Side::Devenv, &critic, None), ["critic+scout@lap scout@lap: hi"]);

    let posted = wire.post(Side::Devenv, &critic, None, &["scout@lap"], "back").unwrap();
    assert_eq!(posted.group, "critic+scout@lap", "the group the two share, wherever it is kept");
    let posted = wire.post(Side::Devenv, &critic, None, &["builder"], "and you").unwrap();
    assert_eq!(
        (posted.group.as_str(), woke(&posted)),
        ("builder+critic", vec![("builder@lap", Reach::Woken)])
    );
    let posted =
        wire.post(Side::Laptop, &builder, None, &["critic", "scout"], "all of you").unwrap();
    assert_eq!(posted.group, "builder+critic+scout");
}

/// `--to` a pane on another machine reaches the agent in it, though it never joined anything:
/// that machine makes it a participant, named after the pane, as `--to` does there.
#[test]
fn a_post_to_a_pane_on_another_machine_rings_it_there() {
    let mut wire = Wire::new();
    wire.pane(Side::Devenv, "p2dev");
    let builder = session("builder");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");

    let posted = wire.post(Side::Laptop, &builder, None, &["p2dev"], "carry on").unwrap();
    assert_eq!(
        (posted.group.as_str(), woke(&posted)),
        ("builder+p2dev", vec![("p2dev@devenv", Reach::Woken)])
    );
    let rung: Vec<_> = wire.wakes.iter().filter(|(side, _)| *side == Side::Devenv).collect();
    assert_eq!(rung.len(), 1);
    assert_eq!(rung[0].1.via, Via::Pane("p2dev".to_string()));

    let refused = wire.post(Side::Laptop, &builder, None, &["nobody"], "anyone?");
    assert_eq!(refused.unwrap_err(), Refusal::NoSuchParticipant { name: "nobody".to_string() });
}

/// A pane on another machine is still an address once the agent in it has joined under a name
/// of its own: the other machine answers with that name, and the post reaches it.
#[test]
fn a_post_to_a_pane_on_another_machine_reaches_the_name_its_agent_took() {
    let mut wire = Wire::new();
    wire.pane(Side::Devenv, "p2dev");
    let src = Caller { pane: Some("p2dev".to_string()), ..session("src") };
    wire.join(Side::Devenv, &src, Some("src"), "far");
    let builder = session("builder");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");

    let posted = wire.post(Side::Laptop, &builder, None, &["p2dev"], "report back").unwrap();
    assert_eq!(
        (posted.group.as_str(), woke(&posted)),
        ("builder+src", vec![("src@devenv", Reach::Woken)])
    );
    wire.read(Side::Laptop, &builder, None);
    let posted = wire.post(Side::Laptop, &builder, None, &["p2dev@devenv"], "again").unwrap();
    assert_eq!(posted.group, "builder+src");
    assert_eq!(
        wire.read(Side::Devenv, &src, Some("builder+src")),
        ["builder+src@lap builder@lap: report back", "builder+src@lap builder@lap: again"]
    );

    let found = wire.found(Side::Laptop, &["p2dev".to_string()]);
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let added = laptop
        .group_members(&builder, "review", &["p2dev".to_string()], &[], &found, sessions, now)
        .unwrap();
    assert_eq!(added.added, ["src@devenv"]);
}

/// The group of this machine's `critic` and `builder` goes by `builder+critic`, so the group of
/// `builder` and the other machine's `critic` takes the next free name.
#[test]
fn a_pair_whose_name_is_taken_by_another_pair_takes_the_next() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Laptop, &critic, Some("critic"), "other");
    wire.join(Side::Devenv, &session("critic"), Some("critic"), "far");
    wire.post(Side::Laptop, &builder, None, &["critic"], "near").unwrap();

    let posted = wire.post(Side::Laptop, &builder, None, &["critic@devenv"], "far").unwrap();
    assert_eq!(posted.group, "builder+critic-2");
    assert_eq!(woke(&posted), [("critic@devenv", Reach::Woken)]);
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

/// The same entries twice wake nobody again, and say whom they reached when they were taken;
/// entries after a gap are refused with the head the gap follows.
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
    assert!(again.wakes.is_empty(), "the same entries woke somebody again: {again:?}");
    assert_eq!(again.reached, [("critic".to_string(), Reach::Woken)], "{again:?}");

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
        urgent: set(&["*"]),
        paused: false,
    }
}

/// A director on the laptop adds a devenv pane to its council and briefs it. The agent there
/// reads the brief, which gives the pane's participant its inbox, and names itself: the
/// laptop is told the name took the pane's place, so its policy does not hold the name out.
/// Whether the name is taken with the join or on its own, the council has it and not the pane.
#[test]
fn a_devenv_pane_added_to_a_laptop_council_takes_a_name_in_it() {
    for joining_the_council in [true, false] {
        let mut wire = Wire::new();
        wire.pane(Side::Devenv, "p9");
        let director = session("director");
        let now = wire.tick();
        let (laptop, sessions) = wire.split(Side::Laptop);
        laptop.join(&director, Some("director"), None, sessions, now).unwrap();
        let policy = Some(directed(&["director", HUMAN]));
        laptop.group_new(&director, "council", policy, sessions, now).unwrap();
        let found = wire.found(Side::Laptop, &["p9".to_string()]);
        let now = wire.tick();
        let (laptop, sessions) = wire.split(Side::Laptop);
        let added = laptop
            .group_members(&director, "council", &["p9".to_string()], &[], &found, sessions, now)
            .unwrap();
        wire.tell(Side::Laptop, &added.tell);
        wire.post(Side::Laptop, &director, Some("council"), &["p9"], "brief").unwrap();

        let tracer = Caller { pane: Some("p9".to_string()), ..session("tracer") };
        assert_eq!(wire.read(Side::Devenv, &tracer, None), ["council@lap director@lap: brief"]);
        if joining_the_council {
            assert_eq!(wire.join(Side::Devenv, &tracer, Some("tracer"), "council"), "council@lap");
        } else {
            let now = wire.tick();
            let (devenv, sessions) = wire.split(Side::Devenv);
            devenv.join(&tracer, Some("tracer"), None, sessions, now).unwrap();
            wire.fold(Side::Devenv);
        }

        let posted = wire.post(Side::Devenv, &tracer, None, &["director"], "done").unwrap();
        assert_eq!(woke(&posted), [("director@lap", Reach::Woken)]);
        let members = |wire: &Wire, side: Side, group: &str| {
            let service = match side {
                Side::Laptop => &wire.laptop,
                Side::Devenv => &wire.devenv,
            };
            let summary = service.groups().into_iter().find(|summary| summary.name == group);
            summary.unwrap().members
        };
        assert_eq!(members(&wire, Side::Laptop, "council"), ["director", "tracer@devenv"]);
        assert_eq!(members(&wire, Side::Devenv, "council@lap"), ["director@lap", "tracer"]);
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
    // A member on the devenv, or its replica is not held; the posts are not sent on, so the
    // pages below are what bring them.
    wire.join(Side::Devenv, &session("critic"), Some("critic"), "review");
    let joined = wire.devenv.log("review@lap", 0).unwrap().last().unwrap().seq;
    wire.losing = true;
    let page = "x".repeat(muster_msg::LARGEST_BODY - 1024);
    for _ in 0..18 {
        wire.post(Side::Laptop, &builder, None, &[], &page).unwrap();
    }
    wire.losing = false;

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
    let (mut after, mut pages, mut more) = (joined, 0, true);
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

fn bad_name(result: &Result<(), Refusal>) -> bool {
    matches!(result, Err(Refusal::BadName { .. }))
}

/// Whoever joins, leaves or posts in a call is the asker's own participant, and the group is one
/// kept here, so both are bare as the asker writes them. A call naming this machine's own
/// participant, the human included, would otherwise become that participant on arrival, and a
/// name no participant could have never reaches anything here.
#[test]
fn a_call_acting_as_this_machines_own_or_naming_no_name_is_refused() {
    let mut wire = Wire::new();
    wire.join(Side::Laptop, &session("builder"), Some("builder"), "review");
    let post = |author: &str, to: &[&str]| Call::Post {
        author: author.to_string(),
        group: "review".to_string(),
        to: to.iter().map(ToString::to_string).collect(),
        body: "done".to_string(),
        urgent: false,
        cursor: 99,
        head: 0,
    };
    let forged = [
        Call::Join {
            name: "builder@lap".to_string(),
            group: "review".to_string(),
            head: 0,
            was: None,
        },
        // Taking the place of this machine's own member, which only this machine may move.
        Call::Join {
            name: "critic".to_string(),
            group: "review".to_string(),
            head: 0,
            was: Some("builder@lap".to_string()),
        },
        Call::Leave { name: "builder@lap".to_string(), group: "review".to_string(), head: 0 },
        post("@human@lap", &[]),
        post("critic", &["a b"]),
        Call::Join {
            name: "critic".to_string(),
            group: "x'; sh; '".to_string(),
            head: 0,
            was: None,
        },
        Call::Since { group: "review@lap".to_string(), after: 0 },
    ];
    let before = wire.laptop.log("review", 0).unwrap();
    for call in forged {
        assert!(bad_name(&call.check()), "{call:?}");
        let (laptop, sessions) = wire.split(Side::Laptop);
        let answered = laptop.answer(&Side::Laptop.peer(), call.clone(), sessions, 50);
        let refused = matches!(
            answered.reply,
            Reply::Refused { refusal: Refusal::BadName { .. }, caught: None }
        );
        assert!(refused, "{call:?} answered {:?}", answered.reply);
    }
    assert_eq!(wire.laptop.log("review", 0).unwrap(), before, "nothing changed here");
    assert_eq!(post("critic", &["builder@lap", "@human@lap", "scout@third"]).check(), Ok(()));
}

/// A home's entries name its own members bare and everyone else's with their machine, the
/// receiver's own included, and its group bare: a group written `review@<receiver>` would land
/// on a group the receiver keeps. Names no participant could have are refused whole, so none of
/// them reaches a window.
#[test]
fn entries_from_a_home_with_names_no_participant_could_have_are_refused() {
    let entry = |author: &str| Entry {
        seq: 1,
        at_ms: 1,
        what: What::Message {
            author: author.to_string(),
            to: Vec::new(),
            body: "x".to_string(),
            urgent: false,
        },
    };
    let caught = |group: &str, author: &str| Caught {
        group: group.to_string(),
        policy: Policy::default(),
        entries: vec![entry(author)],
        more: false,
    };
    assert_eq!(caught("review", "critic").check(), Ok(()));
    assert_eq!(caught("review", "@human@lap").check(), Ok(()), "a human who posted from there");
    assert!(bad_name(&caught("review@lap", "critic").check()));
    assert!(bad_name(&caught("x'; sh; '", "critic").check()));
    assert!(bad_name(&caught("review", "critic\u{1b}[2J").check()));
    let mut ruled = caught("review", "critic");
    ruled.policy.membership = vec!["a b".to_string()];
    assert!(bad_name(&ruled.check()));

    let member = |name: &str| Member {
        name: name.to_string(),
        liveness: Liveness::Alive,
        activity: None,
        groups: Vec::new(),
        inbox: None,
        pane: None,
    };
    assert_eq!(Reply::Members(vec![member("critic")]).check(), Ok(()));
    assert!(bad_name(&Reply::Members(vec![member("critic\n")]).check()));
    let posted = |name: &str| Reply::Posted {
        seq: 1,
        reached: vec![(name.to_string(), Reach::Woken)],
        caught: caught("review", "critic"),
    };
    assert_eq!(posted("builder@lap").check(), Ok(()));
    assert!(bad_name(&posted("a;b").check()));
    let refused = Reply::Refused {
        refusal: Refusal::AddressedSelf,
        caught: Some(caught("review@lap", "critic")),
    };
    assert!(bad_name(&refused.check()));
}

/// A change whose reply never came may have been made at the home all the same, so the replica
/// says it may be behind until the home's entries next reach it.
#[test]
fn a_replica_whose_call_went_unanswered_may_be_behind_until_it_hears_again() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    assert_eq!(wire.devenv.behind("review"), None);

    wire.devenv.unanswered("review@lap");
    assert_eq!(wire.devenv.behind("review"), Some("lap"));
    wire.post(Side::Laptop, &builder, None, &[], "the home's next entry").unwrap();
    assert_eq!(wire.devenv.behind("review"), None);
}

/// Entries a refetch brings wake the human once, for all of them: each would otherwise raise
/// the window's notice again, one message at a time.
#[test]
fn a_refetch_tells_the_human_once_for_all_it_brings() {
    let mut wire = Wire::new();
    wire.sessions.attended.set(true);
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &human(), None, "review");

    wire.cut();
    for body in ["one", "two", "three"] {
        let now = wire.tick();
        let (devenv, sessions) = wire.split(Side::Devenv);
        devenv.post(&critic, None, &[], body, sessions, now).unwrap();
    }
    wire.wakes.clear();
    wire.mend();
    let told: Vec<u64> = wire
        .wakes
        .iter()
        .filter(|(side, wake)| *side == Side::Laptop && wake.via == Via::Human)
        .map(|(_, wake)| wake.notice.count)
        .collect();
    assert_eq!(told, [3]);
}

/// A leave from every group is refused whole when a group's policy keeps the caller in one, as
/// on one machine: no group kept elsewhere is left first.
#[test]
fn a_leave_from_every_group_is_refused_before_any_is_left() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.group_new(&builder, "desk", Some(directed(&["director"])), sessions, now).unwrap();

    let refused = laptop.route_leave(&builder, None, sessions);
    assert!(
        matches!(&refused, Err(Refusal::NotPermitted { group, action: Action::Leave, .. }) if group == "desk"),
        "{refused:?}"
    );
}

/// The home removes a member on another machine by the name it knows it by, and that machine's
/// replica lets it go as a leave would: no cursor left to refetch for, and a wait kept to the
/// group ended.
#[test]
fn a_member_on_another_machine_can_be_removed_and_is_let_go_there() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    let (devenv, sessions) = wire.split(Side::Devenv);
    let waited = devenv.wait(&critic, Some("review@lap"), false, sessions).unwrap();
    let muster_msg::Waited::Waiting { ticket, .. } = waited else { panic!("nothing unread") };

    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let removed = laptop
        .group_members(&builder, "review", &[], &["critic@devenv".to_string()], &[], sessions, now)
        .unwrap();
    assert_eq!(removed.removed, ["critic@devenv"]);
    wire.tell(Side::Laptop, &removed.tell);

    let critic_there = wire.devenv.participant("critic").unwrap();
    assert!(!critic_there.cursors.contains_key("review@lap"), "{:?}", critic_there.cursors);
    assert!(wire.devenv.replicas_of("lap").is_empty(), "nothing left to refetch");
    assert_eq!(wire.ended, [(Side::Devenv, ticket)]);
}

/// A member on another machine that the home removes with its dismissal unread can still read
/// it there, once; then its machine lets go of the replica.
#[test]
fn a_member_removed_on_another_machine_reads_its_dismissal_there() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.post(Side::Laptop, &builder, None, &["critic"], "thanks, you are done").unwrap();

    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let removed = laptop
        .group_members(&builder, "review", &[], &["critic@devenv".to_string()], &[], sessions, now)
        .unwrap();
    wire.tell(Side::Laptop, &removed.tell);
    wire.post(Side::Laptop, &builder, None, &[], "after").unwrap();

    assert!(wire.devenv.woken_for("critic", "review@lap"), "its wake still stands");
    assert_eq!(
        wire.read(Side::Devenv, &critic, Some("review")),
        ["review@lap builder@lap: thanks, you are done"]
    );
    assert!(wire.devenv.replicas_of("lap").is_empty(), "nothing left to refetch");
    let (devenv, sessions) = wire.split(Side::Devenv);
    assert!(matches!(
        devenv.read(&critic, Some("review"), sessions),
        Err(Refusal::NoSuchGroup { .. })
    ));
}

/// A replica replayed from nothing a page at a time lets a member go only by where the whole log
/// leaves it: one that left and came back, its leave closing one page and its return in the
/// next, keeps its cursor, its wait and the replica.
#[test]
fn a_member_that_left_and_rejoined_across_a_page_boundary_is_kept() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    // Eight bodies nearly fill a page, and the ninth opens the next: a page ends only before a
    // message, so the leave closes the first page and the return falls in the second.
    let body = "x".repeat(muster_msg::LARGEST_BODY - 1024);
    for _ in 0..8 {
        wire.post(Side::Laptop, &builder, None, &[], &body).unwrap();
    }
    let (devenv, sessions) = wire.split(Side::Devenv);
    let away = devenv.route_leave(&critic, Some("review@lap"), sessions).unwrap();
    for away in away {
        wire.send(Side::Devenv, &away.call).unwrap();
    }
    wire.post(Side::Laptop, &builder, None, &[], &body).unwrap();
    wire.join(Side::Devenv, &critic, Some("critic"), "review@lap");
    let rejoined = wire.laptop.log("review", 0).unwrap().last().unwrap().seq;
    assert_eq!(wire.devenv.participant("critic").unwrap().cursors["review@lap"], rejoined);

    let saved = wire.devenv.store().saved.clone().unwrap();
    wire.devenv = Messaging::restore(Memory::default(), saved, BTreeMap::default());
    wire.cut();
    let (devenv, sessions) = wire.split(Side::Devenv);
    let waited = devenv.wait(&critic, Some("review@lap"), false, sessions).unwrap();
    let muster_msg::Waited::Waiting { ticket, .. } = waited else { panic!("nothing unread") };

    let (mut after, mut pages, mut more, mut ended) = (0, Vec::new(), true, Vec::new());
    while more {
        let page = wire.laptop.since("review", after).unwrap();
        let (devenv, sessions) = wire.split(Side::Devenv);
        let applied = devenv.apply(&Side::Devenv.peer(), page.clone(), sessions, 99).unwrap();
        ended.extend(applied.ended);
        after = page.entries.last().unwrap().seq;
        more = page.more;
        pages.push(page.entries.last().unwrap().what.clone());
    }
    assert_eq!(pages.len(), 2, "{pages:?}");
    assert_eq!(pages[0], What::Left { who: "critic@devenv".to_string() }, "the leave ends a page");
    assert!(ended.is_empty(), "no wait ended: {ended:?}");
    assert_eq!(wire.devenv.participant("critic").unwrap().cursors["review@lap"], rejoined);
    let replica = wire.devenv.log("review@lap", 0).expect("the replica is kept");
    assert_eq!(
        replica.iter().filter(|entry| matches!(entry.what, What::Message { .. })).count(),
        9
    );
    let (devenv, sessions) = wire.split(Side::Devenv);
    let again = devenv.wait(&critic, Some("review@lap"), false, sessions).unwrap();
    assert!(
        matches!(again, muster_msg::Waited::Waiting { superseded: Some(old), .. } if old == ticket),
        "the first wait was still kept: {again:?}"
    );
}

/// The devenv once the laptop's daemon has dialed it: the human is homed on the laptop.
fn dialed() -> Wire {
    let mut wire = Wire::new();
    wire.devenv.dialed_by(&Side::Devenv.peer()).unwrap();
    wire
}

fn human_elsewhere() -> Refusal {
    Refusal::HumanElsewhere { machine: "lap".to_string(), calls_us: "devenv".to_string() }
}

/// A person's shell on a daemon the laptop's dialed is the laptop's human. What keeps the
/// human's cursors is refused there, naming the laptop, and no human is made on the devenv. Its
/// daemon carries such a request to the laptop while a link is up (`linked.rs` in
/// muster-daemon's tests).
#[test]
fn a_person_on_the_far_machine_is_the_laptops_human() {
    let mut wire = dialed();
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    let now = wire.tick();
    let (devenv, sessions) = wire.split(Side::Devenv);

    let refused = devenv.join(&human(), None, Some("review"), sessions, now);
    assert_eq!(refused.unwrap_err(), human_elsewhere());
    let refused = devenv.join(&human(), Some(HUMAN), Some("review"), sessions, now);
    assert_eq!(refused.unwrap_err(), human_elsewhere());
    assert_eq!(devenv.read(&human(), None, sessions).unwrap_err(), human_elsewhere());
    assert_eq!(devenv.wait(&human(), None, false, sessions).unwrap_err(), human_elsewhere());
    assert_eq!(devenv.leave(&human(), None, sessions, now).unwrap_err(), human_elsewhere());
    let refused = devenv.group_new(&human(), "mine", None, sessions, now);
    assert_eq!(refused.unwrap_err(), human_elsewhere());
    assert!(devenv.participant(HUMAN).is_none(), "the devenv made a human of its own");
}

/// The person's join on the far machine is refused as theirs before anything asks where the
/// group is kept: with the link up, down, or to a group kept on the laptop, it names the laptop
/// rather than the link or a group to make.
#[test]
fn the_persons_join_on_the_far_machine_names_the_laptop_whatever_the_link() {
    let mut wire = dialed();
    wire.join(Side::Laptop, &session("builder"), Some("builder"), "review");
    wire.join(Side::Devenv, &session("critic"), Some("critic"), "review");
    for up in [true, false] {
        if !up {
            wire.cut();
        }
        let (devenv, sessions) = wire.split(Side::Devenv);
        for group in ["council", "review"] {
            for name in [None, Some(HUMAN)] {
                let routed = devenv.route_join(&human(), name, Some(group), sessions);
                assert_eq!(routed.unwrap_err(), human_elsewhere(), "{group}, link up: {up}");
            }
        }
    }
}

/// An agent on the far machine that asks for the human means the laptop's: in a group the
/// human joined, the laptop is told and wakes it; in none, the group of the two is made there,
/// and the laptop is told of it too.
#[test]
fn the_far_machines_human_is_the_laptops() {
    let mut wire = dialed();
    let (critic, scout) = (session("critic"), session("scout"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.join(Side::Devenv, &scout, Some("scout"), "other");

    let posted = wire.post(Side::Devenv, &critic, None, &[HUMAN], "a question").unwrap();
    assert_eq!(woke(&posted), [("@human@lap", Reach::Waiting)]);
    assert_eq!(told_the_human(&wire), [(Side::Laptop, "review@devenv")]);

    let posted = wire.post(Side::Devenv, &scout, None, &[HUMAN], "and mine").unwrap();
    assert_eq!(posted.group, "@human+scout");
    assert_eq!(woke(&posted), [("@human@lap", Reach::Waiting)]);
    assert_eq!(told_the_human(&wire).last(), Some(&(Side::Laptop, "@human+scout@devenv")));
    assert!(wire.devenv.participant(HUMAN).is_none(), "the devenv made a human of its own");
}

/// The person may post in, and change, a group kept on the far machine from a shell there,
/// as the laptop's human. The laptop learns of the post as its own human's, which wakes the
/// laptop's members and not the human, and a resume there does not wake the one who made it.
#[test]
fn the_person_posts_and_resumes_from_the_far_machine_as_the_laptops_human() {
    let mut wire = dialed();
    let (critic, builder) = (session("critic"), session("builder"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.join(Side::Laptop, &builder, Some("builder"), "review");

    let posted = wire.post(Side::Devenv, &human(), None, &[], "go ahead").unwrap();
    assert_eq!(posted.author, "@human@lap");
    assert_eq!(
        woke(&posted),
        [("critic", Reach::Woken), ("builder@lap", Reach::Woken)],
        "the laptop was told, and reached its own members"
    );
    assert!(told_the_human(&wire).is_empty(), "the human was woken by its own post");
    assert_eq!(wire.read(Side::Laptop, &builder, None), ["review@devenv @human: go ahead"]);

    let now = wire.tick();
    let (devenv, sessions) = wire.split(Side::Devenv);
    let paused = devenv.pause(&critic, "review", sessions, now).unwrap();
    wire.tell(Side::Devenv, &paused.tell);
    wire.read(Side::Devenv, &critic, None);
    wire.post(Side::Devenv, &critic, None, &[], "while paused").unwrap();
    wire.wakes.clear();
    let now = wire.tick();
    let (devenv, sessions) = wire.split(Side::Devenv);
    let resumed = devenv.resume(&human(), "review", sessions, now).unwrap();
    assert_eq!(resumed.author, "@human@lap");
    wire.tell(Side::Devenv, &resumed.tell);
    assert!(told_the_human(&wire).is_empty(), "the resume woke the one who made it");
}

/// What the person asks of a group kept on the laptop goes to the laptop, where the human's
/// cursors are: the devenv forwards only its own members'.
#[test]
fn the_person_posts_to_a_laptop_group_from_the_laptop() {
    let mut wire = dialed();
    let critic = session("critic");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    let refused = wire.post(Side::Devenv, &human(), Some("review"), &[], "from here");
    assert_eq!(refused.unwrap_err(), human_elsewhere());
    let refused = wire.post(Side::Devenv, &human(), None, &["critic"], "or to you");
    assert_eq!(refused.unwrap_err(), human_elsewhere());
}

/// A daemon a window attends is the human's home, whoever dialed it last: the app runs on its
/// machine too. What was learned survives a restart.
#[test]
fn a_daemon_a_window_attends_keeps_its_own_human() {
    let mut wire = dialed();
    wire.sessions.attended.set(true);
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    assert_eq!(wire.join(Side::Devenv, &human(), None, "review"), "review");

    let saved = wire.devenv.store().saved.clone().expect("the devenv saved its state");
    let mut restored = Messaging::restore(Memory::default(), saved, BTreeMap::new());
    wire.sessions.attended.set(false);
    let refused = restored.read(&human(), None, &wire.sessions);
    assert_eq!(refused.unwrap_err(), human_elsewhere());
}

/// A new ring set at the home can stop ringing the human for what already waits, or start:
/// the laptop's replica says what waits for the human under it, as the laptop's own group
/// set does.
#[test]
fn a_new_ring_set_at_the_home_re_tells_what_waits_for_the_human() {
    let mut wire = Wire::new();
    let critic = session("critic");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    wire.join(Side::Laptop, &human(), None, "review");
    wire.post(Side::Devenv, &critic, None, &[], "one").unwrap();
    wire.post(Side::Devenv, &critic, None, &[], "two").unwrap();
    wire.wakes.clear();

    let agents_only = Policy {
        ring: BTreeMap::from([("*".to_string(), vec!["*".to_string()])]),
        ..Policy::default()
    };
    let now = wire.tick();
    let (devenv, sessions) = wire.split(Side::Devenv);
    let set = devenv.group_set(&critic, "review", agents_only, sessions, now).unwrap();
    wire.tell(Side::Devenv, &set.tell);
    let told: Vec<(Side, &str, u64)> = wire
        .wakes
        .iter()
        .filter(|(_, wake)| wake.via == Via::Human)
        .map(|(side, wake)| (*side, wake.notice.group.as_str(), wake.notice.count))
        .collect();
    assert_eq!(told, [(Side::Laptop, "review@devenv", 0)]);
}

/// What the devenv's replica of `review` lacks from the laptop, applied as one batch, as a
/// replica that missed the entries is sent them when it next hears from the home.
fn catch_up(wire: &mut Wire, head: u64) -> muster_msg::Applied {
    let caught = wire.laptop.since("review", head).unwrap();
    let now = wire.tick();
    let (devenv, sessions) = wire.split(Side::Devenv);
    devenv.apply(&Side::Devenv.peer(), caught, sessions, now).unwrap()
}

/// The devenv's replica of `review` with `critic` in it, and its head: from here on the laptop's
/// entries are lost on the way, until [`catch_up`].
fn replica_missing_entries() -> (Wire, Caller, u64) {
    let mut wire = Wire::new();
    wire.join(Side::Laptop, &session("builder"), Some("builder"), "review");
    wire.join(Side::Devenv, &session("critic"), Some("critic"), "review");
    let head = wire.laptop.since("review", 0).unwrap().entries.last().unwrap().seq;
    wire.losing = true;
    (wire, session("builder"), head)
}

/// A batch holding a message and then a pause wakes the message's members at once, as the
/// home woke its own when it was posted. The pause forgets that wake, as it did at the home, so
/// a resume wakes them again for what they still have unread.
#[test]
fn a_message_before_a_pause_in_one_batch_wakes_at_once() {
    let (mut wire, builder, head) = replica_missing_entries();
    wire.post(Side::Laptop, &builder, None, &[], "before the pause").unwrap();
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.pause(&builder, "review", sessions, now).unwrap();

    let applied = catch_up(&mut wire, head);
    assert_eq!(applied.reached, [("critic".to_string(), Reach::Woken)]);

    wire.losing = false;
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let resumed = laptop.resume(&builder, "review", sessions, now).unwrap();
    let reached = wire.tell(Side::Laptop, &resumed.tell);
    assert_eq!(reached, [("critic@devenv".to_string(), Reach::Woken)]);
}

/// A message posted while the group was paused is held, though the batch ends resumed, and the
/// resume wakes its members once.
#[test]
fn a_message_while_paused_in_one_batch_is_held_until_the_resume() {
    let (mut wire, builder, head) = replica_missing_entries();
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.pause(&builder, "review", sessions, now).unwrap();
    wire.post(Side::Laptop, &builder, None, &[], "while paused").unwrap();
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.resume(&builder, "review", sessions, now).unwrap();

    let applied = catch_up(&mut wire, head);
    assert_eq!(
        applied.reached,
        [("critic".to_string(), Reach::Paused), ("critic".to_string(), Reach::Woken)]
    );
}

/// A message posted before a new ring set is rung under the one it was posted under, which the
/// replica holds from the batch before.
#[test]
fn a_message_before_a_new_ring_set_in_one_batch_rings_under_the_old() {
    let (mut wire, builder, head) = replica_missing_entries();
    wire.post(Side::Laptop, &builder, None, &[], "to everyone").unwrap();
    let only_builder = Policy {
        ring: BTreeMap::from([("*".to_string(), vec!["builder".to_string()])]),
        ..Policy::default()
    };
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.group_set(&builder, "review", only_builder, sessions, now).unwrap();

    let applied = catch_up(&mut wire, head);
    assert_eq!(applied.reached, [("critic".to_string(), Reach::Woken)]);
}

/// A message between two new ring sets in one batch is rung under the first, the one it was
/// posted under, rather than under the last the batch ends with.
#[test]
fn a_message_between_two_ring_sets_in_one_batch_rings_under_the_one_before_it() {
    let (mut wire, builder, head) = replica_missing_entries();
    let ring = |names: &[&str]| Policy {
        ring: BTreeMap::from([(
            "*".to_string(),
            names.iter().map(ToString::to_string).collect::<Vec<_>>(),
        )]),
        ..Policy::default()
    };
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.group_set(&builder, "review", ring(&["critic@devenv"]), sessions, now).unwrap();
    wire.post(Side::Laptop, &builder, None, &[], "for the critic").unwrap();
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.group_set(&builder, "review", ring(&["builder"]), sessions, now).unwrap();

    let applied = catch_up(&mut wire, head);
    assert_eq!(applied.reached, [("critic".to_string(), Reach::Woken)]);
}

/// A name copied from the other machine's answer means what it meant there: the devenv reads
/// `review@devenv` and `critic@devenv`, as the laptop writes them, as its own `review` and
/// `critic`.
#[test]
fn a_name_written_as_the_other_machine_writes_ours_is_ours() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    assert_eq!(
        wire.join(Side::Laptop, &builder, Some("builder"), "review@devenv"),
        "review@devenv"
    );

    let posted =
        wire.post(Side::Devenv, &critic, Some("review@devenv"), &["builder@lap"], "one").unwrap();
    assert_eq!(posted.group, "review");
    wire.read(Side::Laptop, &builder, None);
    let posted = wire.post(Side::Laptop, &builder, None, &["critic@devenv"], "two").unwrap();
    assert_eq!(woke(&posted), [("critic@devenv", Reach::Woken)]);
    wire.read(Side::Devenv, &critic, None);
    let (devenv, sessions) = wire.split(Side::Devenv);
    let refused =
        devenv.post(&critic, Some("review"), &["critic@devenv".to_string()], "me", sessions, 9);
    assert_eq!(refused.unwrap_err(), Refusal::AddressedSelf);
}

/// A group's policy can name a member on another machine, as its home names it: a director on
/// the devenv of a council kept on the laptop joins it, rings it and is rung by it, and a member
/// added there from the laptop is held to it. The devenv reads the policy in its own names.
#[test]
fn a_policy_names_a_member_on_another_machine() {
    let set = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
    let council = Policy {
        ring: BTreeMap::from([
            ("director@devenv".to_string(), set(&["*"])),
            ("*".to_string(), set(&["director@devenv"])),
        ]),
        allow: BTreeMap::from([
            ("director@devenv".to_string(), set(&["*"])),
            ("*".to_string(), set(&["director@devenv", HUMAN])),
        ]),
        membership: set(&["director@devenv", "builder"]),
        urgent: set(&["*"]),
        paused: false,
    };
    let mut wire = Wire::new();
    let (builder, director, critic) = (session("builder"), session("director"), session("critic"));
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.join(&builder, Some("builder"), None, sessions, now).unwrap();
    laptop.group_new(&builder, "council", Some(council), sessions, now).unwrap();

    assert_eq!(wire.join(Side::Devenv, &director, Some("director"), "council"), "council@lap");
    let (devenv, sessions) = wire.split(Side::Devenv);
    let route = devenv.route_join(&critic, Some("critic"), Some("council@lap"), sessions);
    let Route::Away(away) = route.unwrap() else { panic!("council is kept on the laptop") };
    assert_eq!(
        wire.send(Side::Devenv, &away.call).unwrap_err(),
        Refusal::NotPermitted {
            name: "critic".to_string(),
            group: "council@lap".to_string(),
            action: Action::Join,
            permitted: vec!["director".to_string(), "builder@lap".to_string()],
        }
    );

    let found = wire.found(Side::Laptop, &["critic".to_string()]);
    let now = wire.tick();
    let (laptop, sessions) = wire.split(Side::Laptop);
    let added = laptop
        .group_members(&builder, "council", &["critic".to_string()], &[], &found, sessions, now)
        .unwrap();
    assert_eq!(added.added, ["critic@devenv"]);
    wire.tell(Side::Laptop, &added.tell);

    let posted = wire.post(Side::Laptop, &builder, None, &[], "ready").unwrap();
    assert_eq!(woke(&posted), [("director@devenv", Reach::Woken)]);
    wire.read(Side::Devenv, &director, None);
    let posted = wire.post(Side::Devenv, &director, None, &[], "go").unwrap();
    assert_eq!(woke(&posted), [("builder@lap", Reach::Woken), ("critic", Reach::Woken)]);
    wire.read(Side::Devenv, &critic, None);
    let refused = wire.post(Side::Devenv, &critic, None, &["builder"], "around the director");
    assert_eq!(
        refused.unwrap_err(),
        Refusal::NotAllowed {
            addressee: "builder@lap".to_string(),
            group: "council@lap".to_string(),
            allowed: vec!["director".to_string(), HUMAN.to_string()],
        }
    );
}

/// A request the person makes on the far machine is carried to the laptop with its names as the
/// devenv writes them, which the laptop turns into its own: what the devenv knows means what
/// it means there, and what it does not is taken to be the laptop's.
#[test]
fn a_carried_request_names_what_the_far_machine_meant() {
    let mut wire = dialed();
    wire.pane(Side::Devenv, "p9");
    wire.join(Side::Devenv, &session("critic"), Some("critic"), "review");
    wire.join(Side::Laptop, &session("builder"), Some("builder"), "board");
    wire.join(Side::Devenv, &session("scout"), Some("scout"), "board");
    let (devenv, sessions) = wire.split(Side::Devenv);
    assert_eq!(
        devenv.person_elsewhere(&human(), sessions).map(|home| home.machine),
        Some("lap".to_string())
    );
    assert_eq!(devenv.person_elsewhere(&session("critic"), sessions), None);
    assert_eq!(devenv.home_of("board"), Some("lap".to_string()));
    assert_eq!(devenv.home_of("review"), None);

    let laptop_reads = Side::Laptop.peer();
    let group = |name: &str| laptop_reads.inward(&devenv.carrying_group(name));
    assert_eq!(group("review"), "review@devenv");
    assert_eq!(group("review@devenv"), "review@devenv");
    assert_eq!(group("board"), "board");
    assert_eq!(group("council"), "council");
    let name = |name: &str| laptop_reads.inward(&devenv.carrying_name(name, sessions));
    assert_eq!(name("critic"), "critic@devenv");
    assert_eq!(name("p9"), "p9@devenv");
    assert_eq!(name("builder"), "builder");
    assert_eq!(name("stranger"), "stranger");
    assert_eq!(name(HUMAN), HUMAN);
    assert_eq!(name("critic@devenv"), "critic@devenv");
}

/// Deleting a group at its home lets go of its members on the other machine: told to forget
/// it, that machine drops the replica, their places in it, and a wait kept to it.
#[test]
fn a_group_deleted_at_its_home_is_forgotten_on_the_other_machine() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");
    let (devenv, sessions) = wire.split(Side::Devenv);
    let waited = devenv.wait(&critic, Some("review@lap"), false, sessions).unwrap();
    let muster_msg::Waited::Waiting { ticket, .. } = waited else { panic!("nothing unread") };

    let (laptop, sessions) = wire.split(Side::Laptop);
    let deleted = laptop.group_delete(&builder, "review", sessions).unwrap();
    assert_eq!(deleted.let_go, ["builder", "critic@devenv"]);
    assert_eq!(deleted.forget, ["devenv"]);
    assert_eq!(wire.laptop.store().removed, ["review"]);

    let forgot = wire.devenv.forget_replica(&Side::Devenv.peer(), "review").unwrap();
    assert_eq!(forgot.group, "review@lap");
    assert_eq!(forgot.ended, [ticket]);
    let critic_there = wire.devenv.participant("critic").unwrap();
    assert!(!critic_there.cursors.contains_key("review@lap"), "{:?}", critic_there.cursors);
    assert!(wire.devenv.replicas_of("lap").is_empty(), "nothing left to refetch");
}

/// A machine whose link was down when the group was deleted is not told, and finds out when the
/// link returns: its refetch is refused as no such group, and it forgets the replica then.
#[test]
fn a_replica_that_missed_a_delete_forgets_the_group_when_the_link_returns() {
    let mut wire = Wire::new();
    let (builder, critic) = (session("builder"), session("critic"));
    wire.join(Side::Laptop, &builder, Some("builder"), "review");
    wire.join(Side::Devenv, &critic, Some("critic"), "review");

    wire.cut();
    let (laptop, sessions) = wire.split(Side::Laptop);
    laptop.group_delete(&builder, "review", sessions).unwrap();
    wire.up = true;
    wire.laptop.linked(&Side::Laptop.peer());
    wire.devenv.linked(&Side::Devenv.peer());
    let replicas = wire.devenv.replicas_of("lap");
    assert_eq!(replicas.len(), 1, "{replicas:?}");
    let (group, after) = replicas[0].clone();
    let refused = wire.send(Side::Devenv, &Call::Since { group, after }).unwrap_err();

    assert_eq!(refused.code(), "no_such_group");
    assert!(wire.devenv.replicas_of("lap").is_empty(), "nothing left to refetch");
    assert!(!wire.devenv.participant("critic").unwrap().cursors.contains_key("review@lap"));
}
