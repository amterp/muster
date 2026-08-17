//! What the records plus a daemon's mirror add up to on screen.
//!
//! The whole answer every time, absolute and idempotent, in the same discipline as the
//! events a daemon sends. That is what lets the shell stay dumb: it renders what it is
//! handed and holds no picture of its own to patch, so there is no state up there to drift.
//!
//! Structure and identity only. What an agent is doing is not here, and deliberately: a
//! pane's state moves far more often than the arrangement it sits in, and joining the two
//! would repaint the tree every time an agent blinked. It has its own per-pane message and
//! one writer, and gains nothing from a second (`architecture.md`, agent state has one
//! writer).
//!
//! Derived and disposable, unlike the records next door. Nothing here is saved, and nothing
//! here is authoritative: a view is correct exactly as long as the mirror behind it is.

use std::collections::BTreeSet;

use crate::composition::record::{Composition, DaemonId, PaneKey, Region, RegionId};
use crate::mirror::Mirror;
use crate::mirror::backend::{Layout, LayoutNode, PaneId, SplitAxis, TabId};

/// Everything one window is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub regions: Vec<ViewRegion>,
    /// The region whose pane the keyboard feeds.
    pub focused: Option<RegionId>,
}

/// One region, and the tab it is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewRegion {
    pub id: RegionId,
    pub daemon: DaemonId,
    pub tab: TabId,
    /// The pane in this region Muster's keyboard feeds when the region is focused.
    pub pane: Option<PaneId>,
    /// How much of the window's width this region gets, relative to the others in the list.
    ///
    /// Muster's own answer rather than a daemon's. The arrangement over regions is the one
    /// part of the layout nothing upstream will ever have an opinion about, because no daemon
    /// knows the other one exists.
    pub weight: f32,
    /// `None` while the daemon has not said how this tab is arranged.
    ///
    /// A real state rather than a failure - herdr publishes the tree on its own event,
    /// which may follow the panes it names - and a distinct one from an empty region: a
    /// shell told `None` leaves what it has alone, where a shell told "no panes" would tear
    /// down surfaces that are about to be described.
    pub root: Option<ViewNode>,
    /// Whether `root` is one pane filling the region rather than the tab's whole tree.
    ///
    /// The tree is already resolved to the zoomed pane, so a shell that ignores this still
    /// renders the right thing; what the flag is for is saying so in the chrome, because a
    /// zoomed tab and a tab with one pane are otherwise indistinguishable on screen.
    pub zoomed: bool,
    /// How a pane's frames get here, when they come from another machine.
    ///
    /// On the region rather than on each of its panes because it is a property of the daemon,
    /// and every pane in a region belongs to one. `None` is a daemon on this machine, which is
    /// the only difference a shell ever has to notice between local and remote.
    pub transport: Option<Transport>,
    /// Which daemon this region's frame streams should come from, on this machine.
    ///
    /// A pane's frames arrive from a herdr CLI rather than over the control socket, and that
    /// CLI finds a daemon the way any other client does. That stopped being good enough when
    /// Muster started running its own daemon under a session of its own: a bridge left to
    /// find one reaches whatever the user last started, does not find the pane there, and the
    /// stream ends before a single frame - a pane that renders nothing.
    ///
    /// `None` for a remote region, deliberately. That bridge runs its CLI on the far machine,
    /// where a path from this one names nothing, and it finds the daemon over there the
    /// ordinary way.
    /// Named for the backend rather than for herdr, though herdr is what fills it today.
    /// This type is the core's own vocabulary and a second backend would populate the same
    /// field, so a name carrying one backend's spelling would be a field lying about which
    /// daemon it points at (`architecture.md`, swappable organs). The bridge's `--herdr-socket`
    /// flag keeps herdr's name, because that flag is herdr's CLI being invoked.
    pub backend_socket: Option<String>,
}

/// What a pane's bridge needs in order to reach another machine.
///
/// Carried across the seam rather than worked out by the shell, for the same reason a pane's
/// control socket is: it names something the core opened, and a shell that recomputed it
/// would be guessing at a path only the core knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transport {
    /// The ssh destination, as ssh spells it.
    pub host: String,
    /// The master's control socket, so a pane's frame stream rides the connection the control
    /// plane already opened instead of paying for a handshake of its own.
    pub control_path: String,
}

