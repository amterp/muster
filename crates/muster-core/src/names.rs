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
//! A name is `p` and nine characters - `p1w3r07bsd` - of which the first five say when the pane
//! was made, to the nearest ten seconds, and the last four keep panes made within one of those
//! apart. So names sort into the order their panes were made, and two Musters that never speak
//! to each other cannot mint one name unless they mint within ten seconds of each other.
//!
//! **Names are globally unique, which is a property and not an accident.** Two daemons both
//! hand out `w1:p1`, so a bare backend id stops being an answer the moment a window shows two
//! machines. A minted name is an answer on its own, which is what lets a CLI reach a pane on
//! the devenv without knowing which daemon holds it.
//!
//! **Never reused**, so a name that outlives its pane resolves to nothing rather than to
//! somebody else's work.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flexid::{Alphabet, Generator, OsRandom, RandomError, RandomSource};

use crate::composition::DaemonId;
use crate::diagnostics::{monotonic_now, poison};
use crate::intent::Refusal;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mint {
    /// The machine's clock and the machine's entropy, which is what a running Muster uses.
    Drawn,

    /// The same spelling at an instant the caller fixes, with entropy from a seed.
    ///
    /// flexid takes both impure inputs as arguments, so the same instant and the same seed
    /// always produce the same name. That is what lets a conformance case pin the name a draw
    /// produces rather than only the shape one has, which matters because a name that changed
    /// shape between versions would strand every pane already carrying one in its
    /// environment.
    Replayed { at: SystemTime, seed: u64 },

    /// The backend's own id, verbatim.
    ///
    /// What every conformance case that is *about* something else is driven with. A case
    /// pinning what the mirror does with `w1:p1` is testing the mirror, and a minted name in
    /// its expectations would make it test the mint as well - so the drivers name a pane
    /// after the backend and the naming has cases of its own.
    ///
    /// It gives up the one property a minted name has and a backend id does not: two daemons
    /// both hand out `w1:p1`, so under this mint they collide. That is why it is a mint for
    /// cases about something else, and never one Muster runs in.
    Backend,
}

impl Mint {
    /// The next name, which the caller still has to check for a collision.
    fn draw(&mut self, backend: &BackendPaneId) -> String {
        match self {
            Mint::Backend => backend.to_string(),
            Mint::Drawn => spell(SystemTime::now(), &mut OsRandom),
            Mint::Replayed { at, seed } => {
                // Zero is xorshift's fixed point, so a case that gave it would draw one name
                // over and over and exhaust the collision retries instead of saying why.
                if *seed == 0 {
                    *seed = 1;
                }
                spell(*at, &mut Seeded(seed))
            }
        }
    }
}

/// Crockford's base32: no `i`, `l`, `o` or `u`, so nothing reads as something else.
///
/// Lowercase, rather than flexid's uppercase [`Alphabet::CROCKFORD_BASE32`], because a name is
/// something somebody types after `--pane`. Still in ascending byte order, which is what
/// flexid needs for names to sort.
const ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// How a name is spelled, decided once.
///
/// The same generator for a running Muster and for a replayed case, so that a name pinned in
/// the corpus is a name Muster actually mints.
///
/// **A 2026 epoch and ten-second ticks** are chosen together, and the pairing is the whole
/// trick. flexid does not pad the tick count, so a name grows a character each time the count
/// crosses a power of 32 - and across that boundary the shorter old names sort *after* the
/// longer new ones. Ten-second ticks from 2026 hold the count at five characters from May 2026
/// until **August 2036**, which is long enough to say plainly what a name is. One-second ticks
/// would have crossed in January 2027.
///
/// **Four random characters** is 1,048,576 names per tick, and they only have to cover two
/// Musters minting in the same ten seconds without talking to each other: within one, the
/// registry below re-draws on a collision.
// Seconds rather than the days clippy prefers: 1767225600 is a Unix timestamp, which a reader
// can recognize and look up. 20454 days is a number nobody can place.
#[allow(clippy::duration_suboptimal_units)]
fn spelling() -> &'static Generator {
    static SPELLING: LazyLock<Generator> = LazyLock::new(|| {
        Generator::builder()
            // 2026-01-01, a few months before Muster's first pane. Everything before it is
            // time no name has to spend characters encoding.
            .epoch(UNIX_EPOCH + Duration::from_secs(1_767_225_600))
            .tick_size(Duration::from_secs(10))
            .alphabet(Alphabet::new(ALPHABET).expect("base32 is 32 distinct ASCII characters"))
            .random_chars(4)
            .build()
            .expect("a ten-second tick is not zero")
    });
    &SPELLING
}

/// `p`, and then what flexid says for this instant.
///
/// The `p` says the name is a pane's, so that a name never reads as the position number the
/// sidebar shows beside it.
///
/// The instant is clamped to the epoch rather than passed through, because a machine whose
/// clock is set before 2026 would otherwise be refused a name outright - and a pane with no
/// name is a split missing from the window for no stated reason. Those names all sit in tick
/// zero: still distinct, they just stop saying when.
fn spell(at: SystemTime, entropy: &mut impl RandomSource) -> String {
    let generator = spelling();
    let at = at.max(generator.epoch());
    let drawn = generator
        .generate_at(at, entropy)
        .or_else(|_| generator.generate_at(at, &mut FromTheClock))
        // Unreachable: the fallback reads no OS entropy, the clamp rules out an instant
        // before the epoch, and one-second ticks cannot overflow a u64 this side of the heat
        // death. An empty name would be caught by the collision check either way.
        .unwrap_or_default();
    format!("p{drawn}")
}

