//! A pane as somebody types it: its id (`p29te12q4t`), its name (`otter`), or its name and the
//! machine holding it (`otter@devenv`).
//!
//! An id is unique across machines and never reused, so it answers on its own. A name is unique
//! only among one daemon's live panes, which is what makes it short: two machines can each have
//! an `otter`, and the machine then says which. Pure, so that a window and a CLI with no window
//! read a typed pane the same way; each hands over the panes it can see.

/// A pane that typed text might mean.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub pane: &'a str,
    /// What it is called (`crate::roster::pane_name`), if anything.
    pub name: Option<&'a str>,
    /// The machine holding it, as a window names its daemons (`local`, `devenv`).
    pub machine: &'a str,
}

/// What typed text turned out to mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// The pane a name means, by its id.
    Pane(String),
    /// The text goes on as typed: it is an id already, or nothing answers to it - and then
    /// whatever refuses it says what it always has, while a pane made a moment ago, which
    /// nothing here has heard of yet, is still reached by its id.
    AsTyped,
    /// More than one pane answers to it, and why that is refused, in words.
    Ambiguous(String),
}

/// What `typed` means among `candidates`: an id first, then a name, then `name@machine`.
///
/// A name is matched whole before it is split at its last `@`, so a pane somebody called
/// `me@work` is still reached by that. Exactly as typed: a name is matched with its case, and
/// never by a prefix of it.
pub fn resolve(typed: &str, candidates: &[Candidate<'_>]) -> Address {
    if typed.is_empty() || candidates.iter().any(|candidate| candidate.pane == typed) {
        return Address::AsTyped;
    }
    let called: Vec<&Candidate<'_>> =
        candidates.iter().filter(|candidate| candidate.name == Some(typed)).collect();
    let called = if called.is_empty() {
        let Some((name, machine)) = typed.rsplit_once('@') else { return Address::AsTyped };
        candidates
            .iter()
            .filter(|candidate| candidate.name == Some(name) && candidate.machine == machine)
            .collect()
    } else {
        called
    };
    match called.as_slice() {
        [] => Address::AsTyped,
        [one] => Address::Pane(one.pane.to_string()),
        several => Address::Ambiguous(ambiguous(typed, several)),
    }
}

fn ambiguous(typed: &str, several: &[&Candidate<'_>]) -> String {
    let each: Vec<String> = several
        .iter()
        .map(|candidate| {
            format!(
                "{}@{} ({})",
                candidate.name.unwrap_or_default(),
                candidate.machine,
                candidate.pane
            )
        })
        .collect();
    format!(
        "{} panes are called {typed:?}: {}. Say which by its id, or as name@machine when they are \
         on different machines; `muster window` lists them",
        several.len(),
        each.join(", "),
    )
}
