//! One tree over a tab that spans machines, merged from the copy each machine keeps (MIP-8).
//!
//! Every daemon holding a part of a tab keeps the whole tab's tree, with the panes on other
//! machines as foreign leaves it never runs. No daemon knows another exists, so only a window
//! sees every copy, and this is where it decides what they add up to: start from the copy a
//! window wrote last, drop a pane only on its own machine's word, add a pane a machine holds
//! that the tree lacks where that machine's copy put it, and say which copies to write back.
//!
//! Pure: copies, generations and what each machine holds in, a tree and the writes out. The seam
//! reads them out of its mirrors and sends the writes; the view draws the tree.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::{DaemonId, MusterTab};
use crate::mirror::Mirror;
use crate::mirror::backend::{CopyNode, Health, PaneId, SplitAxis};

/// A tab's tree over every machine it spans, each leaf naming the machine running its pane.
#[derive(Debug, Clone, PartialEq)]
pub enum SpanNode {
    Pane { pane: PaneId, daemon: DaemonId },
    Split { axis: SplitAxis, ratio: f32, first: Box<SpanNode>, second: Box<SpanNode> },
}

/// One machine's part of a tab, as its mirror holds it.
#[derive(Debug, Clone)]
pub struct Part {
    pub daemon: DaemonId,
    pub copy: CopyNode,
    pub zoomed: Option<PaneId>,
    pub generation: u64,
    /// The panes this machine holds in the tab.
    pub holds: BTreeSet<PaneId>,
    /// Its share of the tab's width, as the window's region for it has it: what a tab no window
    /// has written yet is arranged by.
    pub weight: f32,
    /// Whether this machine can be written to: it answers, has finished restoring what it saved,
    /// and speaks a protocol that knows a spanning tree.
    pub writable: bool,
}

/// What the copies add up to.
#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub tree: SpanNode,
    pub zoomed: Option<PaneId>,
    /// The copies to write, so that every writable part holds `tree`.
    pub writes: Vec<Write>,
}

/// One part's copy, as the merged tree has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    pub daemon: DaemonId,
    pub root: CopyNode,
    pub zoomed: Option<PaneId>,
    pub generation: u64,
    pub based_on: u64,
}

/// A tab's parts as a window's mirrors hold them, in the order its regions sit, with which
/// machines have said all they are going to about their panes: connected, and finished
/// restoring, so a pane they do not hold is one they do not have. `writable` says which machines
/// speak a protocol that holds a spanning tree.
pub fn parts_of<'a>(
    tab: &MusterTab,
    mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
    writable: impl Fn(&DaemonId) -> bool,
) -> (Vec<Part>, BTreeMap<DaemonId, bool>) {
    let settled = |held: &Mirror| held.health() == Health::Connected && !held.restoring();
    let mut parts = Vec::new();
    let mut said = BTreeMap::new();
    for region in tab.regions() {
        let Some(held) = mirror(&region.daemon) else { continue };
        said.insert(region.daemon.clone(), settled(held));
        let Some(layout) = held.tab(&tab.id) else { continue };
        for foreign in foreign_machines(&layout.copy) {
            if let Some(other) = mirror(&foreign) {
                said.insert(foreign, settled(other));
            }
        }
        parts.push(Part {
            daemon: region.daemon.clone(),
            copy: layout.copy.clone(),
            zoomed: layout.zoomed.clone(),
            generation: layout.layout_generation,
            holds: held.panes_in_tab(&tab.id).map(|pane| pane.id.clone()).collect(),
            weight: region.weight,
            writable: settled(held) && writable(&region.daemon),
        });
    }
    (parts, said)
}

fn foreign_machines(copy: &CopyNode) -> Vec<DaemonId> {
    match copy {
        CopyNode::Own(_) => Vec::new(),
        CopyNode::Foreign { daemon, .. } => vec![DaemonId::new(daemon)],
        CopyNode::Split { first, second, .. } => {
            let mut machines = foreign_machines(first);
            machines.extend(foreign_machines(second));
            machines
        }
    }
}

/// Merges a tab's parts, in the order the window's regions for them sit. `settled` says, for
/// every machine the window knows, whether it is connected and has finished restoring: a leaf is
/// dropped only when its machine is settled and does not hold its pane.
///
/// A tab whose copies no window has written - every generation zero - starts from its parts side
/// by side, at their regions' weights, which is how a window drew it before this. Nothing is
/// written back for such a tab: until a window sends a spanning tab's every change as a whole
/// tree, a part holding foreign leaves would refuse the resize, zoom and swap a window still
/// sends it (MIP-8, "How it lands").
///
/// `None` for a tab with no parts.
pub fn merge(parts: &[Part], settled: &BTreeMap<DaemonId, bool>) -> Option<Merged> {
    let written = parts.iter().map(|part| part.generation).max()?;
    let (tree, zoomed) = if written == 0 { side_by_side(parts)? } else { newest(parts) };
    let holder = |pane: &PaneId| {
        parts.iter().find(|part| part.holds.contains(pane)).map(|part| part.daemon.clone())
    };
    let mut tree = pruned(tree, &holder, settled)?;
    for part in parts {
        for pane in panes_of(&part.copy) {
            if part.holds.contains(pane) && !tree.names(pane) {
                tree = added(tree, pane, part);
            }
        }
    }
    let zoomed = zoomed.filter(|zoomed| tree.names(zoomed));
    let writes =
        if written == 0 { Vec::new() } else { writes(parts, &tree, zoomed.as_ref(), written) };
    Some(Merged { tree, zoomed, writes })
}

