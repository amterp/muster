//! The records themselves: daemons, the window's tabs, and which pane the keyboard feeds.

use std::collections::{BTreeMap, BTreeSet};

use crate::mirror::Mirror;
use crate::mirror::backend::{PaneId, TabId};

/// Muster's name for one attached daemon.
///
/// Muster's own name, unlike every other id here: herdr has no notion of its own identity,
/// and two daemons on one machine differ only by socket path. What a config file will
/// carry and what a log line has to be readable with, so `local` and `devenv` rather than
/// `/var/folders/…/herdr.sock`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DaemonId(String);

impl DaemonId {
    pub fn new(id: impl Into<String>) -> DaemonId {
        DaemonId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DaemonId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DaemonId {
    fn from(id: &str) -> DaemonId {
        DaemonId(id.to_string())
    }
}

/// How a daemon is reached.
///
/// The wish rather than the resolution. What is written here is what someone asked for -
/// this host, or whatever daemon is on this machine - and never what was found, because a
/// discovered socket path is an observation and observations are meaningless after the
/// thing they observed restarted (`architecture.md`, durability: persist intent, never
/// observation). Where a path was actually found is the runtime's business and lives beside
/// the connection it opened.
///
/// That is why both variants take an optional path. Absent means "the daemon herdr's own
/// client would find", which is what someone running one daemon means and the only thing
/// they should have to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A daemon on this machine, reached by its control socket.
    Local { socket_path: Option<String> },
    /// A daemon on another machine, reached by ssh.
    ///
    /// `options` are handed to `ssh` verbatim. Connection details belong in the user's own
    /// `~/.ssh/config`, where ProxyJump, IdentityFile and the rest already live and where
    /// every other tool looks - restating any of that here would be a worse copy that is
    /// always missing something. This is the escape hatch for what a host alias cannot
    /// cover, and for a test fixture whose key is in the repo.
    Ssh { host: String, options: Vec<String>, socket_path: Option<String> },
}

/// One daemon Muster is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Daemon {
    pub id: DaemonId,
    pub endpoint: Endpoint,
}

/// Names one region for as long as the composition holds it.
///
/// A number rather than the tab it shows, because what a region shows is the part that
/// changes. A region that swaps tabs is still the same region, and a shell told so keeps
/// its window furniture instead of tearing it down and building it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(u64);

impl RegionId {
    /// Names a region by number, for whoever is reading a composition back.
    ///
    /// Public because this record exists to be written down and read again, and a value
    /// only its own module can rebuild is one no file can hold. Naming a region that does
    /// not exist is allowed and does nothing - every operation that takes one already has
    /// to survive a region closing underneath it.
    pub fn new(number: u64) -> RegionId {
        RegionId(number)
    }

    pub fn number(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RegionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// One pane, named the way anything spanning daemons has to name one.
///
/// Two daemons both hand out `w1:p1`, so anything keyed by pane alone lets one machine's
/// pane answer for the other's. Every message across the seam that names a pane already
/// carries both halves; this is the same pair when it needs to be one value - a set, a map
/// key, a log field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaneKey {
    pub daemon: DaemonId,
    pub pane: PaneId,
}

impl PaneKey {
    pub fn new(daemon: &DaemonId, pane: &PaneId) -> PaneKey {
        PaneKey { daemon: daemon.clone(), pane: pane.clone() }
    }
}

impl std::fmt::Display for PaneKey {
    /// `local/w1:p1`, so a log line or a corpus case can name a pane unambiguously in one
    /// token. Split at the first slash to read one back: a daemon id is Muster's own and
    /// holds none, where a pane id is the backend's string and Muster never parses it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.daemon, self.pane)
    }
}

/// One tab, named the way anything spanning daemons has to name one.
///
/// The same pair as [`PaneKey`] and for the same reason: two daemons both hand out `w1:t1`,
/// so a tab named by id alone lets one machine's tab answer for the other's. What a keystroke
/// asking for the third tab in the window resolves to, and what a sidebar row carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabKey {
    pub daemon: DaemonId,
    pub tab: TabId,
}

