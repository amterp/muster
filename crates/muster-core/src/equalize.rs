//! Evening out the panes in a tab, as the dividers that would do it.
//!
//! The arithmetic somebody arranging a window should not have to do. Every divider in a tab is
//! already addressable one at a time, and an agent with no eyes could reach all of them and still
//! not know where to put any: working that out means holding the tree, counting what hangs off
//! each side, and dividing - which is what this does once, here, rather than in every caller
//! (kan a_2KIH8WAnU).
//!
//! Nothing here asks a daemon anything or changes anything. It answers with the divider positions
//! that would make a set of panes equal, and the caller sends them as ordinary
//! [`crate::BackendIntent::SetSplitRatio`] requests - so this is a pure function over a tree, with
//! no world behind it and nothing to stand up in order to test it.

use crate::intent::Branch;
use crate::mirror::backend::{Layout, LayoutNode, PaneId, SplitAxis};

/// Which panes an equalize evens out.
///
/// The two narrow answers are spelled the way somebody looking at a window would spell them,
/// which is the opposite way round from [`SplitAxis`]: a *row* of panes sits side by side, and
/// side by side is a `Columns` split. That axis says how the children are laid out, not which way
/// the split was made, so the two names cross over exactly once - here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evenly {
    /// Every pane in the tab, each ending with the same area.
    Tab,
    /// The panes side by side with this one.
    Row,
    /// The panes stacked with this one.
    Column,
}

impl Evenly {
    /// Every scope there is, so a test can assert nothing has been left unspelled.
    pub const ALL: [Evenly; 3] = [Evenly::Tab, Evenly::Row, Evenly::Column];

    pub fn parse(name: &str) -> Option<Evenly> {
        match name {
            "tab" => Some(Evenly::Tab),
            "row" => Some(Evenly::Row),
            "column" => Some(Evenly::Column),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Evenly::Tab => "tab",
            Evenly::Row => "row",
            Evenly::Column => "column",
        }
    }

    /// The split axis this scope runs along, or `None` for the one that crosses both.
    fn axis(self) -> Option<SplitAxis> {
        match self {
            Evenly::Row => Some(SplitAxis::Columns),
            Evenly::Column => Some(SplitAxis::Rows),
            Evenly::Tab => None,
        }
    }
}

/// One divider, and the share an equalize wants its first child to have.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    /// The turns from the tab's root, as [`crate::BackendIntent::SetSplitRatio`] takes them.
    pub path: Vec<Branch>,
    pub ratio: f32,
}

/// How close a divider has to be to where it is wanted before it is left alone.
///
/// A backend divides its own rectangle in cells, so a share this small does not move a divider
/// by a whole cell in any window worth using - and a request that moves nothing still costs a
/// round trip and a republished tree. Being slightly generous is what makes evening an already
/// even tab free rather than a burst of requests that change nothing.
const SETTLED: f32 = 1e-3;

/// Where the dividers would have to sit for these panes to come out equal.
///
/// `None` when there is nothing to answer: a tree that does not name this pane, or a scope the
/// pane does not sit in - a pane with nothing beside it is in no row. Both are refusals rather
/// than empty answers, because "nothing to do" is a real and different outcome and an empty list
/// is how it is spelled: a tab already even answers `Some(vec![])`.
///
/// A tree that does not name the pane is worth refusing rather than working around. It means the
/// arrangement in hand is behind the tab it describes - a tab publishes its panes and its tree on
/// separate events - and moving dividers by a stale tree puts them somewhere nobody asked for.
pub fn dividers(layout: &Layout, pane: &PaneId, evenly: Evenly) -> Option<Vec<Divider>> {
    let path = path_to(&layout.root, pane)?;
    let mut found = Vec::new();
    match evenly.axis() {
        // Every divider, both axes, by how many panes hang off each side. Each subtree then gets
        // the share of its parent that its pane count deserves, so by the time it reaches a leaf
        // every pane holds the same area however the tree above it mixes rows and columns.
        None => {
            even(&layout.root, &mut Vec::new(), &mut found);
            Some(found)
        }
        Some(axis) => {
            let (start, run) = run_holding(&layout.root, &path, axis)?;
            even_along(run, axis, &mut start.to_vec(), &mut found);
            Some(found)
        }
    }
}