/// A region's tree, with the panes filled in.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewNode {
    Pane(ViewPane),
    Split {
        axis: SplitAxis,
        /// The first child's share, between 0 and 1.
        ratio: f32,
        first: Box<ViewNode>,
        second: Box<ViewNode>,
    },
}

/// One pane, as a window needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewPane {
    pub id: PaneId,
    /// Where this pane's bridge should dial the core, once there is one to dial.
    ///
    /// `None` means no channel is open for this pane yet, which is what a surface built
    /// from it would render and never be typeable in. Absent rather than empty so that a
    /// shell cannot spawn a bridge pointed at nothing and then wait for it.
    pub control_socket_path: Option<String>,

    /// What the pane's own daemon calls it, for the shell to hand its bridge.
    ///
    /// The bridge streams frames from the daemon directly, so it is the one thing above the
    /// adapter that has to speak the backend's vocabulary - and the only reason this leaves
    /// the core. A handle to relay, on the same terms as `ViewRegion::backend_socket`, and
    /// never something to address a pane by: `id` above is what Muster means by a pane
    /// everywhere else, and the two disagree the moment two daemons are attached.
    ///
    /// `None` for a pane whose daemon no longer holds it, which is a pane no bridge should be
    /// started for.
    pub backend_pane_id: Option<String>,
}