impl TabKey {
    pub fn new(daemon: &DaemonId, tab: &TabId) -> TabKey {
        TabKey { daemon: daemon.clone(), tab: tab.clone() }
    }
}

impl std::fmt::Display for TabKey {
    /// `local/w1:t1`, on the same terms as [`PaneKey`].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.daemon, self.tab)
    }
}

/// The part of a Muster tab that one machine holds, as it sits on screen.
///
/// A region divides a tab and not the window (MIP-2). A tab holding panes on one machine has one
/// region, which is every tab until somebody groups two; a tab holding a laptop pane beside a
/// devenv pane has one for each, side by side in this order. What a region shows is that
/// machine's half of the tab, which is one herdr tab and one pane tree.
///
/// It does not name the tab: the tab owns the list it is in, and a region carrying the answer as
/// well would be one fact written twice.
///
/// `Eq` is absent because of the weight, on the same terms as [`crate::mirror::backend::LayoutNode`]:
/// a share is a float, and a float has no total equality.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub id: RegionId,
    pub daemon: DaemonId,

    /// How much of the window's width this region gets, relative to the others.
    ///
    /// A weight rather than a ratio, and a number per region rather than one per boundary,
    /// because regions are a list and not a tree. Owning a tree over them is what would make
    /// Muster a multiplexer, which is a non-goal - so laying them out has to be something a
    /// list can answer, and dividing the width by the sum of the weights is that.
    ///
    /// Every region starts at one, so equal shares fall out of the arrangement rather than
    /// being a case anybody wrote. A tab on one machine has a single region and its weight
    /// never matters.
    pub weight: f32,
    /// The pane in this region Muster's keyboard feeds while the region is focused.
    ///
    /// View-local, and never read back from the daemon's own cursor. Daemon focus is a
    /// single value shared with every client, so routing input by it would let a herdr TUI
    /// in another window yank this window's keyboard (`architecture.md`, cursors are
    /// written, not read). Muster *writes* daemon focus as the user moves around here, and
    /// then ignores what it wrote.
    pub pane: Option<PaneId>,
}

/// One Muster tab this window holds, and how it is divided when it is on screen.
///
/// A named set of panes a window shows together (MIP-2). Muster mints the name the way it mints
/// a pane's, and which herdr tab that name means on each machine is the name registry's answer
/// (`crate::names`) - so a tab holding panes on two machines is one name over two herdr tabs,
/// and a tab holding panes on one is the same thing with one member, which is every tab until
/// somebody groups two.
///
/// The regions are held per tab rather than per window because they are the tab's arrangement:
/// how wide each machine's half is, and which pane the keyboard was on. Switching away and back
/// lands where it was left because of it.
#[derive(Debug, Clone, PartialEq)]
pub struct MusterTab {
    pub id: TabId,
    /// In the order they are laid out, one per machine holding panes in this tab.
    regions: Vec<Region>,
    /// Which of them the keyboard is in while this tab is on screen.
    focused: Option<RegionId>,
}

impl MusterTab {
    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    /// The machines holding panes in this tab, in the order their regions sit on screen.
    pub fn daemons(&self) -> impl Iterator<Item = &DaemonId> {
        self.regions.iter().map(|region| &region.daemon)
    }

    pub fn focused_region(&self) -> Option<&Region> {
        let focused = self.focused?;
        self.regions.iter().find(|region| region.id == focused)
    }

    /// Keeps the keyboard pointed at a region of this tab that exists.
    ///
    /// Moves to the first surviving region rather than clearing, because a tab with regions in
    /// it and no focus is a tab that ignores the keyboard, and nothing would tell the user why.
    fn settle_focus(&mut self) {
        if self.focused.is_some_and(|id| self.regions.iter().any(|region| region.id == id)) {
            return;
        }
        self.focused = self.regions.first().map(|region| region.id);
    }
}

