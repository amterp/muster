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

use crate::attention::GroupKey;
use crate::composition::record::{Composition, DaemonId, MusterTab, PaneKey, Region};
use crate::mirror::Mirror;
use crate::mirror::backend::{LayoutNode, PaneId, SplitAxis, Tab, TabId};
use crate::spanning::{self, SpanNode};

/// Everything one window is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// The Muster tab on screen, or nothing when this window holds none it may open onto.
    ///
    /// One at a time, which is what a window holding an ordered list of tabs means (MIP-2).
    pub tab: Option<TabId>,
    /// The tab's one tree over every machine it spans, as a window draws it: merged from each
    /// machine's copy (`crate::spanning`, MIP-8) without the panes no part this window holds runs,
    /// and a tab on one machine taking its one copy as it is. The whole tree, even while the tab is
    /// zoomed.
    ///
    /// `None` while no machine has said how the tab is arranged. A real state rather than a
    /// failure - a pane moving between tabs is counted in the tab it left until that tab's tree
    /// changes - and a distinct one from a tab with no panes: a shell told `None` leaves what it
    /// has alone, where a shell told "no panes" would tear down surfaces that are about to be
    /// described.
    pub tree: Option<ViewTree>,
    /// The pane filling the tab, when it is zoomed: the one the keyboard is on, wherever in the
    /// tab it runs, which is how a window draws a zoom.
    pub zoomed: Option<PaneKey>,
    /// The pane Muster's keyboard feeds.
    pub keyboard: Option<PaneKey>,
    /// Every machine holding a part of the tab on screen that this window follows, with where its
    /// panes' bridges dial, in the order its regions sit.
    pub machines: Vec<ViewMachine>,
    zoom: Zoom,
    /// The group whose conversation the window shows in place of its panes (MIP-9, section 7).
    /// The tab stays as it was underneath, with the keyboard where it was, so leaving puts both
    /// back; but while it is shown, no pane is on screen.
    pub conversation: Option<GroupKey>,
    /// Every pane this window has on screen, named by its daemon.
    ///
    /// Held rather than derived from `tree`, because it is not the same question and reading it
    /// off the tree got it wrong. A tab on screen has its panes on screen; the tree says how they
    /// are arranged, and a tree the daemon has not published or has just contradicted does not
    /// put those panes away. A derivation that walked the trees answered "nothing at all" for a
    /// tab the window was still drawing, which is how a pane painting frames came to report
    /// itself hidden.
    ///
    /// A zoom is the one thing that hides a pane without closing its tab, so a zoomed tab
    /// contributes the one pane filling it.
    showing: BTreeSet<PaneKey>,
}

/// A machine a tab's panes run on, as a window's bridges reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewMachine {
    pub daemon: DaemonId,
    /// The socket a pane's bridge dials to draw it: the daemon's own, or for a daemon on another
    /// machine the local end of the ssh forward to it. `None` for a daemon this window has not
    /// reached.
    pub socket: Option<String>,
    /// Whether the daemon is on another machine, which is where a bridge asks for a window of
    /// unacknowledged output sized for the link.
    pub remote: bool,
}

/// A tab's tree over every machine it spans, with the panes filled in and each leaf naming the
/// machine running it.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewTree {
    Pane { daemon: DaemonId, pane: ViewPane },
    Split { axis: SplitAxis, ratio: f32, first: Box<ViewTree>, second: Box<ViewTree> },
}

impl ViewTree {
    fn of(tree: &SpanNode, pane: &impl Fn(&DaemonId, &PaneId) -> ViewPane) -> ViewTree {
        match tree {
            SpanNode::Pane { pane: id, daemon } => {
                ViewTree::Pane { daemon: daemon.clone(), pane: pane(daemon, id) }
            }
            SpanNode::Split { axis, ratio, first, second } => ViewTree::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(ViewTree::of(first, pane)),
                second: Box::new(ViewTree::of(second, pane)),
            },
        }
    }
}

impl ViewTree {
    /// One machine's tree, every leaf its pane.
    fn own(
        node: &LayoutNode,
        daemon: &DaemonId,
        pane: &impl Fn(&DaemonId, &PaneId) -> ViewPane,
    ) -> ViewTree {
        match node {
            LayoutNode::Pane(id) => {
                ViewTree::Pane { daemon: daemon.clone(), pane: pane(daemon, id) }
            }
            LayoutNode::Split { axis, ratio, first, second } => ViewTree::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(ViewTree::own(first, daemon, pane)),
                second: Box::new(ViewTree::own(second, daemon, pane)),
            },
        }
    }
}

