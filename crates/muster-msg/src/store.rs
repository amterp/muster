use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Entry, HumanHome, Participant, Policy};

/// Where the host keeps what must outlive it. Both calls happen before the request that caused
/// them is answered, so an answer never reports something the store did not take.
pub trait Store {
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String>;
    fn save(&mut self, state: &Saved) -> Result<(), String>;
    /// Forgets a deleted group's log, keeping only `last`, the number of its last entry, so a
    /// group made again under the name numbers on from it and never issues a number twice.
    fn retire(&mut self, group: &str, last: u64) -> Result<(), String>;
    /// Forgets a log altogether: a retired name made again spelled with other letter case, whose
    /// new log a file system that ignores case would otherwise keep in the old one's file. A log
    /// already gone is not an error.
    fn remove(&mut self, group: &str) -> Result<(), String>;
}

/// A group's log as the store kept it: its entries, numbered on from `after`, the last number a
/// group deleted under the same name issued (0 for a name never deleted). A log holding no
/// entries is a name retired, with no group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Log {
    pub after: u64,
    pub entries: Vec<Entry>,
}

impl From<Vec<Entry>> for Log {
    fn from(entries: Vec<Entry>) -> Log {
        Log { after: 0, entries }
    }
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
    /// Groups whose log was retired, with the last number each issued, in order.
    pub retired: Vec<(String, u64)>,
    /// Groups whose log was removed, in order.
    pub removed: Vec<String>,
    /// What a disk would hold now: each log as appended, retired and removed.
    pub kept: BTreeMap<String, Log>,
    /// Set to make every call fail, as a full disk would.
    pub failing: bool,
    /// Set to make only saving the state fail, as a disk filling up between two writes would.
    pub failing_saves: bool,
}

impl Store for Memory {
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String> {
        if self.failing {
            return Err("the store is failing on purpose".to_string());
        }
        self.appended.push((group.to_string(), entry.clone()));
        self.kept.entry(group.to_string()).or_default().entries.push(entry.clone());
        Ok(())
    }

    fn save(&mut self, state: &Saved) -> Result<(), String> {
        if self.failing || self.failing_saves {
            return Err("the store is failing on purpose".to_string());
        }
        self.saved = Some(state.clone());
        self.saves += 1;
        Ok(())
    }

    fn retire(&mut self, group: &str, last: u64) -> Result<(), String> {
        if self.failing {
            return Err("the store is failing on purpose".to_string());
        }
        self.retired.push((group.to_string(), last));
        self.kept.insert(group.to_string(), Log { after: last, entries: Vec::new() });
        Ok(())
    }

    fn remove(&mut self, group: &str) -> Result<(), String> {
        if self.failing {
            return Err("the store is failing on purpose".to_string());
        }
        self.removed.push(group.to_string());
        self.kept.remove(group);
        Ok(())
    }
}

impl Memory {
    /// The logs a service restored from this store would read.
    pub fn logs(&self) -> BTreeMap<String, Log> {
        self.kept.clone()
    }
}