/// Which daemons are attached, which tabs this window holds, and which of them is on screen.
///
/// **A window holds an ordered list of Muster tabs and shows one of them** (MIP-2). Tabs arrive
/// in the order the daemons describe them and a new one goes on the end, which is where every
/// other tab strip puts one. `cmd+1` to `cmd+9` and `next_tab` walk that list, and a tab on the
/// list that no daemon still holds is not on it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Composition {
    daemons: BTreeMap<DaemonId, Daemon>,
    /// In the order they are walked. Not a map: the order is part of the answer, and it is
    /// one of the two things about the arrangement Muster owns.
    tabs: Vec<MusterTab>,
    /// The tab on screen, once something has said which.
    ///
    /// `None` means nobody has chosen, not that nothing is on screen: a window that has been
    /// told nothing shows the first tab it holds that is not somebody else's, which is what a
    /// launch onto a machine already holding tabs means. See [`Composition::showing`].
    showing: Option<TabId>,
    /// Tabs this window will not open onto of its own accord, by machine.
    ///
    /// A window somebody asked for is a different launch from the window Muster comes back to
    /// (MIP-2), and herdr allows one client per terminal - so opening onto a tab another window
    /// is already showing is a window of panes that refuse to attach and cannot be closed
    /// (kan a_2IZ5TL6DQ). What each machine was already holding when this window first saw it
    /// is written here, and this window opens onto the tab that appears after.
    ///
    /// They are still listed and still reachable: ⌘2 and a click on a caption go where they are
    /// pointed, and taking a terminal from another window is a thing somebody may mean. What
    /// this stops is Muster deciding it uninvited.
    ///
    /// Per launch and never saved. Next launch there is no other window to inherit from.
    claimed: BTreeMap<DaemonId, BTreeSet<TabId>>,
    /// Never reused, so that a region id names one region for the life of a composition.
    /// Reusing them would let a stale intent - a keystroke sent as a region closed - land
    /// in whatever took its place.
    next_region: u64,
}

impl Composition {
    pub fn new() -> Composition {
        Composition::default()
    }

    /// Attaches a daemon, or restates how an already-named one is reached.
    ///
    /// Idempotent by name, because attaching is what happens on every reconnect and on
    /// every window that opens onto a daemon somebody is already watching.
    pub fn attach_daemon(&mut self, daemon: Daemon) {
        self.daemons.insert(daemon.id.clone(), daemon);
    }

    /// Detaches a daemon, and takes its half out of every tab.
    ///
    /// Its regions go because they have nothing left to show and no way back: their panes
    /// live in a process this window can no longer reach. Leaving them would render a
    /// session that is not there, which is the failure a stale mirror is labeled to avoid. A
    /// tab that machine was the whole of goes with them; one that spans two machines stays,
    /// showing the half still reachable, which is what a tab whose devenv has dropped out has
    /// to do rather than refusing to open (`architecture.md`, degradation).
    pub fn detach_daemon(&mut self, id: &DaemonId) {
        if self.daemons.remove(id).is_none() {
            return;
        }
        let was_at = self.showing_at();
        self.claimed.remove(id);
        for tab in &mut self.tabs {
            tab.regions.retain(|region| &region.daemon != id);
            tab.settle_focus();
        }
        self.tabs.retain(|tab| !tab.regions.is_empty());
        self.settle_showing(was_at);
    }

    pub fn daemon(&self, id: &DaemonId) -> Option<&Daemon> {
        self.daemons.get(id)
    }

    pub fn daemons(&self) -> impl Iterator<Item = &Daemon> {
        self.daemons.values()
    }

