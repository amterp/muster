//! A tab's panes as a binary tree of splits.
//!
//! The daemon holds the tree itself rather than rectangles, and sends it as a tree: a client
//! draws it at whatever size its window is, and nobody reconstructs a shape from coordinates
//! (MIP-3, section 2). Pure: nothing here knows a PTY exists.

/// How a split divides its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Axis {
    /// Side by side; `first` is the left child.
    Columns,
    /// One above the other; `first` is the upper child.
    Rows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    fn axis(self) -> Axis {
        match self {
            Side::Left | Side::Right => Axis::Columns,
            Side::Up | Side::Down => Axis::Rows,
        }
    }

    /// Whether this side is the second child of a split on its axis.
    fn is_after(self) -> bool {
        matches!(self, Side::Right | Side::Down)
    }
}

/// Which child a step down the tree takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Branch {
    First,
    Second,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Node {
    Pane(String),
    Split {
        axis: Axis,
        /// The first child's share of the area, strictly between 0 and 1.
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// What a resize did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resized {
    Moved,
    /// The divider was already as far as it goes.
    AtLimit,
    /// No split on that axis holds the pane.
    NoDivider,
    Absent,
}

/// How far one resize moves a divider when the request does not say.
const DEFAULT_STEP: f32 = 0.05;
/// The most one resize moves a divider, and the closest a divider comes to either edge.
///
/// herdr's figures (`observations/herdr-0.8.0.md` section 19), kept so a resize chord moves the
/// divider as far as it did before the daemon changed.
const LARGEST_STEP: f32 = 0.5;
const NEAREST_EDGE: f32 = 0.1;

impl Node {
    pub(crate) fn contains(&self, pane: &str) -> bool {
        self.path_to(pane).is_some()
    }

    /// Every pane, first child before second: left to right and top to bottom.
    pub(crate) fn panes(&self) -> Vec<&str> {
        let mut found = Vec::new();
        self.collect(&mut found);
        found
    }

    fn collect<'a>(&'a self, into: &mut Vec<&'a str>) {
        match self {
            Node::Pane(name) => into.push(name),
            Node::Split { first, second, .. } => {
                first.collect(into);
                second.collect(into);
            }
        }
    }

    /// Puts `new` on `side` of `beside`, with `beside` keeping `ratio` of the area they share.
    /// False when `beside` is not in this tree.
    pub(crate) fn insert(&mut self, beside: &str, new: &str, side: Side, ratio: f32) -> bool {
        let Some(path) = self.path_to(beside) else { return false };
        let leaf = self.at_mut(&path).expect("the path was just found");
        let existing = Box::new(Node::Pane(beside.to_string()));
        let added = Box::new(Node::Pane(new.to_string()));
        *leaf = if side.is_after() {
            Node::Split { axis: side.axis(), ratio, first: existing, second: added }
        } else {
            Node::Split { axis: side.axis(), ratio: 1.0 - ratio, first: added, second: existing }
        };
        true
    }

    /// This tree without `pane`, its sibling taking the parent split's place. `None` when the
    /// pane was the whole tree. Also says whether the pane was here at all.
    pub(crate) fn without(self, pane: &str) -> (Option<Node>, bool) {
        match self {
            Node::Pane(name) if name == pane => (None, true),
            leaf @ Node::Pane(_) => (Some(leaf), false),
            Node::Split { axis, ratio, first, second } => {
                let (first, found) = first.without(pane);
                if found {
                    let rest = match first {
                        Some(first) => split(axis, ratio, first, *second),
                        None => *second,
                    };
                    return (Some(rest), true);
                }
                let first = first.expect("a pane not found leaves its subtree whole");
                let (second, found) = second.without(pane);
                match second {
                    Some(second) => (Some(split(axis, ratio, first, second)), found),
                    None => (Some(first), found),
                }
            }
        }
    }

    /// Exchanges two panes' places. False unless both are here.
    pub(crate) fn swap(&mut self, one: &str, other: &str) -> bool {
        if !self.contains(one) || !self.contains(other) {
            return false;
        }
        self.rename_each(&mut |name| {
            if name == one {
                other.to_string()
            } else if name == other {
                one.to_string()
            } else {
                name.to_string()
            }
        });
        true
    }

    fn rename_each(&mut self, rename: &mut impl FnMut(&str) -> String) {
        match self {
            Node::Pane(name) => *name = rename(name),
            Node::Split { first, second, .. } => {
                first.rename_each(rename);
                second.rename_each(rename);
            }
        }
    }

    /// Moves the split at `path` to `ratio`. `Ok(false)` when it was already there.
    pub(crate) fn set_ratio(&mut self, path: &[Branch], ratio: f32) -> Result<bool, String> {
        if !(ratio.is_finite() && ratio > 0.0 && ratio < 1.0) {
            return Err(format!("a ratio is strictly between 0 and 1, and {ratio} is not"));
        }
        match self.at_mut(path) {
            Some(Node::Split { ratio: current, .. }) => {
                let changed = (*current - ratio).abs() > f32::EPSILON;
                *current = ratio;
                Ok(changed)
            }
            Some(Node::Pane(_)) | None => Err(format!("there is no split at {path:?}")),
        }
    }

    /// Moves the divider nearest `pane` on `direction`'s axis, in that direction.
    ///
    /// The divider is the one on `direction`'s side of the pane when there is one, else the one
    /// on the other side - so "resize right" moves a divider right whichever side of it the pane
    /// is on. `fraction` is a share of that split's area.
    pub(crate) fn resize(&mut self, pane: &str, direction: Side, fraction: Option<f32>) -> Resized {
        let Some(path) = self.path_to(pane) else { return Resized::Absent };
        let axis = direction.axis();
        let toward = if direction.is_after() { Branch::First } else { Branch::Second };
        let ancestors = |taking: Branch| {
            (0..path.len()).rev().find(|&depth| {
                path[depth] == taking
                    && matches!(self.at(&path[..depth]), Some(Node::Split { axis: found, .. }) if *found == axis)
            })
        };
        let Some(depth) = ancestors(toward).or_else(|| ancestors(opposite(toward))) else {
            return Resized::NoDivider;
        };
        let step = fraction.unwrap_or(DEFAULT_STEP).abs().min(LARGEST_STEP);
        let Some(Node::Split { ratio, .. }) = self.at_mut(&path[..depth]) else {
            unreachable!("the depth was chosen because a split is there")
        };
        let moved = if direction.is_after() { *ratio + step } else { *ratio - step };
        let moved = moved.clamp(NEAREST_EDGE, 1.0 - NEAREST_EDGE);
        if (moved - *ratio).abs() <= f32::EPSILON {
            return Resized::AtLimit;
        }
        *ratio = moved;
        Resized::Moved
    }

    fn path_to(&self, pane: &str) -> Option<Vec<Branch>> {
        match self {
            Node::Pane(name) => (name == pane).then(Vec::new),
            Node::Split { first, second, .. } => {
                for (branch, child) in [(Branch::First, first), (Branch::Second, second)] {
                    if let Some(mut path) = child.path_to(pane) {
                        path.insert(0, branch);
                        return Some(path);
                    }
                }
                None
            }
        }
    }

    fn at(&self, path: &[Branch]) -> Option<&Node> {
        let Some((step, rest)) = path.split_first() else { return Some(self) };
        match (self, step) {
            (Node::Split { first, .. }, Branch::First) => first.at(rest),
            (Node::Split { second, .. }, Branch::Second) => second.at(rest),
            (Node::Pane(_), _) => None,
        }
    }

    fn at_mut(&mut self, path: &[Branch]) -> Option<&mut Node> {
        let Some((step, rest)) = path.split_first() else { return Some(self) };
        match (self, step) {
            (Node::Split { first, .. }, Branch::First) => first.at_mut(rest),
            (Node::Split { second, .. }, Branch::Second) => second.at_mut(rest),
            (Node::Pane(_), _) => None,
        }
    }
}

