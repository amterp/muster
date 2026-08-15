//! The records themselves: daemons, regions, and which pane the keyboard feeds.

use std::collections::BTreeMap;

use crate::mirror::Mirror;
use crate::mirror::backend::{PaneId, TabId, WorkspaceId};

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

/// The part of a window showing one tab's pane tree.
///
/// `Eq` is absent because of the weight, on the same terms as [`crate::mirror::backend::LayoutNode`]:
/// a share is a float, and a float has no total equality.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub id: RegionId,
    pub daemon: DaemonId,
    pub workspace: WorkspaceId,
    pub tab: TabId,

    /// How much of the window's width this region gets, relative to the others.
    ///
    /// A weight rather than a ratio, and a number per region rather than one per boundary,
    /// because regions are a list and not a tree. Owning a tree over them is what would make
    /// Muster a multiplexer, which is a non-goal - so laying them out has to be something a
    /// list can answer, and dividing the width by the sum of the weights is that.
    ///
    /// Every region starts at one, so equal shares fall out of the arrangement rather than
    /// being a case anybody wrote.
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

/// Which daemons are attached, and what each region of the window shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Composition {
    daemons: BTreeMap<DaemonId, Daemon>,
    /// In the order they are laid out. Not a map: the order is part of the answer, and it
    /// is the only part of the arrangement Muster owns.
    regions: Vec<Region>,
    focused: Option<RegionId>,
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

    /// Detaches a daemon, and closes every region showing it.
    ///
    /// The regions go because they have nothing left to show and no way back: their panes
    /// live in a process this window can no longer reach. Leaving them would render a
    /// session that is not there, which is the failure a stale mirror is labeled to avoid.
    pub fn detach_daemon(&mut self, id: &DaemonId) {
        if self.daemons.remove(id).is_none() {
            return;
        }
        self.regions.retain(|region| &region.daemon != id);
        self.settle_focus();
    }

    pub fn daemon(&self, id: &DaemonId) -> Option<&Daemon> {
        self.daemons.get(id)
    }

    pub fn daemons(&self) -> impl Iterator<Item = &Daemon> {
        self.daemons.values()
    }

    /// Shows a tab in a new region, and names it.
    ///
    /// `None` when that daemon is not attached. A region naming a daemon nobody is
    /// connected to would render nothing forever, and would do it silently - so the refusal
    /// is returned rather than the region being built and left empty.
    ///
    /// The new region takes focus only if nothing had it. Opening a region is not by itself
    /// a claim on the keyboard, and a window that moves focus every time a daemon publishes
    /// something is a window that types into the wrong pane.
    pub fn open_region(
        &mut self,
        daemon: &DaemonId,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Option<RegionId> {
        if !self.daemons.contains_key(daemon) {
            return None;
        }
        let id = RegionId(self.next_region);
        self.next_region += 1;
        self.regions.push(Region {
            id,
            daemon: daemon.clone(),
            workspace,
            tab,
            // Filled by the first reconcile that can see the tab's panes. The caller
            // usually knows which pane it wants and says so; this stays the answer for a
            // region opened onto a tab rather than onto a pane.
            pane: None,
            weight: 1.0,
        });
        if self.focused.is_none() {
            self.focused = Some(id);
        }
        Some(id)
    }

    pub fn close_region(&mut self, id: RegionId) {
        self.regions.retain(|region| region.id != id);
        self.settle_focus();
    }

    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.iter().find(|region| region.id == id)
    }

    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    /// The region already showing this tab, if one is.
    ///
    /// An answer rather than a policy: whether to reuse that region or open a second one
    /// onto the same tab is the caller's call, and both are things somebody might want.
    pub fn region_showing(&self, daemon: &DaemonId, tab: &TabId) -> Option<RegionId> {
        self.regions
            .iter()
            .find(|region| &region.daemon == daemon && &region.tab == tab)
            .map(|region| region.id)
    }

    /// Brings a tab onto the screen, and says which region is now showing it.
    ///
    /// The half of attention routing that is not a colour on a pane. A `done` or `blocked`
    /// agent is most often on a pane no window is showing, so being told about it is only
    /// useful if going to it works - and going to it means changing what a region shows
    /// first (`architecture.md`, attention routing: surfacing the hidden is part of the
    /// feature, and the core owns it).
    ///
    /// Three answers, in order. A region already showing this tab is the one to use; a
    /// second region onto the same tab would be two copies of one thing. Otherwise a region
    /// already on this daemon is retargeted - the focused one when it qualifies, so that
    /// following a notification does not move the keyboard's region out from under the next
    /// keystroke. Only when the daemon has no region at all does one open.
    ///
    /// A region on another daemon is never taken. A window showing a laptop and a devenv
    /// side by side is the arrangement this project exists for, and quietly replacing one
    /// with the other would be a worse surprise than a new region appearing.
    ///
    /// `None` only when the daemon is not attached, which is the one case where there is
    /// nothing honest to show.
    pub fn surface(
        &mut self,
        daemon: &DaemonId,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Option<RegionId> {
        if let Some(showing) = self.region_showing(daemon, &tab) {
            return Some(showing);
        }
        let retarget = self
            .focused
            .filter(|id| {
                self.regions.iter().any(|region| region.id == *id && &region.daemon == daemon)
            })
            .or_else(|| {
                self.regions.iter().find(|region| &region.daemon == daemon).map(|region| region.id)
            });
        let Some(id) = retarget else {
            return self.open_region(daemon, workspace, tab);
        };
        let region = self.regions.iter_mut().find(|region| region.id == id)?;
        region.workspace = workspace;
        region.tab = tab;
        // Cleared rather than kept: the pane it held is in the tab this region just stopped
        // showing, and leaving it would point the keyboard at something off screen. The
        // caller names the pane it wanted, and reconcile fills one in otherwise.
        region.pane = None;
        Some(id)
    }

    /// Points the window's keyboard at a pane, and at the region holding it.
    ///
    /// A no-op for a region that is gone, rather than an error. A focus intent racing a
    /// region that just closed is ordinary - the intent was in flight while the daemon said
    /// the tab was over - and there is nothing for a caller to do about it.
    ///
    /// The pane is taken on trust: whether it is really in that region's tab is daemon
    /// truth, and [`Composition::reconcile`] is where daemon truth is applied. Checking here
    /// would need the mirror on a path that is called from a keystroke.
    pub fn focus_pane(&mut self, region: RegionId, pane: PaneId) {
        let Some(found) = self.regions.iter_mut().find(|held| held.id == region) else {
            return;
        };
        found.pane = Some(pane);
        self.focused = Some(region);
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
    /// A boundary that does not exist - the last region, or one that closed while a drag was
    /// in flight - does nothing. There is nothing for a caller to do about that either.
    /// Puts one region's share back to what it was, without touching its neighbours.
    ///
    /// The restore path, and the reason it is not `set_boundary`: dragging a divider is about
    /// a pair and has to leave the rest of the window alone, where reopening a saved
    /// arrangement sets every weight in turn and would fight itself if each one redistributed.
    ///
    /// A weight that is not a positive finite number is ignored rather than taken. Zero is a
    /// region rendered at no width, which nobody can see or grab their way out of.
    pub fn set_weight(&mut self, region: RegionId, weight: f32) {
        if !weight.is_finite() || weight <= 0.0 {
            return;
        }
        if let Some(held) = self.regions.iter_mut().find(|held| held.id == region) {
            held.weight = weight;
        }
    }

    pub fn set_boundary(&mut self, left: RegionId, ratio: f32) {
        if !ratio.is_finite() {
            return;
        }
        let Some(index) = self.regions.iter().position(|region| region.id == left) else {
            return;
        };
        let Some(pair) = self.regions.get(index..=index + 1) else {
            return;
        };
        let total = pair[0].weight + pair[1].weight;
        if !total.is_finite() || total <= 0.0 {
            return;
        }
        let ratio = ratio.clamp(Composition::MINIMUM_SHARE, 1.0 - Composition::MINIMUM_SHARE);
        self.regions[index].weight = total * ratio;
        self.regions[index + 1].weight = total * (1.0 - ratio);
    }

    /// The least of a boundary's pair either side may be dragged down to.
    ///
    /// A tenth, which at any window worth using leaves a region wide enough to see and a
    /// divider wide enough to grab. Smaller would be a state a user can reach and not leave.
    pub const MINIMUM_SHARE: f32 = 0.1;

    /// Points the window's keyboard at a region, keeping whichever pane it last fed.
    pub fn focus_region(&mut self, region: RegionId) {
        if self.regions.iter().any(|held| held.id == region) {
            self.focused = Some(region);
        }
    }

    pub fn focused_region(&self) -> Option<&Region> {
        let focused = self.focused?;
        self.region(focused)
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
    /// Composition names daemon things, and daemon things go away without asking. A region
    /// whose tab was closed from another client has nothing left to show; a keyboard
    /// pointed at a pane whose program exited types into nothing. Neither is a state a view
    /// can render its way out of, so both are resolved here - once, in the core - rather
    /// than guarded at every reader.
    ///
    /// One daemon at a time, and only its own regions. Streams from different daemons have
    /// no mutual order, so a pass over every region would resolve one daemon's regions
    /// against another daemon's news (`architecture.md`, cross-daemon order is core order).
    pub fn reconcile(&mut self, daemon: &DaemonId, mirror: &Mirror) {
        self.regions.retain(|region| &region.daemon != daemon || mirror.tab(&region.tab).is_some());

        for region in self.regions.iter_mut().filter(|region| &region.daemon == daemon) {
            // The pane list decides whether the keyboard's pane is still there, and the tree
            // never does. They are published separately, so a tab mid-split briefly has a
            // tree naming fewer panes than it holds - and a rule that read the tree as
            // evidence would hand the keyboard to another pane and keep it there, because
            // the tree that arrives a moment later makes the wrong answer a valid one.
            // Measured, not guessed: splitting a two-pane tab publishes a one-pane tree in
            // between.
            let held = region
                .pane
                .as_ref()
                .and_then(|pane| mirror.pane(pane))
                .is_some_and(|pane| pane.tab == region.tab);
            if held {
                continue;
            }
            // The first pane rather than the closed one's neighbour. Naming the neighbour
            // needs the order as it stood before the change, and that is a cache of daemon
            // truth - which is exactly what this layer must not keep. A rule someone can
            // predict beats one that is usually nicer and occasionally inexplicable.
            region.pane = panes_of(mirror, &region.tab).into_iter().next();
        }
        self.settle_focus();
    }

    /// Keeps the keyboard pointed at a region that exists.
    ///
    /// Moves to the first surviving region rather than clearing, because a window with
    /// regions in it and no focus is a window that ignores the keyboard, and nothing would
    /// tell the user why.
    fn settle_focus(&mut self) {
        if self.focused.is_some_and(|id| self.regions.iter().any(|region| region.id == id)) {
            return;
        }
        self.focused = self.regions.first().map(|region| region.id);
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