/// The copy a window wrote last, and on a tie the one naming the lowest pane of its own: two
/// windows merging the same copies start from the same one whatever they call the machines.
fn newest(parts: &[Part]) -> (SpanNode, Option<PaneId>) {
    let newest = parts
        .iter()
        .max_by(|one, other| {
            one.generation.cmp(&other.generation).then_with(|| lowest(other).cmp(&lowest(one)))
        })
        .expect("merge is given at least one part");
    (spanned(&newest.copy, &newest.daemon), newest.zoomed.clone())
}

fn lowest(part: &Part) -> Option<&PaneId> {
    panes_of(&part.copy).into_iter().filter(|pane| part.holds.contains(*pane)).min()
}

/// Every part as a column, at its region's weight: a tab no window has written a tree for.
fn side_by_side(parts: &[Part]) -> Option<(SpanNode, Option<PaneId>)> {
    let (last, rest) = parts.split_last()?;
    let mut tree = spanned(&last.copy, &last.daemon);
    let mut after = last.weight;
    for part in rest.iter().rev() {
        let ratio = part.weight / (part.weight + after);
        tree = SpanNode::Split {
            axis: SplitAxis::Columns,
            ratio: if ratio.is_finite() && ratio > 0.0 && ratio < 1.0 { ratio } else { 0.5 },
            first: Box::new(spanned(&part.copy, &part.daemon)),
            second: Box::new(tree),
        };
        after += part.weight;
    }
    Some((tree, None))
}

/// A copy with every leaf naming its machine: its own daemon's for its own panes, and the name
/// the copy was written with for another machine's.
fn spanned(copy: &CopyNode, daemon: &DaemonId) -> SpanNode {
    match copy {
        CopyNode::Own(pane) => SpanNode::Pane { pane: pane.clone(), daemon: daemon.clone() },
        CopyNode::Foreign { pane, daemon } => {
            SpanNode::Pane { pane: pane.clone(), daemon: DaemonId::new(daemon) }
        }
        CopyNode::Split { axis, ratio, first, second } => SpanNode::Split {
            axis: *axis,
            ratio: *ratio,
            first: Box::new(spanned(first, daemon)),
            second: Box::new(spanned(second, daemon)),
        },
    }
}

/// The tree without the leaves whose machine has said it does not hold their pane, each kept
/// leaf naming the machine that does hold it. A name a copy was written with is only the
/// writing window's, so the machine found holding a pane wins over it.
fn pruned(
    tree: SpanNode,
    holder: &impl Fn(&PaneId) -> Option<DaemonId>,
    settled: &BTreeMap<DaemonId, bool>,
) -> Option<SpanNode> {
    match tree {
        SpanNode::Pane { pane, daemon } => match holder(&pane) {
            Some(holder) => Some(SpanNode::Pane { pane, daemon: holder }),
            None if settled.get(&daemon) == Some(&true) => None,
            None => Some(SpanNode::Pane { pane, daemon }),
        },
        SpanNode::Split { axis, ratio, first, second } => {
            match (pruned(*first, holder, settled), pruned(*second, holder, settled)) {
                (Some(first), Some(second)) => Some(SpanNode::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (only, None) | (None, only) => only,
            }
        }
    }
}

/// `tree` with `pane` where `part`'s copy put it: beside the first pane of its sibling that the
/// tree names, on the same side and at the same share. Else a column at the right, as wide as
/// each pane would be if they were all columns.
fn added(tree: SpanNode, pane: &PaneId, part: &Part) -> SpanNode {
    let leaf = SpanNode::Pane { pane: pane.clone(), daemon: part.daemon.clone() };
    if let Some(beside) = sibling(&part.copy, pane)
        && let Some(neighbour) = panes_of(beside.sibling).into_iter().find(|near| tree.names(near))
    {
        let neighbour = neighbour.clone();
        return tree.replacing(&neighbour, &mut |near| {
            let (first, second) =
                if beside.first { (leaf.clone(), near) } else { (near, leaf.clone()) };
            SpanNode::Split {
                axis: beside.axis,
                ratio: beside.ratio,
                first: Box::new(first),
                second: Box::new(second),
            }
        });
    }
    #[expect(clippy::cast_precision_loss, reason = "a tab holds a handful of panes")]
    let held = tree.leaves().len() as f32;
    SpanNode::Split {
        axis: SplitAxis::Columns,
        ratio: held / (held + 1.0),
        first: Box::new(tree),
        second: Box::new(leaf),
    }
}

/// Where a pane sits in a copy: the split holding it, whether it is that split's first child,
/// and the other child.
struct Beside<'a> {
    axis: SplitAxis,
    ratio: f32,
    first: bool,
    sibling: &'a CopyNode,
}