fn split(axis: Axis, ratio: f32, first: Node, second: Node) -> Node {
    Node::Split { axis, ratio, first: Box::new(first), second: Box::new(second) }
}

fn opposite(branch: Branch) -> Branch {
    match branch {
        Branch::First => Branch::Second,
        Branch::Second => Branch::First,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(name: &str) -> Node {
        Node::Pane(name.to_string())
    }

    /// `a | b` at `ratio`.
    fn columns(ratio: f32, first: Node, second: Node) -> Node {
        split(Axis::Columns, ratio, first, second)
    }

    fn rows(ratio: f32, first: Node, second: Node) -> Node {
        split(Axis::Rows, ratio, first, second)
    }

    /// The same shape, with ratios equal to within float rounding.
    #[track_caller]
    fn assert_shaped(actual: &Node, expected: &Node) {
        fn same(actual: &Node, expected: &Node) -> bool {
            match (actual, expected) {
                (Node::Pane(a), Node::Pane(b)) => a == b,
                (
                    Node::Split { axis, ratio, first, second },
                    Node::Split { axis: axis_b, ratio: ratio_b, first: first_b, second: second_b },
                ) => {
                    axis == axis_b
                        && (ratio - ratio_b).abs() < 1e-5
                        && same(first, first_b)
                        && same(second, second_b)
                }
                _ => false,
            }
        }
        assert!(same(actual, expected), "got {actual:?}\nwanted {expected:?}");
    }

    #[test]
    fn a_pane_goes_on_the_side_asked_and_the_existing_pane_keeps_its_share() {
        let cases = [
            (Side::Right, columns(0.7, pane("a"), pane("b"))),
            (Side::Down, rows(0.7, pane("a"), pane("b"))),
            (Side::Left, columns(1.0 - 0.7, pane("b"), pane("a"))),
            (Side::Up, rows(1.0 - 0.7, pane("b"), pane("a"))),
        ];
        for (side, expected) in cases {
            let mut tree = pane("a");
            assert!(tree.insert("a", "b", side, 0.7));
            assert_shaped(&tree, &expected);
        }
    }

    #[test]
    fn inserting_beside_a_nested_pane_splits_only_that_leaf() {
        let mut tree = columns(0.5, pane("a"), pane("b"));
        assert!(tree.insert("b", "c", Side::Down, 0.5));
        assert_shaped(&tree, &columns(0.5, pane("a"), rows(0.5, pane("b"), pane("c"))));
        assert_eq!(tree.panes(), ["a", "b", "c"]);
        assert!(!tree.insert("missing", "d", Side::Down, 0.5));
    }

    #[test]
    fn removing_a_pane_hands_its_place_to_its_sibling() {
        let tree = columns(0.3, pane("a"), rows(0.6, pane("b"), pane("c")));
        assert_eq!(tree.clone().without("b"), (Some(columns(0.3, pane("a"), pane("c"))), true));
        assert_eq!(tree.clone().without("a"), (Some(rows(0.6, pane("b"), pane("c"))), true));
        assert_eq!(tree.clone().without("missing"), (Some(tree.clone()), false));
        assert_eq!(pane("a").without("a"), (None, true));
    }

    #[test]
    fn swapping_exchanges_places_and_leaves_ratios_alone() {
        let mut tree = columns(0.3, pane("a"), rows(0.6, pane("b"), pane("c")));
        assert!(tree.swap("a", "c"));
        assert_shaped(&tree, &columns(0.3, pane("c"), rows(0.6, pane("b"), pane("a"))));
        assert!(!tree.swap("a", "missing"));
    }

    #[test]
    fn a_ratio_is_set_by_the_turns_to_its_split() {
        let mut tree = columns(0.5, pane("a"), rows(0.5, pane("b"), pane("c")));
        assert_eq!(tree.set_ratio(&[Branch::Second], 0.25), Ok(true));
        assert_eq!(tree.set_ratio(&[Branch::Second], 0.25), Ok(false));
        assert_shaped(&tree, &columns(0.5, pane("a"), rows(0.25, pane("b"), pane("c"))));
        assert!(tree.set_ratio(&[Branch::First], 0.5).is_err(), "a pane is not a split");
        assert!(tree.set_ratio(&[], 1.0).is_err(), "1 leaves the second child nothing");
        assert!(tree.set_ratio(&[], f32::NAN).is_err());
    }

    #[test]
    fn resizing_moves_the_divider_on_the_named_side_in_that_direction() {
        let mut tree = columns(0.5, pane("a"), pane("b"));
        assert_eq!(tree.resize("a", Side::Right, Some(0.1)), Resized::Moved);
        assert_shaped(&tree, &columns(0.6, pane("a"), pane("b")));
        // `b` has no neighbour to its right, so the divider on its left moves right instead.
        assert_eq!(tree.resize("b", Side::Right, Some(0.1)), Resized::Moved);
        assert_shaped(&tree, &columns(0.7, pane("a"), pane("b")));
        assert_eq!(tree.resize("a", Side::Left, None), Resized::Moved);
        assert_shaped(&tree, &columns(0.65, pane("a"), pane("b")));
    }

    #[test]
    fn resizing_takes_the_nearest_split_on_the_axis() {
        let mut tree = columns(0.5, pane("a"), columns(0.5, pane("b"), pane("c")));
        assert_eq!(tree.resize("b", Side::Right, Some(0.1)), Resized::Moved);
        assert_shaped(&tree, &columns(0.5, pane("a"), columns(0.6, pane("b"), pane("c"))));
        assert_eq!(tree.resize("b", Side::Left, Some(0.1)), Resized::Moved);
        assert_shaped(&tree, &columns(0.4, pane("a"), columns(0.6, pane("b"), pane("c"))));
    }

    #[test]
    fn a_resize_saturates_and_then_says_so() {
        let mut tree = rows(0.5, pane("a"), pane("b"));
        assert_eq!(tree.resize("a", Side::Down, Some(10.0)), Resized::Moved);
        assert_shaped(&tree, &rows(0.9, pane("a"), pane("b")));
        assert_eq!(tree.resize("a", Side::Down, None), Resized::AtLimit);
        assert_eq!(tree.resize("a", Side::Right, None), Resized::NoDivider);
        assert_eq!(tree.resize("missing", Side::Down, None), Resized::Absent);
    }
}