impl View {
    /// What one window is showing, given a way to reach each daemon's mirror.
    ///
    /// A closure rather than a map of mirrors, because in a running app each one is behind
    /// its own lock and the caller is the only thing that knows how to hold them. A region
    /// whose daemon answers `None` is dropped: it names a daemon nothing is following, so
    /// there is no tab to render and no honest thing to say about it.
    pub fn of<'a>(
        composition: &Composition,
        mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
        socket: impl Fn(&DaemonId, &PaneId) -> Option<String>,
        transport: impl Fn(&DaemonId) -> Option<Transport>,
        backend_socket: impl Fn(&DaemonId) -> Option<String>,
        backend_pane: impl Fn(&DaemonId, &PaneId) -> Option<String>,
    ) -> View {
        let regions = composition
            .regions()
            .filter_map(|region| {
                let held = mirror(&region.daemon)?;
                let layout =
                    held.layout(&region.tab).filter(|layout| arranges(held, region, layout));
                Some(ViewRegion {
                    id: region.id,
                    daemon: region.daemon.clone(),
                    tab: region.tab.clone(),
                    pane: region.pane.clone(),
                    weight: region.weight,
                    root: layout.map(|layout| {
                        // Resolved here rather than flagged for the shell. herdr keeps
                        // publishing every pane's ordinary rect while a tab is zoomed, so a
                        // renderer handed the whole tree paints all of them while the daemon
                        // is showing one (`observations/herdr-0.8.0.md` section 13).
                        let zoomed = layout.zoomed.clone().map(LayoutNode::Pane);
                        build(
                            zoomed.as_ref().unwrap_or(&layout.root),
                            &region.daemon,
                            &socket,
                            &backend_pane,
                        )
                    }),
                    zoomed: layout.is_some_and(|layout| layout.zoomed.is_some()),
                    transport: transport(&region.daemon),
                    backend_socket: backend_socket(&region.daemon),
                })
            })
            .collect();
        View { regions, focused: composition.focused_region().map(|region| region.id) }
    }

    pub fn region(&self, id: RegionId) -> Option<&ViewRegion> {
        self.regions.iter().find(|region| region.id == id)
    }

    /// Every pane this window has on screen, named by its daemon.
    ///
    /// What seen-ness is answered against: a pane nobody is showing cannot have been seen,
    /// however focused the window is. A tab's zoom needs no special case, because a zoomed
    /// region publishes only the zoomed pane - the tree here already is what is visible.
    ///
    /// A region whose tree has not arrived contributes nothing, which is the honest answer.
    /// Its panes may well be on screen from the last published tree, but this window has not
    /// been told they are, and guessing would mark agents seen on the strength of a
    /// arrangement the daemon has stopped describing.
    pub fn showing(&self) -> BTreeSet<PaneKey> {
        self.regions
            .iter()
            .flat_map(|region| {
                region
                    .root
                    .iter()
                    .flat_map(ViewNode::panes)
                    .map(|pane| PaneKey::new(&region.daemon, &pane.id))
            })
            .collect()
    }

    /// Where the keyboard lands after stepping one pane.
    ///
    /// Reading order across the whole window, not within a region: two regions side by side
    /// are one thing to a person looking at them, and a step that stopped at a region's edge
    /// would leave panes no keystroke could reach. It wraps, because a window has no edge to
    /// bump against and a step that silently did nothing is indistinguishable from a dead key.
    ///
    /// Here rather than in the shell because it is a decision, and because the CLI and the
    /// agent-facing API ask for it in the same words. Here rather than on [`Composition`]
    /// because the order is the tab's tree, and composition holds no tree - it is daemon truth
    /// (`architecture.md`, one action path).
    pub fn step(&self, direction: Step) -> Option<(RegionId, PaneId)> {
        if let Some(axis) = direction.axis() {
            return self.neighbour(axis, direction.towards_start());
        }
        let order: Vec<(RegionId, PaneId)> = self
            .regions
            .iter()
            .flat_map(|region| {
                region
                    .root
                    .iter()
                    .flat_map(ViewNode::panes)
                    .map(|pane| (region.id, pane.id.clone()))
            })
            .collect();
        let at = self.focused.and_then(|focused| {
            let pane = self.region(focused)?.pane.as_ref()?;
            order.iter().position(|(region, held)| *region == focused && held == pane)
        });
        match at {
            Some(at) => {
                // Only the two ordinal steps reach here; the four directions returned above.
                let step = match direction {
                    Step::Previous => order.len().checked_sub(1)?,
                    _ => 1,
                };
                order.get((at + step) % order.len()).cloned()
            }
            // The keyboard is on a pane no tree names, which is an ordinary moment rather
            // than a bug: a tab mid-split publishes its panes and its tree separately. A step
            // from nowhere goes to the end it came from rather than refusing.
            None => match direction {
                Step::Previous => order.last().cloned(),
                _ => order.first().cloned(),
            },
        }
    }

    /// The pane in a given direction from the one with the keyboard.
    ///
    /// Geometric rather than a walk up the tree, and that is the decision worth recording.
    /// Walking up to the first ancestor split on the matching axis and back down the near
    /// side is cheaper and disagrees with the screen: on a perpendicular split it has to pick
    /// a child by position in the tree rather than by where it actually is, so in any
    /// arrangement that is not symmetric it lands somewhere the user did not point at.
    ///
    /// The ratios are already here, and after region weights so is the arrangement over
    /// regions, so the whole window is one normalized space and the honest answer is a
    /// rectangle comparison. Asking the daemon was the other option and was rejected:
    /// `BackendChannel::submit` is write-only by design, and every future backend would owe
    /// us a read to answer a question about an arrangement Muster is already holding.
    ///
    /// A candidate has to be on the far side and overlap the source across the direction of
    /// travel, so nothing diagonal is reachable in one move. That is deliberate: next and
    /// previous already reach every pane and wrap, so this one can afford to be predictable
    /// instead. It does not wrap either - falling off the edge of a window and reappearing on
    /// the other side is disorienting in a way stepping through an order is not.
    fn neighbour(&self, axis: Axis, towards_start: bool) -> Option<(RegionId, PaneId)> {
        let places = self.places();
        let focused = self.focused?;
        let pane = self.region(focused)?.pane.as_ref()?;
        let from = places
            .iter()
            .find(|(region, held, _)| *region == focused && held == pane)
            .map(|(_, _, rect)| *rect)?;

        places
            .iter()
            .enumerate()
            .filter(|(_, (region, held, _))| !(*region == focused && held == pane))
            .filter_map(|(index, (region, held, rect))| {
                let gap = from.gap_to(*rect, axis, towards_start)?;
                let overlap = from.overlap(*rect, axis.across())?;
                Some((gap, overlap, index, (*region, held.clone())))
            })
            // Nearest first; then the one sharing the most edge with where the keyboard was,
            // which is what "straight on" means when two panes are the same distance away.
            // Reading order last, so the answer is the same every time rather than whichever
            // pane the tree happened to yield first.
            .min_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then(right.1.total_cmp(&left.1))
                    .then(left.2.cmp(&right.2))
            })
            .map(|(_, _, _, found)| found)
    }

    /// Where every pane on screen sits, as fractions of the window.
    ///
    /// Normalized rather than in points, because the core has no window and needs none: which
    /// pane is to the left of which is the same answer at any size.
    fn places(&self) -> Vec<(RegionId, PaneId, Rect)> {
        let total: f32 = self
            .regions
            .iter()
            .map(|region| region.weight.max(0.0))
            .filter(|w| w.is_finite())
            .sum();
        if total <= 0.0 {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut x = 0.0;
        for region in &self.regions {
            let weight = if region.weight.is_finite() { region.weight.max(0.0) } else { 0.0 };
            let width = weight / total;
            if let Some(root) = &region.root {
                place(root, Rect { x, y: 0.0, width, height: 1.0 }, region.id, &mut found);
            }
            x += width;
        }
        found
    }
}

