use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Entry, HumanHome, Operation, Participant, Policy, across::Away};

/// Where the host keeps what must outlive it. Both calls happen before the request that caused
/// them is answered, so an answer never reports something the store did not take.
pub trait Store {
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String>;
    fn save(&mut self, state: &Saved) -> Result<(), String>;
    /// [`Self::save`], kept through a power loss too: for a change that, once lost, could let
    /// something already done be done again. A store whose every save is that durable has
    /// nothing more to do.
    fn save_durably(&mut self, state: &Saved) -> Result<(), String> {
        self.save(state)
    }
    /// Forgets a deleted group's log, keeping only `last`, the number of its last entry, so a
    /// group made again under the name numbers on from it and never issues a number twice, and
    /// `operated`, the posts programs made in it that a retry may still ask about. One step, so
    /// whatever a crash interrupts, the log or what replaces it remembers them.
    fn retire(&mut self, group: &str, last: u64, operated: &[Operated]) -> Result<(), String>;
    /// Moves a retired name's log, and the number it keeps, to the same name spelled with other
    /// letter case, which is being made again: a file system that ignores case would otherwise
    /// keep the new group's log under the old spelling. One step, so the number is kept on disk
    /// whatever happens next. A log already gone is not an error.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
}

/// A group's log as the store kept it: its entries, numbered on from `after`, the last number a
/// group deleted under the same name issued (0 for a name never deleted). A log holding no
/// entries is a name retired, with no group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Log {
    pub after: u64,
    pub entries: Vec<Entry>,
    /// Posts programs made in the group deleted under this name, as its retiring kept them.
    pub operated: Vec<Operated>,
}

impl From<Vec<Entry>> for Log {
    fn from(entries: Vec<Entry>) -> Log {
        Log { entries, ..Log::default() }
    }
}

/// A post a program made under an operation id, as the group's home remembers it: read from the
/// post's own entry while its group lives, and from the record its retired log keeps after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operated {
    pub client: String,
    pub id: String,
    pub asked: u64,
    pub group: String,
    pub seq: u64,
    pub at_ms: u64,
}

/// A post a program made under an operation id that this machine sent to the group's home on
/// another, as it sent it: a retry goes to the same home with the same call, whatever this
/// machine's groups say by then, and the home says whether it was posted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    pub operation: Operation,
    pub at_ms: u64,
    pub away: Away,
}

/// Everything but the logs. A group's members are not here: they are read back from its log's
/// notices, so a log appended before a crash that lost this file still says who is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub participants: Vec<Participant>,
    pub groups: Vec<GroupRecord>,
    /// Where the human is homed, once a daemon there has dialed this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_home: Option<HumanHome>,
    /// The callsign of each pane participant on another machine, as that machine said it, both
    /// written as this machine writes them: `p2w3r07bsd@devenv` is `otter@devenv`. Kept because
    /// a group's log is, and nothing else here can say what its authors there are called.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub called_elsewhere: BTreeMap<String, String>,
    /// Programs' posts sent from here to another machine's group, within the retention period.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sent: Vec<Sent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRecord {
    pub name: String,
    pub policy: Policy,
}

/// A store in memory, for tests and for anything that needs no persistence.
#[derive(Debug, Default)]
pub struct Memory {
    pub appended: Vec<(String, Entry)>,
    pub saved: Option<Saved>,
    /// How many times the state was saved.
    pub saves: usize,
    /// How many of those saves were to outlast a power loss.
    pub durable_saves: usize,
    /// Groups whose log was retired, with the last number each issued, in order.
    pub retired: Vec<(String, u64)>,
    /// Groups whose log was renamed, from and to, in order.
    pub renamed: Vec<(String, String)>,
    /// What a disk would hold now: each log as appended, retired and renamed.
    pub kept: BTreeMap<String, Log>,
    /// Set to make every call fail, as a full disk would.
    pub failing: bool,
    /// Set to make only saving the state fail, as a disk filling up between two writes would.
    pub failing_saves: bool,
    /// Set to make only appends fail, as a disk that fills just after a smaller write would.
    pub failing_appends: bool,
    /// Set to let this many more appends through and then fail every call, as a disk that fills
    /// partway through a request would.
    pub failing_after: Option<usize>,
}

impl Store for Memory {
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String> {
        if self.failing() || self.failing_appends {
            return Err("the store is failing on purpose".to_string());
        }
        if let Some(left) = &mut self.failing_after {
            *left -= 1;
        }
        self.appended.push((group.to_string(), entry.clone()));
        self.kept.entry(group.to_string()).or_default().entries.push(entry.clone());
        Ok(())
    }

    fn save(&mut self, state: &Saved) -> Result<(), String> {
        if self.failing() || self.failing_saves {
            return Err("the store is failing on purpose".to_string());
        }
        self.saved = Some(state.clone());
        self.saves += 1;
        Ok(())
    }

    fn save_durably(&mut self, state: &Saved) -> Result<(), String> {
        self.save(state)?;
        self.durable_saves += 1;
        Ok(())
    }

    fn retire(&mut self, group: &str, last: u64, operated: &[Operated]) -> Result<(), String> {
        if self.failing() {
            return Err("the store is failing on purpose".to_string());
        }
        self.retired.push((group.to_string(), last));
        let log = Log { after: last, entries: Vec::new(), operated: operated.to_vec() };
        self.kept.insert(group.to_string(), log);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if self.failing() {
            return Err("the store is failing on purpose".to_string());
        }
        self.renamed.push((from.to_string(), to.to_string()));
        if let Some(log) = self.kept.remove(from) {
            self.kept.insert(to.to_string(), log);
        }
        Ok(())
    }
}

impl Memory {
    fn failing(&self) -> bool {
        self.failing || self.failing_after == Some(0)
    }

    /// The logs a service restored from this store would read.
    pub fn logs(&self) -> BTreeMap<String, Log> {
        self.kept.clone()
    }
}
