//! Muster's own name for a pane, and what the backend calls the same thing.
//!
//! Every other noun in `mirror::backend` is Muster's word for a backend's concept; a pane's
//! *name* is Muster's outright. The reason is not tidiness - it is that Muster has to be able
//! to tell a pane which pane it is, and a backend's id arrives too late for that. herdr
//! assigns `w1:p3` in its *answer* to `pane.split`, and the environment a new pane is born
//! with has to be sent with the request, so there is no moment where Muster holds both. A name
//! Muster mints itself can go into the request that creates the pane and be bound to whatever
//! comes back.
//!
//! That is what lets an agent in a pane say "below me" (`architecture.md`, one action path),
//! and it is the general form of a rule the daemon half of a pane's name has always followed:
//! Muster names the things it has to be able to talk about, rather than being limited to what
//! a dependency happens to hand out.
//!
//! **Names are globally unique, which is a property and not an accident.** Two daemons both
//! hand out `w1:p1`, so a bare backend id stops being an answer the moment a window shows two
//! machines. A minted name is an answer on its own, which is what lets a CLI reach a pane on
//! the devenv without knowing which daemon holds it.
//!
//! **Never reused**, so a name that outlives its pane resolves to nothing rather than to
//! somebody else's work.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::DaemonId;
use crate::mirror::backend::PaneId;

/// What a backend calls a pane.
///
/// Its own type rather than a `String`, because from here on the two spellings travel
/// together and passing one where the other belongs is a lookup that quietly finds nothing -
/// which is the failure `mirror::backend`'s id types already exist to prevent, arrived at
/// from the other side. Opaque: Muster never parses it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BackendPaneId(String);

impl BackendPaneId {
    pub fn new(id: impl Into<String>) -> BackendPaneId {
        BackendPaneId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BackendPaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for BackendPaneId {
    fn from(id: &str) -> BackendPaneId {
        BackendPaneId(id.to_string())
    }
}

/// Where a name's pane actually is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Located {
    pub daemon: DaemonId,
    pub backend: BackendPaneId,
}

/// Where the next name comes from.
///
/// An injected edge rather than something read here, on the same terms as the clock: drawing
/// entropy is the shell's to do, and a core that did it for itself could not be replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mint {
    /// Names drawn from a seed the caller supplies, which is what a running Muster uses.
    ///
    /// Seeded rather than random per draw so that a run is reproducible from its log: the
    /// same seed always produces the same sequence of names.
    Drawn { state: u64 },

    /// The backend's own id, verbatim.
    ///
    /// What every conformance case that is *about* something else is driven with. A case
    /// pinning what the mirror does with `w1:p1` is testing the mirror, and a minted name in
    /// its expectations would make it test the mint as well - so the drivers name a pane
    /// after the backend and the naming has cases of its own.
    ///
    /// It gives up the one property a drawn name has and a backend id does not: two daemons
    /// both hand out `w1:p1`, so under this mint they collide. That is why it is a mint for
    /// cases about something else, and never one Muster runs in.
    Backend,
}

impl Mint {
    /// The next name, which the caller still has to check for a collision.
    fn draw(&mut self, backend: &BackendPaneId) -> String {
        match self {
            Mint::Backend => backend.to_string(),
            Mint::Drawn { state } => {
                // xorshift64*, which is six lines and good enough for thirty bits of name.
                // What is wanted here is that two Musters writing one state file do not
                // collide, not that a name is unguessable - the socket a name is spoken over
                // is already the user's own.
                *state ^= *state >> 12;
                *state ^= *state << 25;
                *state ^= *state >> 27;
                spell(state.wrapping_mul(0x2545_f491_4f6c_dd1d))
            }
        }
    }
}

/// How many characters a drawn name has after its `p`.
///
/// Six, at five bits each, is thirty bits. A window nobody can fill past about fifteen panes
/// draws a handful of these a day, so the birthday odds are nowhere - and it is short enough
/// to type, to fit in a log line, and to read back over somebody's shoulder.
const DRAWN_LENGTH: usize = 6;