fn sibling<'a>(copy: &'a CopyNode, pane: &PaneId) -> Option<Beside<'a>> {
    let CopyNode::Split { axis, ratio, first, second } = copy else { return None };
    let is = |node: &CopyNode| matches!(node, CopyNode::Own(own) if own == pane);
    if is(first) {
        return Some(Beside { axis: *axis, ratio: *ratio, first: true, sibling: second });
    }
    if is(second) {
        return Some(Beside { axis: *axis, ratio: *ratio, first: false, sibling: first });
    }
    sibling(first, pane).or_else(|| sibling(second, pane))
}

/// The copies to send so that every writable part holds `tree`. At the generation already
/// written when every copy that differs is behind it, so catching up a lagging part does not
/// rewrite the rest; one above it when a part already at that generation holds something else.
fn writes(parts: &[Part], tree: &SpanNode, zoomed: Option<&PaneId>, written: u64) -> Vec<Write> {
    let wanted: Vec<(&Part, CopyNode)> = parts
        .iter()
        .filter(|part| part.writable)
        .map(|part| (part, tree.copy_for(&part.daemon)))
        .collect();
    let differs =
        |part: &Part, root: &CopyNode| part.copy != *root || part.zoomed.as_ref() != zoomed;
    let generation =
        if wanted.iter().any(|(part, root)| differs(part, root) && part.generation >= written) {
            written + 1
        } else {
            written
        };
    wanted
        .into_iter()
        .filter(|(part, root)| differs(part, root) || part.generation < generation)
        .map(|(part, root)| Write {
            daemon: part.daemon.clone(),
            root,
            zoomed: zoomed.cloned(),
            generation,
            based_on: part.generation,
        })
        .collect()
}

fn panes_of(copy: &CopyNode) -> Vec<&PaneId> {
    match copy {
        CopyNode::Own(pane) | CopyNode::Foreign { pane, .. } => vec![pane],
        CopyNode::Split { first, second, .. } => {
            let mut panes = panes_of(first);
            panes.extend(panes_of(second));
            panes
        }
    }
}

impl SpanNode {
    /// Every leaf, in reading order, with the machine running it.
    pub fn leaves(&self) -> Vec<(&PaneId, &DaemonId)> {
        match self {
            SpanNode::Pane { pane, daemon } => vec![(pane, daemon)],
            SpanNode::Split { first, second, .. } => {
                let mut leaves = first.leaves();
                leaves.extend(second.leaves());
                leaves
            }
        }
    }

    pub fn names(&self, pane: &PaneId) -> bool {
        self.leaves().iter().any(|(leaf, _)| *leaf == pane)
    }

    /// Whether any two leaves are on different machines.
    pub fn spans(&self) -> bool {
        let machines: BTreeSet<&DaemonId> =
            self.leaves().into_iter().map(|(_, daemon)| daemon).collect();
        machines.len() > 1
    }

    /// The copy `daemon` keeps of this tree: its own panes, and every other machine's as a
    /// foreign leaf under the name this window gives that machine.
    pub fn copy_for(&self, daemon: &DaemonId) -> CopyNode {
        match self {
            SpanNode::Pane { pane, daemon: holder } if holder == daemon => {
                CopyNode::Own(pane.clone())
            }
            SpanNode::Pane { pane, daemon: holder } => {
                CopyNode::Foreign { pane: pane.clone(), daemon: holder.as_str().to_string() }
            }
            SpanNode::Split { axis, ratio, first, second } => CopyNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(first.copy_for(daemon)),
                second: Box::new(second.copy_for(daemon)),
            },
        }
    }

    fn replacing(self, pane: &PaneId, with: &mut impl FnMut(SpanNode) -> SpanNode) -> SpanNode {
        match self {
            SpanNode::Pane { pane: ref leaf, .. } if leaf == pane => with(self),
            SpanNode::Pane { .. } => self,
            SpanNode::Split { axis, ratio, first, second } => SpanNode::Split {
                axis,
                ratio,
                first: Box::new(first.replacing(pane, with)),
                second: Box::new(second.replacing(pane, with)),
            },
        }
    }
}

/// `columns(p1@laptop, rows(p2@devenv, p3@laptop@0.5)@0.5)`, as the conformance cases spell it.
impl std::fmt::Display for SpanNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanNode::Pane { pane, daemon } => write!(f, "{pane}@{daemon}"),
            SpanNode::Split { axis, ratio, first, second } => {
                let axis = match axis {
                    SplitAxis::Columns => "columns",
                    SplitAxis::Rows => "rows",
                };
                write!(f, "{axis}({first}, {second}@{ratio})")
            }
        }
    }
}
