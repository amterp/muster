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

use crate::composition::record::{Composition, DaemonId, RegionId};
use crate::mirror::Mirror;
use crate::mirror::backend::{LayoutNode, PaneId, SplitAxis, TabId};

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
    ) -> View {
        let regions = composition
            .regions()
            .filter_map(|region| {
                let held = mirror(&region.daemon)?;
                let layout = held.layout(&region.tab);
                Some(ViewRegion {
                    id: region.id,
                    daemon: region.daemon.clone(),
                    tab: region.tab.clone(),
                    pane: region.pane.clone(),
                    root: layout.map(|layout| {
                        // Resolved here rather than flagged for the shell. herdr keeps
                        // publishing every pane's ordinary rect while a tab is zoomed, so a
                        // renderer handed the whole tree paints all of them while the daemon
                        // is showing one (`observations/herdr-0.8.0.md` section 13).
                        let zoomed = layout.zoomed.clone().map(LayoutNode::Pane);
                        build(zoomed.as_ref().unwrap_or(&layout.root), &region.daemon, &socket)
                    }),
                    zoomed: layout.is_some_and(|layout| layout.zoomed.is_some()),
                })
            })
            .collect();
        View { regions, focused: composition.focused_region().map(|region| region.id) }
    }

    pub fn region(&self, id: RegionId) -> Option<&ViewRegion> {
        self.regions.iter().find(|region| region.id == id)
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

fn build(
    node: &LayoutNode,
    daemon: &DaemonId,
    socket: &impl Fn(&DaemonId, &PaneId) -> Option<String>,
) -> ViewNode {
    match node {
        LayoutNode::Pane(id) => {
            ViewNode::Pane(ViewPane { id: id.clone(), control_socket_path: socket(daemon, id) })
        }
        LayoutNode::Split { axis, ratio, first, second } => ViewNode::Split {
            axis: *axis,
            ratio: *ratio,
            first: Box::new(build(first, daemon, socket)),
            second: Box::new(build(second, daemon, socket)),
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
