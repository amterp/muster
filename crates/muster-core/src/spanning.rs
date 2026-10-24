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
use crate::equalize::{self, Evenly};
use crate::intent::{Branch, Side};
use crate::mirror::Mirror;
use crate::mirror::backend::{CopyNode, Health, LayoutNode, PaneId, SplitAxis};
use crate::resize;

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

/// Whether a tab spans machines: it has a part on more than one, or its one part keeps panes on
/// another. A tab that does not is its one machine's tree, with nothing to merge and nothing to
/// write, which is how nearly every tab is, so this is asked before its parts are read.
pub fn spans<'a>(tab: &MusterTab, mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>) -> bool {
    let mut regions = tab.regions();
    let Some(first) = regions.next() else { return false };
    regions.next().is_some()
        || mirror(&first.daemon)
            .and_then(|held| held.tab(&tab.id))
            .is_some_and(|layout| layout.copy.has_foreign())
}

/// A tab's parts as a window's mirrors hold them, in the order its regions sit, with which
/// machines have said all they are going to about their panes: connected, finished restoring,
/// and not stopping, so a pane they do not hold is one they do not have. A stopping daemon
/// closes every pane it holds and brings them back on its next run, and a merge that took
/// those closes at their word would drop the panes from every other machine's copy.
///
/// A machine whose mirror holds the tab, where the tab has no region onto it yet, has not
/// said either: a mirror moves on before the window reconciles what it heard, and its part of
/// the tab, with the panes it holds there, is not among the parts until then. `writable` says which machines
/// speak a protocol that holds a spanning tree.
pub fn parts_of<'a>(
    tab: &MusterTab,
    mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
    writable: impl Fn(&DaemonId) -> bool,
) -> (Vec<Part>, BTreeMap<DaemonId, bool>) {
    let settled =
        |held: &Mirror| held.health() == Health::Connected && !held.restoring() && !held.stopping();
    let mut parts = Vec::new();
    let mut said = BTreeMap::new();
    for region in tab.regions() {
        let Some(held) = mirror(&region.daemon) else { continue };
        said.insert(region.daemon.clone(), settled(held));
        let Some(layout) = held.tab(&tab.id) else { continue };
        for foreign in foreign_machines(&layout.copy) {
            if let Some(other) = mirror(&foreign)
                && !tab.daemons().any(|part| *part == foreign)
            {
                said.insert(foreign, settled(other) && other.tab(&tab.id).is_none());
            }
        }
        parts.push(Part {
            daemon: region.daemon.clone(),
            copy: layout.copy.clone(),
            zoomed: layout.copy_zoomed.clone(),
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
/// by side, at their regions' weights, which is how a window drew it before this, and is written
/// to every part that can take it, at generation one (MIP-8, section 9).
///
/// `None` for a tab with no parts.
pub fn merge(parts: &[Part], settled: &BTreeMap<DaemonId, bool>) -> Option<Merged> {
    let written = parts.iter().map(|part| part.generation).max()?;
    let (tree, zoomed) = if written == 0 { side_by_side(parts)? } else { newest(parts) };
    let holder = |pane: &PaneId| {
        parts.iter().find(|part| part.holds.contains(pane)).map(|part| part.daemon.clone())
    };
    let mut tree = pruned(tree, &holder, settled)?;
    let mut named: BTreeSet<PaneId> =
        tree.leaves().into_iter().map(|(pane, _)| pane.clone()).collect();
    for part in parts {
        for pane in panes_of(&part.copy) {
            if part.holds.contains(pane) && !named.contains(pane) {
                tree = added(tree, pane, part, &named);
                named.insert(pane.clone());
            }
        }
    }
    let zoomed = zoomed.filter(|zoomed| named.contains(zoomed));
    let writes = writes(parts, &tree, zoomed.as_ref(), written);
    Some(Merged { tree, zoomed, writes })
}

/// The copy a window wrote last, and on a tie the one naming the lowest pane of its own, a part
/// holding none of its panes yet losing to one that does: two windows merging the same copies
/// start from the same one whatever they call the machines.
fn newest(parts: &[Part]) -> (SpanNode, Option<PaneId>) {
    let newest = parts
        .iter()
        .max_by(|one, other| {
            one.generation.cmp(&other.generation).then_with(|| match (lowest(one), lowest(other)) {
                (Some(one), Some(other)) => other.cmp(one),
                (one, other) => one.is_some().cmp(&other.is_some()),
            })
        })
        .expect("merge is given at least one part");
    (spanned(&newest.copy, &newest.daemon), newest.zoomed.clone())
}

fn lowest(part: &Part) -> Option<&PaneId> {
    panes_of(&part.copy).into_iter().filter(|pane| part.holds.contains(*pane)).min()
}

/// Every part as a column, at its region's weight: a tab no window has written a tree for. Zoomed
/// on the first part's zoomed pane, in region order, so a zoomed tab is still zoomed once it is
/// written as one tree.
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
    Some((tree, parts.iter().find_map(|part| part.zoomed.clone())))
}

/// The tree a window draws of `tree`: without the panes `held` says no part of the tab it holds
/// runs, each split left with one child giving way to that child. A window reaching only some of a
/// tab's machines, or opened while one was down, draws the panes it can reach (MIP-8, section 7).
/// `None` when it holds none of them.
pub fn drawn(tree: &SpanNode, held: &impl Fn(&PaneId) -> bool) -> Option<SpanNode> {
    match tree {
        SpanNode::Pane { pane, .. } => held(pane).then(|| tree.clone()),
        SpanNode::Split { axis, ratio, first, second } => {
            match (drawn(first, held), drawn(second, held)) {
                (Some(first), Some(second)) => Some(SpanNode::Split {
                    axis: *axis,
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (only, None) | (None, only) => only,
            }
        }
    }
}

/// The way down `tree` to the split a window drew at `path` in [`drawn`]'s tree of it: a split
/// it did not draw, because one side holds nothing it runs, takes no turn of `path`.
fn whole_path(
    tree: &SpanNode,
    path: &[Branch],
    held: &impl Fn(&PaneId) -> bool,
) -> Option<Vec<Branch>> {
    let SpanNode::Split { first, second, .. } = tree else { return None };
    let drawing = |node: &SpanNode| node.leaves().into_iter().any(|(pane, _)| held(pane));
    let (branch, child, rest) = match (drawing(first), drawing(second)) {
        (true, true) => match path.split_first() {
            None => return Some(Vec::new()),
            Some((Branch::First, rest)) => (Branch::First, first, rest),
            Some((Branch::Second, rest)) => (Branch::Second, second, rest),
        },
        (true, false) => (Branch::First, first, path),
        (false, true) => (Branch::Second, second, path),
        (false, false) => return None,
    };
    let mut way = whole_path(child, rest, held)?;
    way.insert(0, branch);
    Some(way)
}

/// Which line a divider a window drew in a tab no window has written is, by the turns down to it
/// in that tab's tree, its parts side by side in region order: the column between a part and the
/// parts after it, which is the window's own region weights, or a split inside one part, which is
/// that part's own tree. How a tab whose machines are all too old to hold a spanning tree is
/// arranged (MIP-8, section 9). `None` for a path that names no split there.
pub fn side_by_side_divider(parts: &[Part], path: &[Branch]) -> Option<Divider> {
    let within = |part: &Part, path: &[Branch]| {
        splits_at(&part.copy, path)
            .then(|| Divider::Within { daemon: part.daemon.clone(), path: path.to_vec() })
    };
    let mut rest = path;
    for (index, part) in parts.iter().enumerate() {
        if index + 1 == parts.len() {
            return within(part, rest);
        }
        match rest.split_first() {
            None => return Some(Divider::Column { left: part.daemon.clone() }),
            Some((Branch::First, inner)) => return within(part, inner),
            Some((Branch::Second, further)) => rest = further,
        }
    }
    None
}

fn splits_at(copy: &CopyNode, path: &[Branch]) -> bool {
    match (copy, path.split_first()) {
        (CopyNode::Split { .. }, None) => true,
        (CopyNode::Split { first, .. }, Some((Branch::First, rest))) => splits_at(first, rest),
        (CopyNode::Split { second, .. }, Some((Branch::Second, rest))) => splits_at(second, rest),
        _ => false,
    }
}

/// A divider in a tab drawn as its parts side by side.
#[derive(Debug, Clone, PartialEq)]
pub enum Divider {
    /// The line between the part on `left` and every part after it.
    Column { left: DaemonId },
    /// A split at `path` in the tree of the part on `daemon`.
    Within { daemon: DaemonId, path: Vec<Branch> },
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

/// `tree`, whose panes are `named`, with `pane` where `part`'s copy put it: beside the first pane
/// of its sibling that the tree names, on the same side and at the same share. Else a column at
/// the right, as wide as each pane would be if they were all columns.
fn added(tree: SpanNode, pane: &PaneId, part: &Part, named: &BTreeSet<PaneId>) -> SpanNode {
    let leaf = SpanNode::Pane { pane: pane.clone(), daemon: part.daemon.clone() };
    if let Some(beside) = sibling(&part.copy, pane)
        && let Some(neighbour) =
            panes_of(beside.sibling).into_iter().find(|near| named.contains(*near))
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

/// A change to a tab's arrangement, worked out on the tab's whole tree rather than asked of one
/// machine: a part holding other machines' panes refuses those requests, since a change it made
/// on its own would race the window's next write (MIP-8, section 3).
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The split at `path`, in the tab's tree as a window draws it ([`drawn`]), to `ratio`.
    Ratio { path: Vec<Branch>, ratio: f32 },
    /// The divider nearest `pane` toward `side`, by the rule a daemon resizes its own tree by.
    Resize { pane: PaneId, side: Side, fraction: Option<f32> },
    /// Two panes trading places.
    Swap { pane: PaneId, with: PaneId },
    /// `pane` filling the tab, or nothing zoomed when it already does.
    Zoom { pane: PaneId },
    /// The panes around `pane` evened out, as [`equalize::dividers`] has it.
    Even { pane: PaneId, evenly: Evenly },
}

/// Whether a tab's arrangement is the window's to write as one tree, so every change to it goes
/// through [`rearrange`]: it spans machines, and some part's copy already holds another machine's
/// pane or some part's machine `speaks` a protocol that holds a spanning tree. A part holding
/// foreign leaves refuses a change asked of it alone, and one that can hold them is about to be
/// written them. A tab whose machines are all too old for either keeps its region weights.
pub fn written<'a>(
    tab: &MusterTab,
    mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
    speaks: impl Fn(&DaemonId) -> bool,
) -> bool {
    spans(tab, &mirror)
        && tab.regions().any(|region| {
            speaks(&region.daemon)
                || mirror(&region.daemon)
                    .and_then(|held| held.tab(&tab.id))
                    .is_some_and(|layout| layout.copy.has_foreign())
        })
}

/// Whether the daemons keep a tab's arrangement: some part's copy is one a window has written, so
/// the window's region weights for it say nothing any more.
pub fn tree_kept<'a>(tab: &MusterTab, mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>) -> bool {
    tab.regions().any(|region| {
        mirror(&region.daemon)
            .and_then(|held| held.tab(&tab.id))
            .is_some_and(|layout| layout.layout_generation > 0)
    })
}

/// The tab merged as [`merge`] has it, with `change` made to it and written to every part that
/// can take it, one generation above the newest copy. A change that leaves the tree as it was
/// writes only what the merge would have.
///
/// Refused, saying why, when the change names a pane or a split the tree does not hold, or no
/// part can take a write.
pub fn rearrange(
    parts: &[Part],
    settled: &BTreeMap<DaemonId, bool>,
    change: &Change,
) -> Result<Merged, String> {
    let merged = merge(parts, settled).ok_or("the tab has no part on any machine")?;
    let written = parts.iter().map(|part| part.generation).max().unwrap_or_default();
    let change = match change {
        Change::Ratio { path, ratio } => {
            let held = |pane: &PaneId| parts.iter().any(|part| part.holds.contains(pane));
            let path = whole_path(&merged.tree, path, &held)
                .ok_or_else(|| format!("the tab's tree has no split at {path:?}"))?;
            Change::Ratio { path, ratio: *ratio }
        }
        other => other.clone(),
    };
    let (tree, zoomed) = changed(&merged.tree, merged.zoomed.as_ref(), &change)?;
    if tree == merged.tree && zoomed == merged.zoomed {
        return Ok(merged);
    }
    let generation = written + 1;
    let writes: Vec<Write> = parts
        .iter()
        .filter(|part| part.writable)
        .map(|part| Write {
            daemon: part.daemon.clone(),
            root: tree.copy_for(&part.daemon),
            zoomed: zoomed.clone(),
            generation,
            based_on: part.generation,
        })
        .collect();
    if writes.is_empty() {
        return Err("no machine holding a part of the tab can take its tree just now: each is \
                    unreachable, still restoring, stopping, or too old to hold one"
            .to_string());
    }
    Ok(Merged { tree, zoomed, writes })
}

fn changed(
    tree: &SpanNode,
    zoomed: Option<&PaneId>,
    change: &Change,
) -> Result<(SpanNode, Option<PaneId>), String> {
    let named = |pane: &PaneId| {
        if tree.leaves().iter().any(|(leaf, _)| *leaf == pane) {
            Ok(())
        } else {
            Err(format!("the tab's tree holds no pane {pane}"))
        }
    };
    let zoomed = zoomed.cloned();
    match change {
        Change::Ratio { path, ratio } => {
            let Some(SpanNode::Split { .. }) = tree.at(path) else {
                return Err(format!("the tab's tree has no split at {path:?}"));
            };
            Ok((tree.with_ratio(path, *ratio)?, zoomed))
        }
        Change::Resize { pane, side, fraction } => {
            named(pane)?;
            let turns = tree.turns_to(pane).unwrap_or_default();
            let Some(depth) = resize::divider(&turns, *side) else {
                return Err(format!(
                    "no split on that axis holds {pane}, so there is no divider to move"
                ));
            };
            let path: Vec<Branch> = turns[..depth].iter().map(|turn| turn.branch).collect();
            let Some(SpanNode::Split { ratio, .. }) = tree.at(&path) else {
                unreachable!("the depth was chosen because a split is there")
            };
            match resize::stepped(*ratio, *side, *fraction) {
                Some(moved) => Ok((tree.with_ratio(&path, moved)?, zoomed)),
                None => Ok((tree.clone(), zoomed)),
            }
        }
        Change::Swap { pane, with } => {
            named(pane)?;
            named(with)?;
            Ok((tree.swapped(pane, with), zoomed))
        }
        Change::Zoom { pane } => {
            named(pane)?;
            let unzooming = zoomed.as_ref() == Some(pane);
            Ok((tree.clone(), (!unzooming).then(|| pane.clone())))
        }
        Change::Even { pane, evenly } => {
            named(pane)?;
            let dividers = equalize::dividers(&tree.layout(), pane, *evenly).ok_or_else(|| {
                format!("the pane {pane} sits in no {} that could be evened out", evenly.as_str())
            })?;
            let mut evened = tree.clone();
            for divider in dividers {
                evened = evened.with_ratio(&divider.path, divider.ratio)?;
            }
            Ok((evened, zoomed))
        }
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
///
/// A copy differs by its tree and its zoom, never by what it calls another machine: that name
/// is only the writing window's (MIP-8, section 1), so two windows naming one machine
/// differently would otherwise rewrite each other's copies, each a generation above the last.
fn writes(parts: &[Part], tree: &SpanNode, zoomed: Option<&PaneId>, written: u64) -> Vec<Write> {
    let wanted: Vec<(&Part, CopyNode)> = parts
        .iter()
        .filter(|part| part.writable)
        .map(|part| (part, tree.copy_for(&part.daemon)))
        .collect();
    let differs =
        |part: &Part, root: &CopyNode| !alike(&part.copy, root) || part.zoomed.as_ref() != zoomed;
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

#[expect(clippy::float_cmp, reason = "a ratio a daemon stores comes back as the bits it was sent")]
fn alike(one: &CopyNode, other: &CopyNode) -> bool {
    match (one, other) {
        (CopyNode::Own(one), CopyNode::Own(other))
        | (CopyNode::Foreign { pane: one, .. }, CopyNode::Foreign { pane: other, .. }) => {
            one == other
        }
        (
            CopyNode::Split { axis, ratio, first, second },
            CopyNode::Split {
                axis: other_axis,
                ratio: other_ratio,
                first: other_first,
                second: other_second,
            },
        ) => {
            axis == other_axis
                && ratio == other_ratio
                && alike(first, other_first)
                && alike(second, other_second)
        }
        _ => false,
    }
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

    /// The way down to `pane`: each split's axis and the child taken there.
    fn turns_to(&self, pane: &PaneId) -> Option<Vec<resize::Turn>> {
        match self {
            SpanNode::Pane { pane: leaf, .. } => (leaf == pane).then(Vec::new),
            SpanNode::Split { axis, first, second, .. } => {
                for (branch, child) in [(Branch::First, first), (Branch::Second, second)] {
                    if let Some(mut turns) = child.turns_to(pane) {
                        turns.insert(0, resize::Turn { branch, axis: *axis });
                        return Some(turns);
                    }
                }
                None
            }
        }
    }

    fn at(&self, path: &[Branch]) -> Option<&SpanNode> {
        let Some((step, rest)) = path.split_first() else { return Some(self) };
        match (self, step) {
            (SpanNode::Split { first, .. }, Branch::First) => first.at(rest),
            (SpanNode::Split { second, .. }, Branch::Second) => second.at(rest),
            (SpanNode::Pane { .. }, _) => None,
        }
    }

    /// This tree with the split at `path` at `ratio`.
    fn with_ratio(&self, path: &[Branch], ratio: f32) -> Result<SpanNode, String> {
        if !(ratio.is_finite() && ratio > 0.0 && ratio < 1.0) {
            return Err(format!("a ratio is strictly between 0 and 1, and {ratio} is not"));
        }
        match (self, path.split_first()) {
            (SpanNode::Split { axis, first, second, .. }, None) => Ok(SpanNode::Split {
                axis: *axis,
                ratio,
                first: first.clone(),
                second: second.clone(),
            }),
            (SpanNode::Split { axis, ratio: held, first, second }, Some((step, rest))) => {
                let (first, second) = match step {
                    Branch::First => (Box::new(first.with_ratio(rest, ratio)?), second.clone()),
                    Branch::Second => (first.clone(), Box::new(second.with_ratio(rest, ratio)?)),
                };
                Ok(SpanNode::Split { axis: *axis, ratio: *held, first, second })
            }
            (SpanNode::Pane { .. }, _) => Err(format!("the tab's tree has no split at {path:?}")),
        }
    }

    /// This tree with `one` and `other` in each other's places, each leaf keeping its machine.
    fn swapped(&self, one: &PaneId, other: &PaneId) -> SpanNode {
        let leaf = |pane: &PaneId| {
            self.leaves()
                .into_iter()
                .find(|(leaf, _)| *leaf == pane)
                .map(|(pane, daemon)| SpanNode::Pane { pane: pane.clone(), daemon: daemon.clone() })
        };
        let (Some(first), Some(second)) = (leaf(one), leaf(other)) else { return self.clone() };
        self.mapped(&|pane| {
            if pane == one {
                Some(second.clone())
            } else if pane == other {
                Some(first.clone())
            } else {
                None
            }
        })
    }

    /// This tree with each leaf `swap` answers for put in its place.
    fn mapped(&self, swap: &impl Fn(&PaneId) -> Option<SpanNode>) -> SpanNode {
        match self {
            SpanNode::Pane { pane, .. } => swap(pane).unwrap_or_else(|| self.clone()),
            SpanNode::Split { axis, ratio, first, second } => SpanNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(first.mapped(swap)),
                second: Box::new(second.mapped(swap)),
            },
        }
    }

    /// The tree with each leaf a pane alone, which is how a daemon's own tree is shaped: pane ids
    /// are unique across machines, so the arithmetic written for one machine's tree reads it.
    fn layout(&self) -> LayoutNode {
        match self {
            SpanNode::Pane { pane, .. } => LayoutNode::Pane(pane.clone()),
            SpanNode::Split { axis, ratio, first, second } => LayoutNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(first.layout()),
                second: Box::new(second.layout()),
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