/// Crockford's base32: no `i`, `l`, `o` or `u`, so nothing reads as something else.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A drawn name, spelled.
///
/// Pure, and separate from the draw, so that a case can pin what a number is spelled as
/// without pinning how the number was arrived at.
fn spell(value: u64) -> String {
    let mut name = String::with_capacity(DRAWN_LENGTH + 1);
    name.push('p');
    for character in 0..DRAWN_LENGTH {
        // Five bits at a time from the top, so the whole word contributes rather than the
        // low bits alone - a generator with weak low bits would otherwise show through.
        let shift = 64 - 5 * (character + 1);
        let index = usize::try_from((value >> shift) & 0b1_1111).unwrap_or_default();
        name.push(char::from(ALPHABET[index]));
    }
    name
}

/// Every pane Muster has a name for.
///
/// Both directions, because both are asked: outward, to turn a name a caller said into the id
/// a backend understands; inward, to turn what a daemon just said into the name everything
/// above the adapter uses.
#[derive(Debug)]
pub struct PaneNames {
    mint: Mint,
    located: BTreeMap<PaneId, Located>,
    named: BTreeMap<Located, PaneId>,
    /// Names handed out for panes that do not exist yet.
    ///
    /// A pane is named *before* it is asked for, so there is a moment where a name has been
    /// spoken - it is in the request, and about to be in the pane's environment - and nothing
    /// answers to it. Held here so a second draw in that moment cannot land on it.
    reserved: BTreeSet<PaneId>,
}

impl PaneNames {
    pub fn new(mint: Mint) -> PaneNames {
        PaneNames {
            mint,
            located: BTreeMap::new(),
            named: BTreeMap::new(),
            reserved: BTreeSet::new(),
        }
    }

    /// What Muster calls this pane, naming it now if nobody has.
    ///
    /// Total on purpose. Every path that reads a backend's payload arrives here, and a pane
    /// that could not be named would be a pane the mirror silently drops - which renders as a
    /// window missing a split for no stated reason. Naming on first sight also means a pane
    /// another client made is addressable, even though nothing can tell it its own name.
    pub fn name(&mut self, daemon: &DaemonId, backend: &BackendPaneId) -> PaneId {
        let at = Located { daemon: daemon.clone(), backend: backend.clone() };
        if let Some(name) = self.named.get(&at) {
            return name.clone();
        }
        let name = self.draw(backend);
        self.bind(name.clone(), at);
        name
    }

    /// A name for a pane that does not exist yet.
    ///
    /// The whole reason this registry exists. What it is for is the `env` of the request that
    /// creates the pane, so the pane is born knowing what to call itself; the caller binds it
    /// when the backend answers, or releases it when the backend refuses.
    pub fn reserve(&mut self) -> PaneId {
        // The backend has no id for it yet, and `Mint::Backend` has nothing to echo - so a
        // reservation under that mint is spelled for what it is. Conformance cases drive
        // creation through the daemon that answers, so this is only ever seen by a case
        // written about reservation itself.
        let name = self.draw(&BackendPaneId::new(format!("reserved-{}", self.reserved.len())));
        self.reserved.insert(name.clone());
        name
    }

    /// Says where a reserved name's pane turned out to be.
    pub fn settle(&mut self, name: &PaneId, daemon: &DaemonId, backend: &BackendPaneId) {
        self.reserved.remove(name);
        self.bind(name.clone(), Located { daemon: daemon.clone(), backend: backend.clone() });
    }

    /// Gives back a name whose pane was never made.
    pub fn release(&mut self, name: &PaneId) {
        self.reserved.remove(name);
    }

    /// Where this name's pane is, or nothing at all.
    ///
    /// Nothing is the ordinary answer for a name whose pane has closed, and the CLI turns it
    /// into a refusal that says so - which is the only honest thing to do with a name that
    /// used to mean something.
    pub fn locate(&self, name: &PaneId) -> Option<&Located> {
        self.located.get(name)
    }

    /// What this daemon calls the pane Muster calls `name`.
    pub fn backend(&self, daemon: &DaemonId, name: &PaneId) -> Option<&BackendPaneId> {
        self.located
            .get(name)
            .filter(|located| &located.daemon == daemon)
            .map(|located| &located.backend)
    }

    /// Forgets the panes a daemon no longer holds.
    ///
    /// Driven by what the daemon says it has rather than by a name going unused, and scoped to
    /// one daemon, because that is the only question this can answer safely: a name belonging
    /// to a daemon nothing is attached to is not gone, it is unwitnessed - and dropping it
    /// would strand an agent that is still running in it.
    pub fn prune(&mut self, daemon: &DaemonId, held: &BTreeSet<BackendPaneId>) {
        self.located
            .retain(|_, located| &located.daemon != daemon || held.contains(&located.backend));
        self.named
            .retain(|located, _| &located.daemon != daemon || held.contains(&located.backend));
    }