/// Which way a rectangle is being measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    fn across(self) -> Axis {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
}

/// A pane's place in the window, as fractions of it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// How close two rectangles have to be before they count as touching.
///
/// Every number here is arrived at by dividing, so two edges that are the same line come out
/// a few bits apart. Small enough that no real arrangement is inside it: a window would have
/// to hold ten thousand panes across before two of them were this close by accident.
const TOUCHING: f32 = 1e-4;

impl Rect {
    fn span(self, axis: Axis) -> (f32, f32) {
        match axis {
            Axis::Horizontal => (self.x, self.x + self.width),
            Axis::Vertical => (self.y, self.y + self.height),
        }
    }

    /// The distance to a rectangle lying in the given direction, or `None` if it does not.
    fn gap_to(self, other: Rect, axis: Axis, towards_start: bool) -> Option<f32> {
        let (start, end) = self.span(axis);
        let (other_start, other_end) = other.span(axis);
        let gap = if towards_start { start - other_end } else { other_start - end };
        (gap >= -TOUCHING).then_some(gap.max(0.0))
    }

    /// How much of an edge two rectangles share, or `None` when they share none.
    fn overlap(self, other: Rect, axis: Axis) -> Option<f32> {
        let (start, end) = self.span(axis);
        let (other_start, other_end) = other.span(axis);
        let shared = end.min(other_end) - start.max(other_start);
        (shared > TOUCHING).then_some(shared)
    }
}

/// Cuts a rectangle up the way a tree says, down to one per pane.
fn place(node: &ViewNode, rect: Rect, region: RegionId, found: &mut Vec<(RegionId, PaneId, Rect)>) {
    match node {
        ViewNode::Pane(pane) => found.push((region, pane.id.clone(), rect)),
        ViewNode::Split { axis, ratio, first, second } => {
            // A ratio is a backend's number and is not this core's to trust. An unusable one
            // splits evenly rather than collapsing a pane to nothing, on the same terms as
            // the shell's own geometry.
            let ratio = if ratio.is_finite() { ratio.clamp(0.0, 1.0) } else { 0.5 };
            match axis {
                SplitAxis::Columns => {
                    let width = rect.width * ratio;
                    place(first, Rect { width, ..rect }, region, found);
                    let beyond = Rect { x: rect.x + width, width: rect.width - width, ..rect };
                    place(second, beyond, region, found);
                }
                SplitAxis::Rows => {
                    let height = rect.height * ratio;
                    place(first, Rect { height, ..rect }, region, found);
                    let beyond = Rect { y: rect.y + height, height: rect.height - height, ..rect };
                    place(second, beyond, region, found);
                }
            }
        }
    }
}

/// Which way a step through the window's panes goes.
///
/// Two kinds in one word, deliberately. Next and previous walk the reading order and wrap, so
/// between them they reach every pane - that is what makes them the guarantee. The four
/// directions are geometric and do not wrap, so they can be predictable instead: they go where
/// the user pointed or nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
    Left,
    Right,
    Up,
    Down,
}

impl Step {
    pub fn parse(name: &str) -> Option<Step> {
        match name {
            "next" => Some(Step::Next),
            "previous" => Some(Step::Previous),
            "left" => Some(Step::Left),
            "right" => Some(Step::Right),
            "up" => Some(Step::Up),
            "down" => Some(Step::Down),
            _ => None,
        }
    }