    /// Says that this daemon holds panes in this tab, and names the region showing its half.
    ///
    /// The one way a tab enters a window and the one way it grows a second machine. A tab
    /// nothing had heard of is appended to the list; a tab already there gains a region on
    /// the end for this machine, which is grouping. Both are idempotent: a daemon that
    /// already has a region in this tab gets the one it has.
    ///
    /// `None` when that daemon is not attached. A region naming a daemon nobody is connected
    /// to would render nothing forever, and would do it silently - so the refusal is returned
    /// rather than the region being built and left empty.
    ///
    /// Nothing is brought on screen. Opening a region is not by itself a claim on the window,
    /// and a window that switched tabs every time a daemon published something is a window
    /// that types into the wrong pane.
    pub fn open_region(&mut self, daemon: &DaemonId, tab: TabId) -> Option<RegionId> {
        if !self.daemons.contains_key(daemon) {
            return None;
        }
        if let Some(held) = self.region_of(daemon, &tab) {
            return Some(held);
        }
        let id = RegionId(self.next_region);
        self.next_region += 1;
        let region = Region {
            id,
            daemon: daemon.clone(),
            // Filled by the first reconcile that can see the tab's panes. The caller
            // usually knows which pane it wants and says so; this stays the answer for a
            // region opened onto a tab rather than onto a pane.
            pane: None,
            weight: 1.0,
        };
        match self.tabs.iter_mut().find(|held| held.id == tab) {
            Some(held) => {
                held.regions.push(region);
                held.settle_focus();
            }
            None => self.tabs.push(MusterTab { id: tab, regions: vec![region], focused: Some(id) }),
        }
        Some(id)
    }

