//! Muster's own names for panes and tabs, and what the backend calls the same things.
//!
//! Every other noun in `mirror::backend` is Muster's word for a backend's concept; these two
//! are Muster's outright. One mechanism, two nouns, and they are named for different reasons -
//! which is worth stating, because the difference is what the pane half carries and the tab
//! half does not.
//!
//! **A pane is named so that it can be told which pane it is.** A backend's id arrives too
//! late for that: herdr assigns `w1:p3` in its *answer* to `pane.split`, and the environment a
//! new pane is born with has to be sent with the request, so there is no moment where Muster
//! holds both. A name Muster mints itself can go into the request that creates the pane and be
//! bound to whatever comes back. That is what lets an agent in a pane say "below me"
//! (`architecture.md`, one action path), and it is why the registry has
//! [`reserve`](Registry::reserve) at all.
//!
//! **A tab is named so that it can be addressed.** Nothing has to tell a tab which tab it is,
//! so a tab name is minted on first sight rather than before creation and no pane's
//! environment carries one. What it buys is the other property: a tab that a script, a CLI or
//! an agent can name. Without it `muster window` could describe a tab and offer no way to act
//! on it, which is what it did until this grew a tab half.
//!
//! A name is a letter and nine characters - `p1w3r07bsd`, `t1w3r07bsd` - of which the first
//! five say when the thing was made, to the nearest ten seconds, and the last four keep things
//! made within one of those apart. So names sort into the order they were made, and two Musters
//! that never speak to each other cannot mint one name unless they mint within ten seconds of
//! each other. The letter says which noun, so a name never reads as the position number a
//! sidebar draws beside it, and a tab name can never be mistaken for a pane's.
//!
//! **Names are globally unique, which is a property and not an accident.** Two daemons both
//! hand out `w1:p1` and `w1:t1`, so a bare backend id stops being an answer the moment a window
//! shows two machines. A minted name is an answer on its own, which is what lets a CLI reach a
//! pane or a tab on the devenv without knowing which daemon holds it.
//!
//! **Never reused**, so a name that outlives what it named resolves to nothing rather than to
//! somebody else's work.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flexid::{Alphabet, Generator, OsRandom, RandomError, RandomSource};

use crate::composition::DaemonId;
use crate::diagnostics::{monotonic_now, poison};
use crate::intent::Refusal;
use crate::mirror::backend::{PaneId, TabId, id_type};

// The two backend spellings, from the same macro the Muster-side ids come from. Their own types
// rather than `String`s, because from here on the two spellings travel together and passing one
// where the other belongs is a lookup that quietly finds nothing - which is the failure
// `mirror::backend`'s id types already exist to prevent, arrived at from the other side.
id_type!(BackendPaneId, "What a backend calls a pane.");
id_type!(BackendTabId, "What a backend calls a tab.");

/// Where a name's thing actually is.
///
/// Generic over the backend spelling rather than written twice, so a pane's location and a
/// tab's cannot drift apart - and so the registry below is one mechanism.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Located<B> {
    pub daemon: DaemonId,
    pub backend: B,
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
    /// after the backend and the naming has cases of its own. It is also what keeps a tab's
    /// name out of every corpus that was written before tabs had one.
    ///
    /// It gives up the one property a minted name has and a backend id does not: two daemons
    /// both hand out `w1:p1`, so under this mint they collide. That is why it is a mint for
    /// cases about something else, and never one Muster runs in.
    Backend,
}

