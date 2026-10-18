//! A pane as somebody types it: its id (`p29te12q4t`), its callsign (`otter`), a name somebody
//! gave it, or either of those and the machine holding it (`otter@devenv`).
//!
//! An id is unique across machines and never reused, so it answers on its own. A callsign is
//! unique among one daemon's live panes, which is what makes it short: two machines can each have
//! an `otter`, and the machine then says which. A given name need not be unique at all, so it is
//! asked after the callsigns, and one several panes share is refused naming each by its
//! callsign. Pure, so that a window and a CLI with no window read a typed pane the same way; each
//! hands over the panes it can see.

/// A pane that typed text might mean.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub pane: &'a str,
    /// The name somebody gave it, or else its callsign (`crate::roster::pane_name`).
    pub name: Option<&'a str>,
    /// Its callsign (`otter`), which it answers to whatever it is named.
    pub callsign: Option<&'a str>,
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

/// What `typed` means among `candidates`: an id first, then a callsign, then a given name, and
/// then each of those again as `name@machine`.
///
/// A name is matched whole before it is split at its last `@`, so a pane somebody called
/// `me@work` is still reached by that. Exactly as typed: a name is matched with its case, and
/// never by a prefix of it.
pub fn resolve(typed: &str, candidates: &[Candidate<'_>]) -> Address {
    if typed.is_empty() || candidates.iter().any(|candidate| candidate.pane == typed) {
        return Address::AsTyped;
    }
    let anywhere = |_: &Candidate<'_>| true;
    let mut called = answering(typed, candidates, anywhere);
    if called.is_empty()
        && let Some((name, machine)) = typed.rsplit_once('@')
    {
        called = answering(name, candidates, |candidate| candidate.machine == machine);
    }
    match called.as_slice() {
        [] => Address::AsTyped,
        [one] => Address::Pane(one.pane.to_string()),
        several => Address::Ambiguous(ambiguous(typed, several)),
    }
}

/// The candidates `filter` keeps that have `name` as their callsign, or failing any, as the
/// name somebody gave them.
fn answering<'c, 'a>(
    name: &str,
    candidates: &'c [Candidate<'a>],
    filter: impl Fn(&Candidate<'a>) -> bool,
) -> Vec<&'c Candidate<'a>> {
    let kept = || candidates.iter().filter(|candidate| filter(candidate));
    let callsigns: Vec<_> = kept().filter(|candidate| candidate.callsign == Some(name)).collect();
    if !callsigns.is_empty() {
        return callsigns;
    }
    kept().filter(|candidate| candidate.name == Some(name)).collect()
}

fn ambiguous(typed: &str, several: &[&Candidate<'_>]) -> String {
    let each: Vec<String> = several
        .iter()
        .map(|candidate| {
            let callsign = candidate.callsign.or(candidate.name).unwrap_or_default();
            format!("{callsign}@{} ({})", candidate.machine, candidate.pane)
        })
        .collect();
    format!(
        "{} panes are called {typed:?}: {}. Say which by its callsign, with the machine when two \
         machines share it, or by its id; `muster window` lists them",
        several.len(),
        each.join(", "),
    )
}