    /// Every step there is, so a test can assert nothing has been left unspelled.
    pub const ALL: [Step; 6] =
        [Step::Next, Step::Previous, Step::Left, Step::Right, Step::Up, Step::Down];

    pub fn as_str(self) -> &'static str {
        match self {
            Step::Next => "next",
            Step::Previous => "previous",
            Step::Left => "left",
            Step::Right => "right",
            Step::Up => "up",
            Step::Down => "down",
        }
    }

    /// The axis a direction travels along, or `None` for the two that walk an order instead.
    fn axis(self) -> Option<Axis> {
        match self {
            Step::Left | Step::Right => Some(Axis::Horizontal),
            Step::Up | Step::Down => Some(Axis::Vertical),
            Step::Next | Step::Previous => None,
        }
    }

    fn towards_start(self) -> bool {
        matches!(self, Step::Left | Step::Up)
    }
}

impl ViewNode {
    /// Every pane in the tree, in reading order.
    pub fn panes(&self) -> Vec<&ViewPane> {
        let mut found = Vec::new();
        self.collect(&mut found);
        found
    }

    fn collect<'a>(&'a self, found: &mut Vec<&'a ViewPane>) {
        match self {
            ViewNode::Pane(pane) => found.push(pane),
            ViewNode::Split { first, second, .. } => {
                first.collect(found);
                second.collect(found);
            }
        }
    }
}

/// Whether a tab's tree describes the panes that tab actually holds.
///
/// A backend publishes a tab's pane list and its arrangement as separate events, and nothing
/// orders them against each other. Two ways that shows up, both measured against herdr 0.8.0:
/// a tab mid-split briefly has a tree naming fewer panes than it holds, and a subscription
/// that has just bootstrapped replays layout events, walking a tab backwards through
/// arrangements it had minutes ago.
///
/// A tree that disagrees is withheld rather than repaired. Repairing it means inventing a
/// place to put a pane no daemon put anywhere, and rendering it as it stands means dropping
/// every pane it omits - which costs those panes their surfaces and, with them, the bridges
/// that feed them. Withholding is a state the shell already understands and already has the
/// right answer to: it leaves what it is showing alone, and the real tree arrives on its own
/// event a moment later.
fn arranges(mirror: &Mirror, region: &Region, layout: &Layout) -> bool {
    let mut arranged: Vec<&PaneId> = layout.root.panes();
    arranged.sort_unstable();
    let mut held: Vec<&PaneId> = mirror.panes_in_tab(&region.tab).map(|pane| &pane.id).collect();
    held.sort_unstable();
    arranged == held
}

fn build(
    node: &LayoutNode,
    daemon: &DaemonId,
    socket: &impl Fn(&DaemonId, &PaneId) -> Option<String>,
    backend_pane: &impl Fn(&DaemonId, &PaneId) -> Option<String>,
) -> ViewNode {
    match node {
        LayoutNode::Pane(id) => ViewNode::Pane(ViewPane {
            id: id.clone(),
            control_socket_path: socket(daemon, id),
            backend_pane_id: backend_pane(daemon, id),
        }),
        LayoutNode::Split { axis, ratio, first, second } => ViewNode::Split {
            axis: *axis,
            ratio: *ratio,
            first: Box::new(build(first, daemon, socket, backend_pane)),
            second: Box::new(build(second, daemon, socket, backend_pane)),
        },
    }
}

/// A view on one line per region, for the run log and the conformance drivers.
///
/// "the view changed" is useless in a log and the shape it changed to is the whole answer,
/// and a reviewer deciding whether an expectation is right can hold a line in their head
/// where four screens of nested JSON hold nobody's.
impl std::fmt::Display for ViewNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewNode::Pane(pane) => {
                write!(f, "{}", pane.id)?;
                // Marked rather than printed: the path carries a pid and a temporary
                // directory, so a case asserting one would assert this machine.
                if pane.control_socket_path.is_some() {
                    f.write_str("*")?;
                }
                Ok(())
            }
            ViewNode::Split { axis, ratio, first, second } => {
                let axis = match axis {
                    SplitAxis::Columns => "columns",
                    SplitAxis::Rows => "rows",
                };
                write!(f, "{axis}({first}, {second}@{ratio})")
            }
        }
    }
}