/// `columns(p1@laptop, x@devenv@0.5)`, as `crate::spanning::SpanNode` is written.
impl std::fmt::Display for ViewTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewTree::Pane { daemon, pane } => write!(f, "{}@{daemon}", pane.id),
            ViewTree::Split { axis, ratio, first, second } => {
                let axis = match axis {
                    SplitAxis::Columns => "columns",
                    SplitAxis::Rows => "rows",
                };
                write!(f, "{axis}({first}, {second}@{ratio})")
            }
        }
    }
}

/// One pane, as a window needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewPane {
    pub id: PaneId,
    /// The socket this pane's bridge reports on, once the core has bound one.
    ///
    /// `None` means none is bound yet, and a surface built from it would have a bridge the
    /// window cannot hear from. Absent rather than empty so that a shell cannot spawn a bridge
    /// pointed at nothing and then wait for it.
    pub link_socket_path: Option<String>,

    /// How big this pane's text is, in points away from what the config file asked for.
    ///
    /// Here rather than on a message of its own, although it is chrome and not daemon truth,
    /// for the reason `RosterPane::on_screen` is resolved rather than left to the shell: a
    /// shell that had to join two messages would render a pane at the wrong size for as long
    /// as they disagreed. A surface is built from a leaf, so the size belongs on the leaf.
    ///
    /// Zero is the ordinary answer and means the size the config file named, or the one the
    /// renderer chose when it named none.
    pub font_size_offset: i32,

    /// How many times this window has replaced this pane's bridge, counting from zero.
    ///
    /// Two things the shell reads off one number. It builds a new surface whenever this
    /// changes, because a bridge is a surface's command and there is no other way to start
    /// one; and a number that is not zero says this window has had a bridge for this pane
    /// before, which is when taking the terminal over is a re-attach rather than stealing it
    /// from another window (`crate::respawn`).
    pub bridge_restarts: u32,
}