    /// Every name, and where its pane is. In name order, so two writes of an unchanged
    /// registry produce the same bytes.
    pub fn entries(&self) -> impl Iterator<Item = (&PaneId, &Located)> {
        self.located.iter()
    }

    fn bind(&mut self, name: PaneId, at: Located) {
        self.located.insert(name.clone(), at.clone());
        self.named.insert(at, name);
    }

    /// A name nothing else answers to.
    ///
    /// Re-drawn on a collision rather than accepted, which is what "never reused" costs. The
    /// loop is bounded because it has to be: `Mint::Backend` draws the same string every time,
    /// so a case that names one pane twice would otherwise spin forever - and the second name
    /// wants to be the same one anyway.
    fn draw(&mut self, backend: &BackendPaneId) -> PaneId {
        for _ in 0..64 {
            let drawn = PaneId::new(self.mint.draw(backend));
            if !self.located.contains_key(&drawn) && !self.reserved.contains(&drawn) {
                return drawn;
            }
            if matches!(self.mint, Mint::Backend) {
                return drawn;
            }
        }
        // Sixty-four collisions in a row is not a state to recover from, and inventing a name
        // that is already in use would hand one pane's keystrokes to another.
        panic!("could not draw a pane name nothing else answers to after 64 tries");
    }
}

/// The version this format is on.
///
/// Read on the same terms as the saved arrangement's: a file this Muster does not understand
/// is ignored rather than guessed at. What that costs is that panes made before the upgrade
/// cannot say which pane they are until they are made again - everything else about them works.
const VERSION: i64 = 1;

/// Names, written down.
///
/// Panes outlive the app, so an agent that has been running for hours has to still be able to
/// resolve its own name after Muster has been quit and reopened. Without this the whole
/// mechanism would last exactly one launch.
///
/// TOML for the reason the arrangement beside it is TOML: one format to learn, and a file
/// somebody opens when a name stops resolving.
pub fn to_toml(names: &PaneNames) -> String {
    let mut root = toml::Table::new();
    root.insert("version".to_string(), toml::Value::Integer(VERSION));

    let panes: Vec<toml::Value> = names
        .entries()
        .map(|(name, at)| {
            let mut table = toml::Table::new();
            table.insert("name".to_string(), toml::Value::String(name.to_string()));
            table.insert("daemon".to_string(), toml::Value::String(at.daemon.to_string()));
            table.insert("backend".to_string(), toml::Value::String(at.backend.to_string()));
            toml::Value::Table(table)
        })
        .collect();
    if !panes.is_empty() {
        root.insert("pane".to_string(), toml::Value::Array(panes));
    }

    toml::to_string_pretty(&toml::Value::Table(root))
        .unwrap_or_else(|error| panic!("pane names should always render as TOML: {error}"))
}

/// Reads names back, or says why it will not.
///
/// Every refusal ends the same way - the panes that were already open keep working and stop
/// being able to say which pane they are - so the message is for whoever is reading a log
/// wondering why `$MUSTER_PANE` stopped resolving.
pub fn from_toml(text: &str, mint: Mint) -> Result<PaneNames, String> {
    let root: toml::Table = toml::from_str(text)
        .map_err(|error| format!("the saved pane names are not TOML: {error}"))?;

    match root.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "the saved pane names are version {other} and this Muster writes version \
                 {VERSION}. They are ignored rather than guessed at, so panes made before now \
                 cannot say which pane they are until they are made again."
            ));
        }
        None => return Err("the saved pane names do not say what version they are".to_string()),
    }

    let mut names = PaneNames::new(mint);
    for entry in root.get("pane").and_then(toml::Value::as_array).into_iter().flatten() {
        // An entry that will not read is skipped rather than failing the file, on the same
        // terms as a saved region: one unreadable line costs one pane's name.
        let Some(table) = entry.as_table() else { continue };
        let (Some(name), Some(daemon), Some(backend)) =
            (text_at(table, "name"), text_at(table, "daemon"), text_at(table, "backend"))
        else {
            continue;
        };
        names.bind(
            PaneId::new(name),
            Located { daemon: DaemonId::new(daemon), backend: BackendPaneId::new(backend) },
        );
    }
    Ok(names)
}

fn text_at(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().filter(|value| !value.is_empty()).map(str::to_string)
}