    /// Closes one machine's half of a tab, and the tab when that was the whole of it.
    pub fn close_region(&mut self, id: RegionId) {
        let was_at = self.showing_at();
        for tab in &mut self.tabs {
            tab.regions.retain(|region| region.id != id);
            tab.settle_focus();
        }
        self.tabs.retain(|tab| !tab.regions.is_empty());
        self.settle_showing(was_at);
    }

    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.tabs.iter().flat_map(MusterTab::regions).find(|region| region.id == id)
    }

    /// The regions of the tab on screen, in the order they are laid out.
    ///
    /// What a window is drawing. A tab that is not on screen still has regions - they are its
    /// arrangement, remembered for when it comes back - and they are reached through
    /// [`Composition::tabs`].
    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.showing_tab().into_iter().flat_map(MusterTab::regions)
    }

    /// Every tab this window holds, in the order they are walked.
    pub fn tabs(&self) -> impl Iterator<Item = &MusterTab> {
        self.tabs.iter()
    }

    pub fn tab(&self, id: &TabId) -> Option<&MusterTab> {
        self.tabs.iter().find(|tab| &tab.id == id)
    }

    /// Which tab this window is showing, or nothing when it holds none it may show.
    pub fn showing(&self) -> Option<&TabId> {
        self.showing_tab().map(|tab| &tab.id)
    }

    /// The tab on screen: the one chosen, or the first this window is free to open onto.
    ///
    /// A fallback rather than a choice made at the moment a tab appears, because the two inputs
    /// land in an order nothing guarantees: a daemon's first snapshot and this window's claim on
    /// that machine arrive on different threads. Derived from both every time, the order stops
    /// mattering.
    fn showing_tab(&self) -> Option<&MusterTab> {
        if let Some(showing) = self.showing.as_ref()
            && let Some(tab) = self.tabs.iter().find(|tab| &tab.id == showing)
        {
            return Some(tab);
        }
        self.tabs.iter().find(|tab| !self.is_claimed(&tab.id))
    }

    fn is_claimed(&self, tab: &TabId) -> bool {
        self.claimed.values().any(|theirs| theirs.contains(tab))
    }

    /// Records what a machine was already holding when this window first saw it.
    ///
    /// Idempotent by machine: a claim is made once, on the first snapshot, and a second one
    /// would take in the tab this window has since opened for itself.
    pub fn claim(&mut self, daemon: &DaemonId, theirs: BTreeSet<TabId>) -> bool {
        if self.claimed.contains_key(daemon) {
            return false;
        }
        self.claimed.insert(daemon.clone(), theirs);
        true
    }

    /// What this machine was holding when this window claimed it, if it has.
    pub fn claimed(&self, daemon: &DaemonId) -> Option<&BTreeSet<TabId>> {
        self.claimed.get(daemon)
    }

    /// The machine's half of a tab, if this window is holding one.
    ///
    /// An answer rather than a policy: a caller deciding what to do about a tab this daemon has
    /// no half of is the one that knows whether to open one.
    pub fn region_of(&self, daemon: &DaemonId, tab: &TabId) -> Option<RegionId> {
        self.tab(tab)?
            .regions
            .iter()
            .find(|region| &region.daemon == daemon)
            .map(|region| region.id)
    }

    /// Puts a tab on screen, and says whether there was one to put there.
    ///
    /// What ⌘2, `next_tab` and a click on a caption all come down to. The tab keeps whichever
    /// region had the keyboard when it was last on screen, so switching away and back lands
    /// where it was left.
    pub fn show(&mut self, tab: &TabId) -> bool {
        let held = self.tabs.iter().any(|held| &held.id == tab);
        if held {
            self.showing = Some(tab.clone());
        }
        held
    }

    /// Brings a tab onto the screen, opening this machine's half of it if there is not one.
    ///
    /// The half of attention routing that is not a colour on a pane. A `done` or `blocked`
    /// agent is most often on a pane no window is showing, so being told about it is only
    /// useful if going to it works - and going to it means bringing its tab on screen
    /// (`architecture.md`, attention routing: surfacing the hidden is part of the feature, and
    /// the core owns it).
    ///
    /// `None` only when the daemon is not attached, which is the one case where there is
    /// nothing honest to show.
    pub fn surface(&mut self, daemon: &DaemonId, tab: &TabId) -> Option<RegionId> {
        let region = self.open_region(daemon, tab.clone())?;
        self.show(tab);
        Some(region)
    }

    /// Points the window's keyboard at a pane, and brings the tab holding it on screen.
    ///
    /// A no-op for a region that is gone, rather than an error. A focus intent racing a
    /// region that just closed is ordinary - the intent was in flight while the daemon said
    /// the tab was over - and there is nothing for a caller to do about it.
    ///
    /// The tab comes on screen because that is what pointing the keyboard at a pane means once
    /// a window shows one tab at a time: leaving it behind would put the keyboard in a pane
    /// nobody can see. It is a no-op for the tab already showing, which is almost every call.
    ///
    /// The pane is taken on trust: whether it is really in that region's tab is daemon
    /// truth, and [`Composition::reconcile`] is where daemon truth is applied. Checking here
    /// would need the mirror on a path that is called from a keystroke.
    pub fn focus_pane(&mut self, region: RegionId, pane: PaneId) {
        let Some(tab) = self.tab_holding_mut(region) else { return };
        if let Some(found) = tab.regions.iter_mut().find(|held| held.id == region) {
            found.pane = Some(pane);
        }
        tab.focused = Some(region);
        let id = tab.id.clone();
        self.showing = Some(id);
    }

    /// Points the window's keyboard at a region, keeping whichever pane it last fed.
    pub fn focus_region(&mut self, region: RegionId) {
        let Some(tab) = self.tab_holding_mut(region) else { return };
        tab.focused = Some(region);
        let id = tab.id.clone();
        self.showing = Some(id);
    }

    fn tab_holding_mut(&mut self, region: RegionId) -> Option<&mut MusterTab> {
        self.tabs.iter_mut().find(|tab| tab.regions.iter().any(|held| held.id == region))
    }

    /// Moves the line between a region and the one to its right.
    ///
    /// `ratio` is the named region's share of the two of them together, so the pair keeps
    /// whatever width it had and only the split between them moves. Everything further along
    /// the window stays where it is, which is what a drag looks like to the person doing it.
    ///
    /// Named by the region on the left rather than by an index, unlike a pane divider. A
    /// pane divider genuinely has no name - it is a position in a tree that changes under
    /// it - but a region does, and a request that survives its neighbours closing mid-drag is
    /// better than one that silently moves a different line.
    ///
    /// Clamped, and this is the one place a share is. A pane's ratio is passed to the daemon
    /// untouched because the daemon sizes its own rectangles and will refuse what it cannot
    /// do; nothing sits behind this one. A region dragged to nothing would leave no divider
    /// to grab and no way back, so neither side goes below a tenth of the pair.
    ///
    /// A boundary that does not exist - the last region of a tab, or one that closed while a
    /// drag was in flight - does nothing. There is nothing for a caller to do about that either.
    pub fn set_boundary(&mut self, left: RegionId, ratio: f32) {
        if !ratio.is_finite() {
            return;
        }
        let Some(tab) = self.tab_holding_mut(left) else { return };
        let Some(index) = tab.regions.iter().position(|region| region.id == left) else { return };
        let Some(pair) = tab.regions.get(index..=index + 1) else { return };
        let total = pair[0].weight + pair[1].weight;
        if !total.is_finite() || total <= 0.0 {
            return;
        }
        let ratio = ratio.clamp(Composition::MINIMUM_SHARE, 1.0 - Composition::MINIMUM_SHARE);
        tab.regions[index].weight = total * ratio;
        tab.regions[index + 1].weight = total * (1.0 - ratio);
    }

    /// Puts one region's share back to what it was, without touching its neighbours.
    ///
    /// The restore path, and the reason it is not `set_boundary`: dragging a divider is about
    /// a pair and has to leave the rest of the tab alone, where reopening a saved arrangement
    /// sets every weight in turn and would fight itself if each one redistributed.
    ///
    /// A weight that is not a positive finite number is ignored rather than taken. Zero is a
    /// region rendered at no width, which nobody can see or grab their way out of.
    pub fn set_weight(&mut self, region: RegionId, weight: f32) {
        if !weight.is_finite() || weight <= 0.0 {
            return;
        }
        let Some(tab) = self.tab_holding_mut(region) else { return };
        if let Some(held) = tab.regions.iter_mut().find(|held| held.id == region) {
            held.weight = weight;
        }
    }

    /// The least of a boundary's pair either side may be dragged down to.
    ///
    /// A tenth, which at any window worth using leaves a region wide enough to see and a
    /// divider wide enough to grab. Smaller would be a state a user can reach and not leave.
    pub const MINIMUM_SHARE: f32 = 0.1;

    pub fn focused_region(&self) -> Option<&Region> {
        self.showing_tab()?.focused_region()
    }

    /// The pane this window's keyboard feeds, if there is one.
    ///
    /// The single question the input path asks of composition, and the reason view-local
    /// focus lives here rather than in the shell: every surface that could answer it is
    /// disposable, and the answer has to survive one being torn down.
    pub fn focused_pane(&self) -> Option<&PaneId> {
        self.focused_region()?.pane.as_ref()
    }

    /// Brings composition in line with what one daemon now holds.
    ///
    /// Composition names daemon things - a tab, a pane - and daemon things go away without
    /// asking. A tab closed from another client has nothing left to show; a keyboard pointed
    /// at a pane whose program exited types into nothing. Neither is a state a view can render
    /// its way out of, so both are resolved here - once, in the core - rather than guarded at
    /// every reader.
    ///
    /// **A tab this daemon holds and this window does not is added.** That is how a window
    /// comes to list a laptop tab beside a devenv tab, and it replaces the rule that gave every
    /// machine a column of its own. Nothing is owed a column any more, so a machine whose last
    /// pane closes simply stops contributing tabs (kan a_2I6h18OU6).
    ///
    /// One daemon at a time, and only its own regions. Streams from different daemons have
    /// no mutual order, so a pass over every region would resolve one daemon's regions
    /// against another daemon's news (`architecture.md`, cross-daemon order is core order). A
    /// tab spanning two machines is reconciled a half at a time for the same reason: its
    /// devenv region goes when the devenv says the tab is over, and its laptop region stays.
    pub fn reconcile(&mut self, daemon: &DaemonId, mirror: &Mirror) {
        if !self.daemons.contains_key(daemon) {
            return;
        }
        let was_at = self.showing_at();
        for tab in mirror.tabs().map(|tab| tab.id.clone()).collect::<Vec<TabId>>() {
            self.open_region(daemon, tab);
        }

        for tab in &mut self.tabs {
            let held = mirror.tab(&tab.id).is_some();
            tab.regions.retain(|region| &region.daemon != daemon || held);
            for region in tab.regions.iter_mut().filter(|region| &region.daemon == daemon) {
                // The pane list decides whether the keyboard's pane is still there, and the
                // tree never does. They are published separately, so a tab mid-split briefly
                // has a tree naming fewer panes than it holds - and a rule that read the tree
                // as evidence would hand the keyboard to another pane and keep it there,
                // because the tree that arrives a moment later makes the wrong answer a valid
                // one. Measured, not guessed: splitting a two-pane tab publishes a one-pane
                // tree in between.
                let holds = region
                    .pane
                    .as_ref()
                    .and_then(|pane| mirror.pane(pane))
                    .is_some_and(|pane| pane.tab == tab.id);
                if holds {
                    continue;
                }
                // The first pane rather than the closed one's neighbour. Naming the neighbour
                // needs the order as it stood before the change, and that is a cache of daemon
                // truth - which is exactly what this layer must not keep. A rule someone can
                // predict beats one that is usually nicer and occasionally inexplicable.
                region.pane = panes_of(mirror, &tab.id).into_iter().next();
            }
            tab.settle_focus();
        }
        self.tabs.retain(|tab| !tab.regions.is_empty());
        self.settle_showing(was_at);
    }

    /// Keeps the window showing a tab it still holds.
    ///
    /// `was_at` is where the tab on screen sat before anything was taken out, so closing the
    /// tab you are on lands on whatever took its place - the next one along, or the last when
    /// it was the last. That is what every tab strip does, and it needs the place because by
    /// the time this runs the tab is not there to be found.
    ///
    /// Clearing the choice rather than picking is the other half: an unchosen window shows the
    /// first tab it may open onto, which is where a window with nothing left ends up.
    fn settle_showing(&mut self, was_at: Option<usize>) {
        if self.showing.as_ref().is_some_and(|id| self.tabs.iter().any(|tab| &tab.id == id)) {
            return;
        }
        self.showing = was_at
            .and_then(|at| self.tabs.get(at).or_else(|| self.tabs.last()))
            .map(|tab| tab.id.clone());
    }

    /// Where the tab on screen sits, read before anything is taken out of the list.
    fn showing_at(&self) -> Option<usize> {
        let showing = self.showing_tab()?.id.clone();
        self.tabs.iter().position(|tab| tab.id == showing)
    }
}

/// A tab's panes, in the order its tree lays them out.
///
/// Tree order when there is a tree and id order when there is not, because a tab whose
/// layout has not arrived yet is a real state - herdr publishes the tree on its own event,
/// which may follow the panes it names - and "the first pane" has to mean something in it.
///
/// Panes the tree names but the mirror does not hold are skipped either way: a tree that
/// arrived ahead of its panes would otherwise put the keyboard on one nothing can type
/// into.
fn panes_of(mirror: &Mirror, tab: &TabId) -> Vec<PaneId> {
    let ordered: Vec<PaneId> = match mirror.layout(tab) {
        Some(layout) => layout
            .root
            .panes()
            .into_iter()
            .filter(|id| mirror.pane(id).is_some())
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    if ordered.is_empty() {
        return mirror.panes_in_tab(tab).map(|pane| pane.id.clone()).collect();
    }
    ordered
}