impl View {
    /// What one window is showing, given a way to reach each daemon's mirror.
    ///
    /// A closure rather than a map of mirrors, because in a running app each one is behind
    /// its own lock and the caller is the only thing that knows how to hold them. A part of the
    /// tab whose daemon answers `None` is left out: it names a daemon nothing is following, so
    /// there is nothing to render and no honest thing to say about it.
    ///
    /// `pane` answers everything about one pane that the layout cannot: the socket its bridge
    /// reports on, the size somebody chose, and how many times its bridge has been replaced.
    /// One closure rather than one per field, because several of the same shape in a row is a
    /// call site nobody can check.
    ///
    /// It answers with the id it was handed. Not enforced here, because putting it back would
    /// cost a clone per pane on a path with a budget against it (`perf/baseline.json`,
    /// `view.build`) to guard a mistake that has to be made on purpose.
    pub fn of<'a>(
        composition: &Composition,
        mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
        daemon_socket: impl Fn(&DaemonId) -> Option<String>,
        remote: impl Fn(&DaemonId) -> bool,
        pane: impl Fn(&DaemonId, &PaneId) -> ViewPane,
    ) -> View {
        match composition.showing().and_then(|tab| composition.tab(tab)) {
            Some(tab) => View::built(tab, Zoom::Resolved, mirror, daemon_socket, remote, pane),
            None => View {
                tab: None,
                tree: None,
                zoomed: None,
                keyboard: None,
                machines: Vec::new(),
                zoom: Zoom::Resolved,
                conversation: None,
                showing: BTreeSet::new(),
            },
        }
    }

    /// How any tab this window holds is arranged, whether or not it is on screen: what `of`
    /// would answer once it was shown, except that its places are the whole tree's even while it
    /// is zoomed.
    ///
    /// The tree rather than the zoomed pane, because this answers how the tab is laid out rather
    /// than what is drawn, and a zoom covers that arrangement without changing it - unzooming
    /// puts it back. `zoomed` still says which pane fills it.
    ///
    /// `None` for a tab this window does not hold. Built on request, for a caller describing the
    /// layout, and never on the path that publishes a view.
    pub fn arranged<'a>(
        composition: &Composition,
        tab: &TabId,
        mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
        daemon_socket: impl Fn(&DaemonId) -> Option<String>,
        remote: impl Fn(&DaemonId) -> bool,
        pane: impl Fn(&DaemonId, &PaneId) -> ViewPane,
    ) -> Option<View> {
        let tab = composition.tab(tab)?;
        Some(View::built(tab, Zoom::Kept, mirror, daemon_socket, remote, pane))
    }

    fn built<'a>(
        held: &MusterTab,
        zoom: Zoom,
        mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
        daemon_socket: impl Fn(&DaemonId) -> Option<String>,
        remote: impl Fn(&DaemonId) -> bool,
        pane: impl Fn(&DaemonId, &PaneId) -> ViewPane,
    ) -> View {
        let tab = held.id.clone();
        let arranged = |region: &Region| {
            let daemon = mirror(&region.daemon)?;
            daemon.tab(&tab).filter(|layout| arranges(daemon, &tab, layout))
        };
        let keyboard = held
            .focused_region()
            .and_then(|region| Some(PaneKey::new(&region.daemon, region.pane.as_ref()?)));
        let (tree, zoomed) = if spanning::spans(held, &mirror) {
            spanning_tree(held, &mirror, keyboard.as_ref(), &pane)
        } else {
            let one = held.regions().next();
            let layout = one.and_then(|region| Some((region, arranged(region)?)));
            let zoomed = layout.and_then(|(region, layout)| {
                Some(PaneKey::new(&region.daemon, &zoom_filling(region, Some(layout))?))
            });
            let tree =
                layout.map(|(region, layout)| ViewTree::own(&layout.root, &region.daemon, &pane));
            (tree, zoomed)
        };
        // What the tab has on screen, which is the panes it holds rather than the tree it was
        // last told about. The tree decides the arrangement and a zoom decides what is covered;
        // neither absence puts a pane away, and reading this off the tree is what once made four
        // panes painting frames report themselves hidden.
        let showing = match zoomed.as_ref().filter(|_| zoom == Zoom::Resolved) {
            Some(filling) => BTreeSet::from([filling.clone()]),
            None => held
                .regions()
                .filter_map(|region| Some((&region.daemon, mirror(&region.daemon)?)))
                .flat_map(|(daemon, held)| {
                    held.panes_in_tab(&tab).map(move |pane| PaneKey::new(daemon, &pane.id))
                })
                .collect(),
        };
        let mut machines: Vec<ViewMachine> = Vec::new();
        for region in held.regions() {
            if mirror(&region.daemon).is_some()
                && machines.iter().all(|machine| machine.daemon != region.daemon)
            {
                machines.push(ViewMachine {
                    daemon: region.daemon.clone(),
                    socket: daemon_socket(&region.daemon),
                    remote: remote(&region.daemon),
                });
            }
        }
        View { tab: Some(tab), tree, zoomed, keyboard, machines, zoom, conversation: None, showing }
    }

    /// This view with a group's conversation shown in place of its panes, or as it is for
    /// `None`.
    ///
    /// Covered panes are not on screen, which is all attention needs to count them unseen: an
    /// agent that finishes under a conversation is announced and keeps its done. Had only the
    /// shell hidden them, the core would count them seen, and the finish would go unannounced.
    #[must_use]
    pub fn covered_by(mut self, conversation: Option<GroupKey>) -> View {
        if conversation.is_some() {
            self.showing.clear();
        }
        self.conversation = conversation;
        self
    }

    /// Every pane this window has on screen, named by its daemon.
    ///
    /// Two things read it and they are the same question: what a row in the agent list says
    /// about a pane, and what seen-ness is answered against - a pane nobody is showing cannot
    /// have been seen, however focused the window is.
    ///
    /// This used to be read off the published trees, and that is the bug it is written down for.
    /// A tree is how a tab's panes are arranged; it is not which panes exist, and it goes absent
    /// for reasons that have nothing to do with what is drawn - a tab whose arrangement has not
    /// arrived, and a tree the daemon has contradicted since. A window in either state is still
    /// drawing the panes it had, and reported every one of them hidden, which is a pane painting
    /// frames and telling an agent that it needs surfacing.
    pub fn showing(&self) -> &BTreeSet<PaneKey> {
        &self.showing
    }

    /// Where the keyboard lands after stepping one pane.
    ///
    /// Reading order across the tab's whole tree, not within one machine's panes: a laptop pane
    /// beside a devenv pane is one thing to a person looking at them, and a step that stopped at
    /// a machine's edge would leave panes no keystroke could reach. It wraps, because a window has no edge to
    /// bump against and a step that silently did nothing is indistinguishable from a dead key.
    ///
    /// Here rather than in the shell because it is a decision, and because the CLI and the
    /// agent-facing API ask for it in the same words. Here rather than on [`Composition`]
    /// because the order is the tab's tree, and composition holds no tree - it is daemon truth
    /// (`architecture.md`, one action path).
    pub fn step(&self, direction: Step) -> Option<PaneKey> {
        if let Some(axis) = direction.axis() {
            return self.neighbour(axis, direction.towards_start());
        }
        let order: Vec<PaneKey> = self.places().into_iter().map(|(pane, _)| pane).collect();
        let at = self
            .keyboard
            .as_ref()
            .and_then(|keyboard| order.iter().position(|pane| pane == keyboard));
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
    /// The ratios are already here, and the tab is one tree over every machine it spans, so the
    /// whole window is one normalized space and the honest answer is a rectangle comparison. Asking the daemon was the other option and was rejected:
    /// `BackendChannel::submit` is write-only by design, and every future backend would owe
    /// us a read to answer a question about an arrangement Muster is already holding.
    ///
    /// A candidate has to be on the far side and overlap the source across the direction of
    /// travel, so nothing diagonal is reachable in one move. That is deliberate: next and
    /// previous already reach every pane and wrap, so this one can afford to be predictable
    /// instead. It does not wrap either - falling off the edge of a window and reappearing on
    /// the other side is disorienting in a way stepping through an order is not.
    fn neighbour(&self, axis: Axis, towards_start: bool) -> Option<PaneKey> {
        let places = self.places();
        let keyboard = self.keyboard.as_ref()?;
        let from = places.iter().find(|(pane, _)| pane == keyboard).map(|(_, rect)| *rect)?;

        places
            .iter()
            .enumerate()
            .filter(|(_, (pane, _))| pane != keyboard)
            .filter_map(|(index, (pane, rect))| {
                let gap = from.gap_to(*rect, axis, towards_start)?;
                let overlap = from.overlap(*rect, axis.across())?;
                Some((gap, overlap, index, pane.clone()))
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

    /// Where every pane on screen sits, as fractions of the tab, in reading order: the one pane
    /// filling a zoomed tab while it is drawn that way, and the whole tree otherwise.
    ///
    /// Normalized rather than in points, because the core has no window and needs none: which
    /// pane is to the left of which is the same answer at any size.
    ///
    /// Public because `muster window` answers with it, and that is the same question the four
    /// directional moves ask: an agent arranging a window has no eyes, and the shares each pane
    /// ended up with are what tells it whether a resize landed. One answer for both, rather than
    /// a second walk of the same tree that could disagree with what the keyboard does.
    pub fn places(&self) -> Vec<(PaneKey, Rect)> {
        let whole = Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
        match (self.zoom, &self.zoomed) {
            (Zoom::Resolved, Some(filling)) => vec![(filling.clone(), whole)],
            _ => self.tree.as_ref().map(|tree| places_in(tree, whole)).unwrap_or_default(),
        }
    }
}

/// A tab spanning machines as a window draws it: the merge of its copies, without the panes no
/// part this window holds runs, and the pane filling it when it is zoomed.
fn spanning_tree<'a>(
    held: &MusterTab,
    mirror: &impl Fn(&DaemonId) -> Option<&'a Mirror>,
    keyboard: Option<&PaneKey>,
    pane: &impl Fn(&DaemonId, &PaneId) -> ViewPane,
) -> (Option<ViewTree>, Option<PaneKey>) {
    let (parts, settled) = spanning::parts_of(held, mirror, |_| false);
    let Some(merged) = spanning::merge(&parts, &settled) else { return (None, None) };
    let ran = |pane: &PaneId| parts.iter().any(|part| part.holds.contains(pane));
    let drawn = spanning::drawn(&merged.tree, &ran);
    let in_tree = |key: &PaneKey| {
        drawn.as_ref().is_some_and(|drawn| {
            drawn
                .leaves()
                .into_iter()
                .any(|(pane, daemon)| *pane == key.pane && *daemon == key.daemon)
        })
    };
    // A tab no window has written yet has a zoom per part, and fills with the keyboard's pane only
    // while the part the keyboard is in is zoomed, as it was drawn before it had one tree.
    let zoomed = if parts.iter().any(|part| part.generation > 0) {
        merged.zoomed.as_ref().and_then(|zoomed| {
            keyboard.cloned().filter(in_tree).or_else(|| {
                let (pane, daemon) =
                    merged.tree.leaves().into_iter().find(|(pane, _)| *pane == zoomed)?;
                Some(PaneKey::new(daemon, pane))
            })
        })
    } else {
        held.focused_region().and_then(|region| {
            let layout = mirror(&region.daemon)?.tab(&held.id);
            Some(PaneKey::new(&region.daemon, &zoom_filling(region, layout)?))
        })
    };
    let zoomed = zoomed.filter(in_tree);
    (drawn.map(|drawn| ViewTree::of(&drawn, pane)), zoomed)
}

/// Whether a zoomed tab is placed as the one pane filling it, or kept as its whole tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zoom {
    Resolved,
    Kept,
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
///
/// Fractions and never points, on the rule the whole backend contract follows: a layout is
/// proportions, and a daemon publishes a tree of ratios rather than rectangles, because cells
/// on its side would describe a grid that is nobody's window (`architecture.md`, the
/// vocabulary). `x` and `y` are from the window's top
/// left, and the tab is one tree over every machine it spans - so two panes on two machines are
/// measured in one space, which is the only way "which of these is wider" has an answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
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

/// Cuts a rectangle up the way a tree says, down to one rectangle per pane.
///
/// Public for a tree no window holds: with no window answering, `muster window --layout` places
/// a daemon's panes in their tab with this, so its fractions are the window's own arithmetic
/// rather than a copy of it.
pub fn places_in(root: &ViewTree, rect: Rect) -> Vec<(PaneKey, Rect)> {
    let mut found = Vec::new();
    place(root, rect, &mut found);
    found
}

fn place(node: &ViewTree, rect: Rect, found: &mut Vec<(PaneKey, Rect)>) {
    match node {
        ViewTree::Pane { daemon, pane } => found.push((PaneKey::new(daemon, &pane.id), rect)),
        ViewTree::Split { axis, ratio, first, second } => {
            // A ratio is a backend's number and is not this core's to trust. An unusable one
            // splits evenly rather than collapsing a pane to nothing, on the same terms as
            // the shell's own geometry.
            let ratio = if ratio.is_finite() { ratio.clamp(0.0, 1.0) } else { 0.5 };
            match axis {
                SplitAxis::Columns => {
                    let width = rect.width * ratio;
                    place(first, Rect { width, ..rect }, found);
                    let beyond = Rect { x: rect.x + width, width: rect.width - width, ..rect };
                    place(second, beyond, found);
                }
                SplitAxis::Rows => {
                    let height = rect.height * ratio;
                    place(first, Rect { height, ..rect }, found);
                    let beyond = Rect { y: rect.y + height, height: rect.height - height, ..rect };
                    place(second, beyond, found);
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

/// The one pane filling a region, when its tab is zoomed - and the whole tab, when that region
/// holds the keyboard (`View::of`).
///
/// Which pane that is is this window's own answer first: the keyboard feeds `region.pane`, and a
/// zoom showing anything else is somebody typing into a pane they cannot see. So moving the
/// keyboard inside a zoomed tab moves what fills it, and the daemon's zoomed pane decides only
/// for a region whose keyboard is on no pane.
///
/// `None` for a region that is not zoomed and for one whose tree the daemon has not published -
/// which is the same answer for a different reason, and the right one for both: a zoom is the
/// only thing that covers a pane without closing its tab, and a tree nobody has published covers
/// nothing at all.
///
/// Public because the seam asks the same question about the same region: a socket is bound per
/// pane a shell will build a surface for, so the narrowing here and the binding there have to
/// be one answer rather than two that agree.
pub fn zoom_filling(region: &Region, layout: Option<&Tab>) -> Option<PaneId> {
    let layout = layout?;
    layout.zoomed.as_ref()?;
    region.pane.clone().or_else(|| layout.zoomed.clone())
}

/// Whether a tab's tree describes the panes that tab actually holds.
///
/// A pane moving between two tabs changes both trees, in two events: between them the pane is
/// named by the tree it now sits in and still counted in the tab it left, so that tab's tree
/// names fewer panes than it holds.
///
/// A tree that disagrees is withheld rather than repaired. Repairing it means inventing a
/// place to put a pane no daemon put anywhere, and rendering it as it stands means dropping
/// every pane it omits - which costs those panes their surfaces and, with them, the bridges
/// that feed them. Withholding is a state the shell already understands and already has the
/// right answer to: it leaves what it is showing alone, and the real tree arrives on its own
/// event a moment later.
fn arranges(mirror: &Mirror, tab: &TabId, layout: &Tab) -> bool {
    let mut arranged: Vec<&PaneId> = layout.root.panes();
    arranged.sort_unstable();
    let mut held: Vec<&PaneId> = mirror.panes_in_tab(tab).map(|pane| &pane.id).collect();
    held.sort_unstable();
    arranged == held
}