impl Mint {
    /// The next name, which the caller still has to check for a collision.
    fn draw(&mut self, backend: &impl std::fmt::Display, prefix: char) -> String {
        match self {
            Mint::Backend => backend.to_string(),
            Mint::Drawn => spell(SystemTime::now(), &mut OsRandom, prefix),
            Mint::Replayed { at, seed } => {
                // Zero is xorshift's fixed point, so a case that gave it would draw one name
                // over and over and exhaust the collision retries instead of saying why.
                if *seed == 0 {
                    *seed = 1;
                }
                spell(*at, &mut Seeded(seed), prefix)
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

/// The noun's letter, and then what flexid says for this instant.
///
/// The letter says which noun the name names, so that a name never reads as the position
/// number the sidebar shows beside it, and so that a tab name and a pane name can never be
/// confused for one another by whoever is reading a script.
///
/// The instant is clamped to the epoch rather than passed through, because a machine whose
/// clock is set before 2026 would otherwise be refused a name outright - and a pane with no
/// name is a split missing from the window for no stated reason. Those names all sit in tick
/// zero: still distinct, they just stop saying when.
fn spell(at: SystemTime, entropy: &mut impl RandomSource, prefix: char) -> String {
    let generator = spelling();
    let at = at.max(generator.epoch());
    let drawn = generator
        .generate_at(at, entropy)
        .or_else(|_| generator.generate_at(at, &mut FromTheClock))
        // Unreachable: the fallback reads no OS entropy, the clamp rules out an instant
        // before the epoch, and one-second ticks cannot overflow a u64 this side of the heat
        // death. An empty name would be caught by the collision check either way.
        .unwrap_or_default();
    format!("{prefix}{drawn}")
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

/// The written record of these names, which another Muster may be holding open too.
///
/// **The reason this exists is that a window is a process.** Two Musters attached to one daemon
/// both see every pane it holds, and both would name one nobody had named yet - so the same
/// pane ends up called two things, each window's `muster window` disagrees with the other's,
/// and the one that writes the file last takes the other's bindings with it. Measured, not
/// feared: two windows on one daemon agreed only about the pane that existed before the second
/// one opened.
///
/// So naming something new is done while holding the record, rather than in memory and written
/// out afterwards. `exclusively` is the whole of what the core asks for; where the record lives
/// and how it is locked is the shell's business, like every other file.
pub trait SharedNames: Send + Sync + std::fmt::Debug {
    /// Runs `while_held` with nobody else able to read or write the record.
    ///
    /// `while_held` is given what the record says at that moment and answers with what to write
    /// back, or `None` to leave it alone. A record that cannot be reached does not stop
    /// anything: `while_held` still runs, given nothing, and this Muster names things the way
    /// it did before there was a record to share - which is right, because a window that
    /// refused to name a pane would be a window that cannot draw one.
    fn exclusively(&self, while_held: &mut dyn FnMut(&str) -> Option<String>);

    /// Whether the record has moved since this Muster last read it.
    ///
    /// What keeps the common answer cheap. Naming something this window has already named is
    /// the overwhelming majority of these calls - every pane of every layout the daemon
    /// describes - and taking a lock for each would be a lock per keystroke somebody typed
    /// into an agent. So a name already held is handed straight back, unless the record has
    /// changed underneath, in which case another Muster has written something and this window
    /// takes the hold to find out what.
    ///
    /// Needed because a window can be *wrong* rather than merely ignorant: a pane created in
    /// another window may be seen here, and named here, before that window has settled the
    /// name it already put in the pane's environment. Without this, the guess would stand
    /// forever, since nothing else would ever look at the record again.
    ///
    /// False by default, which is right for a record nothing else writes.
    fn moved(&self) -> bool {
        false
    }
}

/// Every thing of one noun that Muster has a name for.
///
/// Both directions, because both are asked: outward, to turn a name a caller said into the id
/// a backend understands; inward, to turn what a daemon just said into the name everything
/// above the adapter uses.
///
/// One generic mechanism rather than one per noun, because every method here except
/// [`reserve`](Registry::reserve) is the same question about a different noun, and two copies
/// of a rule as subtle as `unannounced` below would be two copies to get right. `N` is what
/// Muster calls one, `B` what the backend does.
#[derive(Debug)]
pub struct Registry<N, B> {
    mint: Mint,
    /// The letter every name from this registry starts with. See [`spell`].
    prefix: char,
    located: BTreeMap<N, Located<B>>,
    named: BTreeMap<Located<B>, N>,
    /// Names handed out for things that do not exist yet. Panes only.
    ///
    /// A pane is named *before* it is asked for, so there is a moment where a name has been
    /// spoken - it is in the request, and about to be in the pane's environment - and nothing
    /// answers to it. Held here so a second draw in that moment cannot land on it. A tab is
    /// named on sight and never reserved, so a tab registry leaves this empty.
    reserved: BTreeSet<N>,
    /// Names bound from a backend's own answer, whose thing it has not described yet.
    ///
    /// A backend names a pane in its reply to the request that made it and announces the pane
    /// a moment later, so in between there is a name whose pane no mirror has heard of. Without
    /// this, [`prune`](Registry::prune) reads that as a pane that has closed and forgets the
    /// name of the pane somebody just made - taking its keyboard and its `MUSTER_PANE` with it.
    ///
    /// Tabs are in the same gap for the same reason: `tab.create` answers with the tab's id and
    /// the tab is announced afterwards.
    ///
    /// Left the first time a prune sees the thing, so it covers that gap and nothing wider: a
    /// name read back from a previous launch was never in here, and a pane that closed while
    /// Muster was shut is forgotten on the first prune the way it should be.
    unannounced: BTreeSet<N>,
}

/// Every pane Muster has a name for.
pub type PaneNames = Registry<PaneId, BackendPaneId>;

/// Every tab Muster has a name for.
pub type TabNames = Registry<TabId, BackendTabId>;

impl PaneNames {
    pub fn new(mint: Mint) -> PaneNames {
        Registry::of(mint, 'p')
    }

    /// A name for a pane that does not exist yet.
    ///
    /// Why the pane half of this registry exists. What it is for is the `env` of the request
    /// that creates the pane, so the pane is born knowing what to call itself; the caller binds
    /// it when the backend answers, or releases it when the backend refuses. A tab has no
    /// equivalent, because nothing has to tell a tab which tab it is.
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
        self.unannounced.insert(name.clone());
        self.bind(name.clone(), Located { daemon: daemon.clone(), backend: backend.clone() });
    }

    /// Gives back a name whose pane was never made.
    pub fn release(&mut self, name: &PaneId) {
        self.reserved.remove(name);
    }
}

impl TabNames {
    pub fn new(mint: Mint) -> TabNames {
        Registry::of(mint, 't')
    }
}

impl Default for PaneNames {
    /// A registry that mints, which is the only mint a running Muster has any business in.
    /// The other two are chosen explicitly, by a test or by a conformance driver.
    fn default() -> PaneNames {
        PaneNames::new(Mint::Drawn)
    }
}

impl Default for TabNames {
    fn default() -> TabNames {
        TabNames::new(Mint::Drawn)
    }
}

impl<N, B> Registry<N, B>
where
    N: Ord + Clone + std::fmt::Display + From<String>,
    B: Ord + Clone + std::fmt::Display + From<String>,
{
    fn of(mint: Mint, prefix: char) -> Registry<N, B> {
        Registry {
            mint,
            prefix,
            located: BTreeMap::new(),
            named: BTreeMap::new(),
            reserved: BTreeSet::new(),
            unannounced: BTreeSet::new(),
        }
    }

    /// What Muster calls this thing, naming it now if nobody has.
    ///
    /// Total on purpose. Every path that reads a backend's payload arrives here, and a pane
    /// that could not be named would be a pane the mirror silently drops - which renders as a
    /// window missing a split for no stated reason. Naming on first sight also means a pane or
    /// a tab another client made is addressable, even though nothing can tell a pane its own
    /// name unless Muster made it.
    pub fn name(&mut self, daemon: &DaemonId, backend: &B) -> N {
        let at = Located { daemon: daemon.clone(), backend: backend.clone() };
        if let Some(name) = self.named.get(&at) {
            return name.clone();
        }
        let name = self.draw(backend);
        self.bind(name.clone(), at);
        name
    }

    /// The same, for something the backend has only just said it made.
    ///
    /// Held against the next prune, because between an answer naming a tab and an event
    /// announcing it there is a moment where the mirror holds nothing by that name - and a
    /// prune in that moment would forget the name of the tab somebody just made. See
    /// `unannounced`.
    pub fn name_from_answer(&mut self, daemon: &DaemonId, backend: &B) -> N {
        let name = self.name(daemon, backend);
        self.unannounced.insert(name.clone());
        name
    }

    /// What this is already called, without naming it if it is not.
    ///
    /// The half of [`name`](Registry::name) that costs nothing, so that the common answer -
    /// something this registry has seen before - is reached without the record two Musters
    /// share ever being opened.
    pub fn known(&self, daemon: &DaemonId, backend: &B) -> Option<N> {
        self.named.get(&Located { daemon: daemon.clone(), backend: backend.clone() }).cloned()
    }

    /// Takes another Muster's word for what things are called.
    ///
    /// Every binding in `theirs` replaces whatever this registry had, rather than being skipped
    /// where the two disagree, and that direction is the point: the record is what both windows
    /// resolve against, so the one that reads it has to end up agreeing with it. A binding this
    /// window had and the record contradicts was minted in the moment before the record was
    /// read, and keeping it would leave two windows permanently disagreeing about one pane.
    ///
    /// Reserved names are not touched. A reserved name is already inside a pane's environment,
    /// so it is the one thing here that cannot be revised - and it reaches the record on its
    /// own settle, which is what makes the other window adopt it rather than the other way
    /// round.
    pub fn adopt(&mut self, theirs: Registry<N, B>) {
        for (name, at) in theirs.located {
            if self.reserved.contains(&name) {
                continue;
            }
            // `bind` takes out whatever the two maps used to say, which is what makes this a
            // replacement rather than a second answer sitting beside the first.
            self.bind(name, at);
        }
    }

    /// Where this name's thing is, or nothing at all.
    ///
    /// Nothing is the ordinary answer for a name whose thing has closed, and the CLI turns it
    /// into a refusal that says so - which is the only honest thing to do with a name that
    /// used to mean something.
    pub fn locate(&self, name: &N) -> Option<&Located<B>> {
        self.located.get(name)
    }

    /// What this daemon calls the thing Muster calls `name`.
    ///
    /// Scoped to one daemon on purpose: a name belonging to the devenv resolves to nothing
    /// when the laptop is asked, which is what stops a request going out to the wrong machine
    /// about an id that machine happens to also use.
    pub fn backend(&self, daemon: &DaemonId, name: &N) -> Option<B> {
        if let Some(located) = self.located.get(name).filter(|located| &located.daemon == daemon) {
            return Some(located.backend.clone());
        }
        // Under the backend mint a name *is* an id, so the mapping is the identity in both
        // directions and a pane nothing has seen yet still resolves. That is what lets a
        // conformance case pin the request Muster builds for `w1:p1` without first walking an
        // event that mentions it.
        matches!(self.mint, Mint::Backend).then(|| B::from(name.to_string()))
    }

    /// Forgets the things a daemon no longer holds.
    ///
    /// Driven by what the daemon says it has rather than by a name going unused, and scoped to
    /// one daemon, because that is the only question this can answer safely: a name belonging
    /// to a daemon nothing is attached to is not gone, it is unwitnessed - and dropping it
    /// would strand an agent that is still running in it.
    ///
    /// Something the daemon has not got round to describing is not gone either, which is the
    /// whole job of `unannounced` above: absence from `held` means "closed" only for a thing
    /// this registry has seen the daemon hold at least once.
    pub fn prune(&mut self, daemon: &DaemonId, held: &BTreeSet<B>) {
        // Taken out and put back because the retains below read it while borrowing the maps it
        // is filtered against.
        let mut unannounced = std::mem::take(&mut self.unannounced);
        unannounced.retain(|name| !self.holds(daemon, held, name));
        self.located.retain(|name, at| {
            &at.daemon != daemon || held.contains(&at.backend) || unannounced.contains(name)
        });
        self.named.retain(|at, name| {
            &at.daemon != daemon || held.contains(&at.backend) || unannounced.contains(name)
        });
        self.unannounced = unannounced;
    }

    /// Whether this daemon is holding the thing it calls by this name.
    fn holds(&self, daemon: &DaemonId, held: &BTreeSet<B>, name: &N) -> bool {
        self.located.get(name).is_some_and(|at| &at.daemon == daemon && held.contains(&at.backend))
    }

    /// Every name, and where its thing is. In name order, so two writes of an unchanged
    /// registry produce the same bytes.
    pub fn entries(&self) -> impl Iterator<Item = (&N, &Located<B>)> {
        self.located.iter()
    }

    /// Binds a name to a thing, and takes out whatever the two used to say.
    ///
    /// The two maps are one fact read from either end, so a stale entry in one of them is a
    /// registry that answers differently depending which way it is asked. That became
    /// reachable when a second window could name a pane this one has already named: settling a
    /// reserved name over a guessed one left the guess in `located`, both names went into the
    /// record, and the next window to read it adopted whichever came first.
    fn bind(&mut self, name: N, at: Located<B>) {
        if let Some(previous) = self.named.remove(&at) {
            self.located.remove(&previous);
        }
        if let Some(previous) = self.located.remove(&name) {
            self.named.remove(&previous);
        }
        self.located.insert(name.clone(), at.clone());
        self.named.insert(at, name);
    }

    /// A name nothing else answers to.
    ///
    /// Re-drawn on a collision rather than accepted, which is what "never reused" costs. The
    /// loop is bounded because it has to be: `Mint::Backend` draws the same string every time,
    /// so a case that names one thing twice would otherwise spin forever - and the second name
    /// wants to be the same one anyway.
    fn draw(&mut self, backend: &B) -> N {
        for _ in 0..64 {
            let drawn = N::from(self.mint.draw(backend, self.prefix));
            if !self.located.contains_key(&drawn) && !self.reserved.contains(&drawn) {
                return drawn;
            }
            if matches!(self.mint, Mint::Backend) {
                return drawn;
            }
        }
        // Sixty-four collisions in a row is not a state to recover from, and inventing a name
        // that is already in use would hand one pane's keystrokes to another.
        panic!("could not draw a name nothing else answers to after 64 tries");
    }
}

/// The registries as one daemon's adapter sees them: shared, and scoped to that daemon.
///
/// One registry per noun serves every attached machine, because a name has to be unique across
/// all of them - but each adapter only translates its own daemon's ids, so the daemon is carried
/// here rather than repeated at forty call sites. Cloning shares the registries.
///
/// Locked because two threads mint: a daemon's events are decoded on the subscription thread
/// and its requests are built on whichever thread dispatched them. Two locks rather than one
/// over both, so decoding a pane event never waits on a tab being named. Nothing here takes
/// both; the one caller that needs both is the save, and it takes panes first.
#[derive(Debug, Clone)]
pub struct Names {
    daemon: DaemonId,
    panes: Arc<Mutex<PaneNames>>,
    tabs: Arc<Mutex<TabNames>>,
    /// The record another Muster may be naming things in at the same moment.
    ///
    /// `None` is a registry with nowhere to write - a conformance driver, or a window told to
    /// remember nothing - and it names things exactly as it did before this existed.
    shared: Option<Arc<dyn SharedNames>>,
}

impl Names {
    pub fn new(
        daemon: DaemonId,
        panes: Arc<Mutex<PaneNames>>,
        tabs: Arc<Mutex<TabNames>>,
    ) -> Names {
        Names { daemon, panes, tabs, shared: None }
    }

    /// The same, sharing its record with whatever else is holding that record open.
    pub fn sharing(
        daemon: DaemonId,
        panes: Arc<Mutex<PaneNames>>,
        tabs: Arc<Mutex<TabNames>>,
        shared: Arc<dyn SharedNames>,
    ) -> Names {
        Names { daemon, panes, tabs, shared: Some(shared) }
    }

    /// Registries of its own, for a caller that has no session to share them with.
    ///
    /// What the conformance drivers translate through, under [`Mint::Backend`], so that a case
    /// pinning what the mirror does with `w1:p1` says `w1:p1` and is testing the mirror.
    pub fn alone(daemon: &str, mint: Mint) -> Names {
        Names::new(
            DaemonId::new(daemon),
            Arc::new(Mutex::new(PaneNames::new(mint))),
            Arc::new(Mutex::new(TabNames::new(mint))),
        )
    }

    /// What Muster calls this pane of this daemon's, naming it now if nobody has.
    pub fn pane(&self, backend: &str) -> PaneId {
        let backend = BackendPaneId::new(backend);
        if self.settled()
            && let Some(known) = self.locked_panes().known(&self.daemon, &backend)
        {
            return known;
        }
        self.naming(|panes, _| panes.name(&self.daemon, &backend))
    }

    /// What Muster calls this tab of this daemon's, naming it now if nobody has.
    pub fn tab(&self, backend: &str) -> TabId {
        let backend = BackendTabId::new(backend);
        if self.settled()
            && let Some(known) = self.locked_tabs().known(&self.daemon, &backend)
        {
            return known;
        }
        self.naming(|_, tabs| tabs.name(&self.daemon, &backend))
    }

    /// Whether what this window holds can be trusted without opening the record.
    fn settled(&self) -> bool {
        self.shared.as_ref().is_none_or(|shared| !shared.moved())
    }

    /// The same, for a tab the daemon has only just said it made.
    pub fn tab_from_answer(&self, backend: &str) -> TabId {
        let backend = BackendTabId::new(backend);
        self.naming(|_, tabs| tabs.name_from_answer(&self.daemon, &backend))
    }

    /// Names something while holding the shared record, and leaves the record saying so.
    ///
    /// Read, name, write, all inside one hold. Two Musters that both meet a pane neither has
    /// named therefore take it in turns: the second one reads what the first wrote and finds
    /// the pane already named, so `name` below hands back that name rather than drawing a
    /// second one. Doing this in memory and saving afterwards is what let them disagree.
    ///
    /// Both registries are locked whichever noun is being named, and panes before tabs, because
    /// the record holds both and writing it back reads both - so a caller that took only one
    /// would write a record half of which was somebody else's.
    fn naming<T>(&self, draw: impl FnOnce(&mut PaneNames, &mut TabNames) -> T) -> T {
        if let Some(shared) = self.shared.clone() {
            return holding(&self.panes, &self.tabs, shared.as_ref(), draw);
        }
        let (mut panes, mut tabs) = (self.locked_panes(), self.locked_tabs());
        draw(&mut panes, &mut tabs)
    }

    /// What this daemon calls the pane Muster calls `name`, or why it cannot say.
    ///
    /// A refusal rather than the name passed through, because herdr ignores a `target_pane_id`
    /// it does not recognize and splits whatever it has focused instead - so a name sent
    /// hopefully would land a pane in an arbitrary place and report success. `NotThere` is the
    /// same answer the daemon gives for a pane it has dropped, and means the same thing here:
    /// whoever said this name is talking about something that is not there.
    pub fn backend_pane(&self, name: &PaneId) -> Result<BackendPaneId, Refusal> {
        self.locked_panes()
            .backend(&self.daemon, name)
            .ok_or_else(|| Refusal::NotThere(format!("no pane called {name} on {}", self.daemon)))
    }

    /// What this daemon calls the tab Muster calls `name`, or why it cannot say.
    ///
    /// Refused for the reason a pane's is: herdr ignores a `tab_id` it does not recognize and
    /// acts on whatever it has focused, so a name sent hopefully would rename somebody else's
    /// tab and report success.
    pub fn backend_tab(&self, name: &TabId) -> Result<BackendTabId, Refusal> {
        self.locked_tabs()
            .backend(&self.daemon, name)
            .ok_or_else(|| Refusal::NotThere(format!("no tab called {name} on {}", self.daemon)))
    }

    /// A name for a pane this daemon is about to be asked to make.
    pub fn reserve(&self) -> PaneId {
        self.locked_panes().reserve()
    }

    /// Says which of this daemon's panes a reserved name turned out to be.
    ///
    /// Under the shared record like a mint, and for a sharper reason: this name is already in
    /// the pane's environment, so it is the one binding that cannot be revised afterwards.
    /// Writing it while holding the record is what makes another window adopt it rather than
    /// go on calling the same pane whatever it had guessed.
    pub fn settle(&self, name: &PaneId, backend: &str) {
        let backend = BackendPaneId::new(backend);
        self.naming(|panes, _| panes.settle(name, &self.daemon, &backend));
    }

    /// Gives back a name whose pane the daemon never made.
    pub fn release(&self, name: &PaneId) {
        self.locked_panes().release(name);
    }

    /// Forgets the names of panes this daemon no longer holds.
    ///
    /// Collected before the lock is taken, not after: a caller passing a lazy iterator that
    /// asks this registry anything - and the obvious one does, since it starts from names and
    /// wants ids - would otherwise deadlock against a lock that is not reentrant.
    pub fn prune_panes(&self, held: impl IntoIterator<Item = BackendPaneId>) {
        let held: BTreeSet<BackendPaneId> = held.into_iter().collect();
        self.locked_panes().prune(&self.daemon, &held);
    }

    /// Forgets the names of tabs this daemon no longer holds. Collected first, as above.
    pub fn prune_tabs(&self, held: impl IntoIterator<Item = BackendTabId>) {
        let held: BTreeSet<BackendTabId> = held.into_iter().collect();
        self.locked_tabs().prune(&self.daemon, &held);
    }

    fn locked_panes(&self) -> MutexGuard<'_, PaneNames> {
        poison::lock(&self.panes, "pane-names")
    }

    fn locked_tabs(&self) -> MutexGuard<'_, TabNames> {
        poison::lock(&self.tabs, "tab-names")
    }
}

/// The version this format is on.
///
/// Read on the same terms as the saved arrangement's: a file this Muster does not understand
/// is ignored rather than guessed at. What that costs is that panes made before the upgrade
/// cannot say which pane they are until they are made again - everything else about them works.
///
/// **Still 1 now that tabs are written here too, deliberately.** An unknown version refuses the
/// whole file, so announcing `[[tab]]` with a bump would throw away every *pane* name as
/// well - stranding exactly the long-running agent this file exists for. A file written before
/// tabs were named is still true about panes, and its missing `[[tab]]` array reads correctly as
/// "no tab names", which costs one launch's saved arrangement and nothing else.
const VERSION: i64 = 1;

/// Names, written down.
///
/// Panes outlive the app, so an agent that has been running for hours has to still be able to
/// resolve its own name after Muster has been quit and reopened. Without this the whole
/// mechanism would last exactly one launch.
///
/// Tabs are here for a different reason with the same shape: the saved arrangement records the
/// tab each region was showing, so a registry that forgot its tabs would fail every region's
/// check on reopen and open the window as a first launch, every launch.
///
/// TOML for the reason the arrangement beside it is TOML: one format to learn, and a file
/// somebody opens when a name stops resolving.
pub fn to_toml(panes: &PaneNames, tabs: &TabNames) -> String {
    let mut root = toml::Table::new();
    root.insert("version".to_string(), toml::Value::Integer(VERSION));

    for (key, entries) in [("pane", written(panes)), ("tab", written(tabs))] {
        if !entries.is_empty() {
            root.insert(key.to_string(), toml::Value::Array(entries));
        }
    }

    toml::to_string_pretty(&toml::Value::Table(root))
        .unwrap_or_else(|error| panic!("names should always render as TOML: {error}"))
}

/// Reads the shared record, does something to the registries, and writes them back.
///
/// The whole of what makes two Musters agree: read, name, write, all inside one hold, so the
/// second window to meet a pane reads what the first wrote and finds it already named. In
/// memory first and saved afterwards is what let them disagree.
///
/// Panes are locked before tabs, here and everywhere else that takes both.
fn holding<T>(
    panes: &Arc<Mutex<PaneNames>>,
    tabs: &Arc<Mutex<TabNames>>,
    shared: &dyn SharedNames,
    draw: impl FnOnce(&mut PaneNames, &mut TabNames) -> T,
) -> T {
    let mut drawn = None;
    let mut pending = Some(draw);
    shared.exclusively(&mut |record| {
        let mut panes = poison::lock(panes, "pane-names");
        let mut tabs = poison::lock(tabs, "tab-names");
        // What the record says now, which may be more than this window knew a moment ago. A
        // record that will not read is left to whoever owns the file to complain about, and
        // naming carries on from what this window holds - a window that refused to name a pane
        // would be a window that cannot draw one.
        if let Ok((theirs, their_tabs)) = from_toml(record, panes.mint) {
            panes.adopt(theirs);
            tabs.adopt(their_tabs);
        }
        let work = pending.take().expect("a hold does its work once");
        drawn = Some(work(&mut panes, &mut tabs));
        Some(to_toml(&panes, &tabs))
    });
    drawn.expect("`exclusively` runs what it is given, with a record or without one")
}

/// Puts these registries into the shared record, keeping whatever it has learned meanwhile.
///
/// What a window calls when it has changed the registries without naming anything - forgetting
/// what a daemon no longer holds, or giving back a reservation. The same hold as a mint,
/// because a plain overwrite is exactly the thing that used to lose another window's bindings.
pub fn save_shared(
    panes: &Arc<Mutex<PaneNames>>,
    tabs: &Arc<Mutex<TabNames>>,
    shared: &dyn SharedNames,
) {
    holding(panes, tabs, shared, |_, _| ());
}

/// One registry's entries as the tables that go in the file.
fn written<N, B>(names: &Registry<N, B>) -> Vec<toml::Value>
where
    N: Ord + Clone + std::fmt::Display + From<String>,
    B: Ord + Clone + std::fmt::Display + From<String>,
{
    names
        .entries()
        .map(|(name, at)| {
            let mut table = toml::Table::new();
            table.insert("name".to_string(), toml::Value::String(name.to_string()));
            table.insert("daemon".to_string(), toml::Value::String(at.daemon.to_string()));
            table.insert("backend".to_string(), toml::Value::String(at.backend.to_string()));
            toml::Value::Table(table)
        })
        .collect()
}

/// Reads names back, or says why it will not.
///
/// Every refusal ends the same way - what was already open keeps working, panes stop being able
/// to say which pane they are, and the window opens without its arrangement - so the message is
/// for whoever is reading a log wondering why `$MUSTER_PANE` stopped resolving.
pub fn from_toml(text: &str, mint: Mint) -> Result<(PaneNames, TabNames), String> {
    let root: toml::Table =
        toml::from_str(text).map_err(|error| format!("the saved names are not TOML: {error}"))?;

    match root.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "the saved names are version {other} and this Muster writes version {VERSION}. \
                 They are ignored rather than guessed at, so panes made before now cannot say \
                 which pane they are until they are made again, and this window opens without \
                 the arrangement it was left in."
            ));
        }
        None => return Err("the saved names do not say what version they are".to_string()),
    }

    let mut panes = PaneNames::new(mint);
    read_into(&root, "pane", &mut panes);
    let mut tabs = TabNames::new(mint);
    read_into(&root, "tab", &mut tabs);
    Ok((panes, tabs))
}

/// Binds every entry under one key that reads, skipping the ones that do not.
///
/// A missing array is an ordinary answer rather than a problem: it is what a file written before
/// this noun was named looks like, and what a Muster that has named none of them writes.
fn read_into<N, B>(root: &toml::Table, key: &str, names: &mut Registry<N, B>)
where
    N: Ord + Clone + std::fmt::Display + From<String>,
    B: Ord + Clone + std::fmt::Display + From<String>,
{
    for entry in root.get(key).and_then(toml::Value::as_array).into_iter().flatten() {
        // An entry that will not read is skipped rather than failing the file, on the same
        // terms as a saved region: one unreadable line costs one name.
        let Some(table) = entry.as_table() else { continue };
        let (Some(name), Some(daemon), Some(backend)) =
            (text_at(table, "name"), text_at(table, "daemon"), text_at(table, "backend"))
        else {
            continue;
        };
        names.bind(
            N::from(name),
            Located { daemon: DaemonId::new(daemon), backend: B::from(backend) },
        );
    }
}

fn text_at(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().filter(|value| !value.is_empty()).map(str::to_string)
}