/// A last resort when the machine will not hand over entropy.
///
/// `getrandom` does not fail on any platform Muster runs on. It is caught anyway because
/// naming is not something this can decline to do: every path that sees a pane arrives here,
/// so a refusal would cost a split rather than a character. The monotonic clock reads in tens
/// of nanoseconds, so two reads differ in their low byte, and the registry's collision check
/// covers the rest.
struct FromTheClock;

impl RandomSource for FromTheClock {
    fn fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandomError> {
        for byte in dest.iter_mut() {
            *byte = u8::try_from(monotonic_now() & 0xff).unwrap_or_default();
        }
        Ok(())
    }
}

/// Entropy a case can reproduce, standing in for the machine's.
///
/// xorshift64*, which is four lines and does not have to be strong: what a replayed draw needs
/// is that one seed always gives one sequence, not that a name is unguessable. The socket a
/// name is spoken over is already the user's own.
struct Seeded<'a>(&'a mut u64);

impl RandomSource for Seeded<'_> {
    fn fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandomError> {
        for byte in dest.iter_mut() {
            *self.0 ^= *self.0 >> 12;
            *self.0 ^= *self.0 << 25;
            *self.0 ^= *self.0 >> 27;
            // The top byte, because xorshift64*'s low bits are its weakest.
            *byte =
                u8::try_from(self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56).unwrap_or_default();
        }
        Ok(())
    }
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

impl Default for PaneNames {
    /// A registry that mints, which is the only mint a running Muster has any business in.
    /// The other two are chosen explicitly, by a test or by a conformance driver.
    fn default() -> PaneNames {
        PaneNames::new(Mint::Drawn)
    }
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
    ///
    /// Scoped to one daemon on purpose: a name belonging to the devenv resolves to nothing
    /// when the laptop is asked, which is what stops a request going out to the wrong machine
    /// about an id that machine happens to also use.
    pub fn backend(&self, daemon: &DaemonId, name: &PaneId) -> Option<BackendPaneId> {
        if let Some(located) = self.located.get(name).filter(|located| &located.daemon == daemon) {
            return Some(located.backend.clone());
        }
        // Under the backend mint a name *is* an id, so the mapping is the identity in both
        // directions and a pane nothing has seen yet still resolves. That is what lets a
        // conformance case pin the request Muster builds for `w1:p1` without first walking an
        // event that mentions it.
        matches!(self.mint, Mint::Backend).then(|| BackendPaneId::new(name.as_str()))
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

/// The registry as one daemon's adapter sees it: shared, and scoped to that daemon.
///
/// One registry serves every attached machine, because a name has to be unique across all of
/// them - but each adapter only translates its own daemon's ids, so the daemon is carried here
/// rather than repeated at twenty call sites. Cloning shares the registry.
///
/// Locked because two threads mint: a daemon's events are decoded on the subscription thread
/// and its requests are built on whichever thread dispatched them.
#[derive(Debug, Clone)]
pub struct Names {
    daemon: DaemonId,
    panes: Arc<Mutex<PaneNames>>,
}

impl Names {
    pub fn new(daemon: DaemonId, panes: Arc<Mutex<PaneNames>>) -> Names {
        Names { daemon, panes }
    }

    /// A registry of its own, for a caller that has no session to share one with.
    ///
    /// What the conformance drivers translate through, under [`Mint::Backend`], so that a case
    /// pinning what the mirror does with `w1:p1` says `w1:p1` and is testing the mirror.
    pub fn alone(daemon: &str, mint: Mint) -> Names {
        Names::new(DaemonId::new(daemon), Arc::new(Mutex::new(PaneNames::new(mint))))
    }

    /// What Muster calls this pane of this daemon's, naming it now if nobody has.
    pub fn name(&self, backend: &str) -> PaneId {
        self.locked().name(&self.daemon, &BackendPaneId::new(backend))
    }

    /// What this daemon calls the pane Muster calls `name`, or why it cannot say.
    ///
    /// A refusal rather than the name passed through, because herdr ignores a `target_pane_id`
    /// it does not recognize and splits whatever it has focused instead - so a name sent
    /// hopefully would land a pane in an arbitrary place and report success. `NotThere` is the
    /// same answer the daemon gives for a pane it has dropped, and means the same thing here:
    /// whoever said this name is talking about something that is not there.
    pub fn backend(&self, name: &PaneId) -> Result<BackendPaneId, Refusal> {
        self.locked()
            .backend(&self.daemon, name)
            .ok_or_else(|| Refusal::NotThere(format!("no pane called {name} on {}", self.daemon)))
    }

    /// A name for a pane this daemon is about to be asked to make.
    pub fn reserve(&self) -> PaneId {
        self.locked().reserve()
    }

    /// Says which of this daemon's panes a reserved name turned out to be.
    pub fn settle(&self, name: &PaneId, backend: &str) {
        self.locked().settle(name, &self.daemon, &BackendPaneId::new(backend));
    }

    /// Gives back a name whose pane the daemon never made.
    pub fn release(&self, name: &PaneId) {
        self.locked().release(name);
    }

    /// Forgets the names of panes this daemon no longer holds.
    pub fn prune(&self, held: impl IntoIterator<Item = BackendPaneId>) {
        self.locked().prune(&self.daemon, &held.into_iter().collect());
    }

    fn locked(&self) -> MutexGuard<'_, PaneNames> {
        poison::lock(&self.panes, "pane-names")
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
