//! Groups that span machines (MIP-4, section 11). Not built yet: every call answers as though
//! nothing were kept elsewhere.

use serde::{Deserialize, Serialize};

use crate::{
    AnsweredWait, Caller, Entry, Joined, Left, Member, Messaging, Policy, Posted, Presence, Reach,
    Refusal, Store, Wake,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub calls_us: String,
}

impl Peer {
    pub fn inward(&self, name: &str) -> String {
        name.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tell {
    pub machine: String,
    pub group: String,
    pub after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Find { group: String },
    Join { name: String, group: String, head: u64 },
    Leave { name: String, group: String, head: u64 },
    Post { author: String, group: String, to: Vec<String>, body: String, cursor: u64, head: u64 },
    Since { group: String, after: u64 },
    Who { group: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Away {
    pub machine: String,
    pub call: Call,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Here,
    Away(Away),
    Ask { group: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caught {
    pub group: String,
    pub policy: Policy,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Found(bool),
    Joined { seq: u64, caught: Caught },
    Left { caught: Caught },
    Posted { seq: u64, reached: Vec<(String, Reach)>, caught: Caught },
    Caught(Caught),
    Members(Vec<Member>),
    Refused { refusal: Refusal, caught: Option<Caught> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub reply: Reply,
    pub wakes: Vec<Wake>,
    pub answered: Vec<AnsweredWait>,
    pub tell: Vec<Tell>,
    pub unsaved: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub reached: Vec<(String, Reach)>,
    pub wakes: Vec<Wake>,
    pub answered: Vec<AnsweredWait>,
    pub unsaved: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Found(bool),
    Joined(Joined),
    Left(Left),
    Posted(Posted),
    Members(Vec<Member>),
    Caught(Applied),
    Gap(u64),
}

impl<S: Store> Messaging<S> {
    pub fn linked(&mut self, _machine: &str) {}

    pub fn unlinked(&mut self, _machine: &str) {}

    pub fn behind(&self, _group: &str) -> Option<&str> {
        None
    }

    pub fn replicas_of(&self, _machine: &str) -> Vec<(String, u64)> {
        Vec::new()
    }

    pub fn route_join(
        &mut self,
        _caller: &Caller,
        _name: Option<&str>,
        _group: Option<&str>,
        _presence: &dyn Presence,
    ) -> Result<Route, Refusal> {
        Ok(Route::Here)
    }

    pub fn route_post(
        &mut self,
        _caller: &Caller,
        _group: Option<&str>,
        _to: &[String],
        _body: &str,
        _presence: &dyn Presence,
    ) -> Result<Route, Refusal> {
        Ok(Route::Here)
    }

    pub fn route_leave(
        &mut self,
        _caller: &Caller,
        _group: Option<&str>,
        _presence: &dyn Presence,
    ) -> Result<Vec<Away>, Refusal> {
        Ok(Vec::new())
    }

    pub fn route_who(&self, _group: Option<&str>) -> Result<Vec<Away>, Refusal> {
        Ok(Vec::new())
    }

    pub fn settle(
        &mut self,
        _peer: &Peer,
        _call: &Call,
        _reply: Reply,
        _presence: &dyn Presence,
    ) -> Result<Settled, Refusal> {
        Ok(Settled::Found(false))
    }

    pub fn apply(
        &mut self,
        _peer: &Peer,
        _caught: Caught,
        _presence: &dyn Presence,
    ) -> Result<Applied, u64> {
        Ok(Applied::default())
    }

    pub fn heard(_members: &mut [Member], _machine: &str, _heard: &[Member]) {}

    pub fn answer(
        &mut self,
        _peer: &Peer,
        _call: Call,
        _presence: &dyn Presence,
        _now_ms: u64,
    ) -> Answered {
        Answered {
            reply: Reply::Found(false),
            wakes: Vec::new(),
            answered: Vec::new(),
            tell: Vec::new(),
            unsaved: None,
        }
    }

    pub fn since(&self, group: &str, _after: u64) -> Result<Caught, Refusal> {
        Ok(Caught { group: group.to_string(), policy: Policy::default(), entries: Vec::new() })
    }
}