/// The outermost node of the run of one axis this path passes through, and the way to it.
///
/// The run is what somebody means by "the row this pane is in": the nearest split that lays panes
/// out along that axis, and then as far up as the axis keeps holding. In `columns(columns(a, b), c)`
/// pane `a`'s row is all three, because the two `columns` splits are one arrangement wearing two
/// nodes. In `columns(a, rows(b, c))` it is `a` and the pair, because a row of two things is what
/// that is.
fn run_holding<'a>(
    root: &'a LayoutNode,
    path: &'a [Branch],
    axis: SplitAxis,
) -> Option<(&'a [Branch], &'a LayoutNode)> {
    let ancestors = axes_along(root, path);
    let deepest = ancestors.iter().rposition(|held| *held == axis)?;
    let mut top = deepest;
    while top > 0 && ancestors[top - 1] == axis {
        top -= 1;
    }
    Some((&path[..top], at(root, &path[..top])?))
}

/// The axis of every split this path turns at, outermost first.
fn axes_along(root: &LayoutNode, path: &[Branch]) -> Vec<SplitAxis> {
    let mut found = Vec::new();
    let mut node = root;
    for turn in path {
        let LayoutNode::Split { axis, first, second, .. } = node else { break };
        found.push(*axis);
        node = if *turn == Branch::First { first } else { second };
    }
    found
}

/// The node these turns lead to.
fn at<'a>(root: &'a LayoutNode, path: &[Branch]) -> Option<&'a LayoutNode> {
    let mut node = root;
    for turn in path {
        let LayoutNode::Split { first, second, .. } = node else { return None };
        node = if *turn == Branch::First { first } else { second };
    }
    Some(node)
}

/// The turns from here down to a pane, or `None` when this subtree does not hold it.
fn path_to(node: &LayoutNode, pane: &PaneId) -> Option<Vec<Branch>> {
    match node {
        LayoutNode::Pane(id) => (id == pane).then(Vec::new),
        LayoutNode::Split { first, second, .. } => {
            let (turn, mut path) = match path_to(first, pane) {
                Some(path) => (Branch::First, path),
                None => (Branch::Second, path_to(second, pane)?),
            };
            path.insert(0, turn);
            Some(path)
        }
    }
}

/// Every divider under this node, placed so that every pane comes out the same size.
fn even(node: &LayoutNode, path: &mut Vec<Branch>, found: &mut Vec<Divider>) {
    let LayoutNode::Split { ratio, first, second, .. } = node else { return };
    wanted(path, *ratio, share(leaves(first), leaves(node)), found);
    descend(first, second, path, found, even);
}

/// The dividers of one axis under this node, placed so that its members come out the same size.
fn even_along(
    node: &LayoutNode,
    axis: SplitAxis,
    path: &mut Vec<Branch>,
    found: &mut Vec<Divider>,
) {
    let LayoutNode::Split { axis: held, ratio, first, second } = node else { return };
    if *held != axis {
        return;
    }
    wanted(path, *ratio, share(members(first, axis), members(node, axis)), found);
    descend(first, second, path, found, |node, path, found| {
        even_along(node, axis, path, found);
    });
}

/// Both children in turn, each with its own turn on the path.
fn descend(
    first: &LayoutNode,
    second: &LayoutNode,
    path: &mut Vec<Branch>,
    found: &mut Vec<Divider>,
    each: impl Fn(&LayoutNode, &mut Vec<Branch>, &mut Vec<Divider>),
) {
    for (turn, child) in [(Branch::First, first), (Branch::Second, second)] {
        path.push(turn);
        each(child, path, found);
        path.pop();
    }
}

/// Notes a divider, unless it is already where it is wanted.
fn wanted(path: &[Branch], ratio: f32, share: f32, found: &mut Vec<Divider>) {
    // A ratio that is not a number is never close to anything, so it is replaced rather than
    // kept - which is the same answer the view already gives a ratio it cannot lay out.
    if (ratio - share).abs() < SETTLED {
        return;
    }
    found.push(Divider { path: path.to_vec(), ratio: share });
}

/// Every pane under this node, however it is split.
fn leaves(node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Pane(_) => 1,
        LayoutNode::Split { first, second, .. } => leaves(first) + leaves(second),
    }
}

/// How many things sit side by side under this node, along one axis.
///
/// A subtree split the other way counts as one. It is a single member of the row - evening a row
/// out is about how wide each thing in it is, and what is inside one of them is a different
/// question with a different answer.
fn members(node: &LayoutNode, axis: SplitAxis) -> usize {
    match node {
        LayoutNode::Split { axis: held, first, second, .. } if *held == axis => {
            members(first, axis) + members(second, axis)
        }
        _ => 1,
    }
}

/// A count of panes as a share of another.
///
/// The cast is exact for anything a window could hold: f32 counts whole numbers exactly to
/// sixteen million, and a tab that deep has no divider anybody is evening out.
#[allow(clippy::cast_precision_loss)]
fn share(part: usize, whole: usize) -> f32 {
    part as f32 / whole as f32
}
