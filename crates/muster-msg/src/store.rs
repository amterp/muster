use serde::{Deserialize, Serialize};

use crate::{Entry, HumanHome, Participant, Policy};

/// Where the host keeps what must outlive it. Both calls happen before the request that caused
/// them is answered, so an answer never reports something the store did not take.
pub trait Store {
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String>;
    fn save(&mut self, state: &Saved) -> Result<(), String>;
    /// Forgets a deleted group's log. A log already gone is not an error.
    fn remove(&mut self, group: &str) -> Result<(), String>;
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
    /// Groups whose log was removed, in order.
    pub removed: Vec<String>,
    /// Set to make every call fail, as a full disk would.
    pub failing: bool,
}

impl Store for Memory {
    fn append(&mut self, group: &str, entry: &Entry) -> Result<(), String> {
        if self.failing {
            return Err("the store is failing on purpose".to_string());
        }
        self.appended.push((group.to_string(), entry.clone()));
        Ok(())
    }

    fn save(&mut self, state: &Saved) -> Result<(), String> {
        if self.failing {
            return Err("the store is failing on purpose".to_string());
        }
        self.saved = Some(state.clone());
        self.saves += 1;
        Ok(())
    }

    fn remove(&mut self, group: &str) -> Result<(), String> {
        if self.failing {
            return Err("the store is failing on purpose".to_string());
        }
        self.removed.push(group.to_string());
        Ok(())
    }
}
