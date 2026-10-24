//! Which divider a resize moves, and how far: one rule for every tree a pane sits in.
//!
//! A daemon resizes its own tree, and a window resizes the whole tree of a tab spanning machines
//! (MIP-8), which the daemons refuse to change one machine at a time. Both ask here, so a chord
//! moves a divider the same way whether one machine holds the tab or several. Neither tree's
//! type is named: the caller describes the way down to the pane, and gets back which split on
//! that way moves and the ratio it moves to.

use crate::intent::{Branch, Side};
use crate::mirror::backend::SplitAxis;

/// How far one resize moves a divider when the request does not say.
pub const DEFAULT_STEP: f32 = 0.05;

/// The most one resize moves a divider.
///
/// herdr's figure (`observations/herdr-0.8.0.md` section 19), kept so a resize chord moves the
/// divider as far as it did before the daemon was Muster's own.
pub const LARGEST_STEP: f32 = 0.5;

/// The closest a divider comes to either edge, also herdr's.
pub const NEAREST_EDGE: f32 = 0.1;

/// One step on the way from a tree's root down to a pane: which child it takes, and the axis of
/// the split it takes it at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub branch: Branch,
    pub axis: SplitAxis,
}

/// The depth, along `path`, of the split a resize toward `side` moves.
///
/// The divider on `side`'s side of the pane when there is one, else the one on the other side -
/// so "resize right" moves a divider right whichever side of it the pane is on. Of each, the
/// nearest the pane. `None` when no split on `side`'s axis holds the pane.
pub fn divider(path: &[Turn], side: Side) -> Option<usize> {
    let axis = axis_of(side);
    let toward = if after(side) { Branch::First } else { Branch::Second };
    let nearest =
        |taking: Branch| path.iter().rposition(|turn| turn.branch == taking && turn.axis == axis);
    nearest(toward).or_else(|| nearest(opposite(toward)))
}

/// Where a divider at `ratio` goes on one resize toward `side`, `fraction` of its split's area
/// at a time or [`DEFAULT_STEP`] when that is `None`. `None` when it is already as far as it goes.
pub fn stepped(ratio: f32, side: Side, fraction: Option<f32>) -> Option<f32> {
    let step = fraction.unwrap_or(DEFAULT_STEP).abs().min(LARGEST_STEP);
    let moved = if after(side) { ratio + step } else { ratio - step };
    let moved = moved.clamp(NEAREST_EDGE, 1.0 - NEAREST_EDGE);
    ((moved - ratio).abs() > f32::EPSILON).then_some(moved)
}

fn axis_of(side: Side) -> SplitAxis {
    match side {
        Side::Left | Side::Right => SplitAxis::Columns,
        Side::Up | Side::Down => SplitAxis::Rows,
    }
}

/// Whether `side` is the second child of a split on its axis.
fn after(side: Side) -> bool {
    matches!(side, Side::Right | Side::Down)
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

    fn turn(branch: Branch, axis: SplitAxis) -> Turn {
        Turn { branch, axis }
    }

    #[test]
    fn the_divider_on_the_side_asked_for_wins_over_the_one_behind() {
        // a | (b / c) | d, resizing b: a column split on each side of it.
        let path = [
            turn(Branch::Second, SplitAxis::Columns),
            turn(Branch::First, SplitAxis::Columns),
            turn(Branch::First, SplitAxis::Rows),
        ];
        assert_eq!(divider(&path, Side::Right), Some(1));
        assert_eq!(divider(&path, Side::Left), Some(0));
        assert_eq!(divider(&path, Side::Down), Some(2));
        assert_eq!(divider(&path, Side::Up), Some(2));
    }

    #[test]
    fn a_pane_with_no_split_on_the_axis_has_no_divider() {
        assert_eq!(divider(&[turn(Branch::First, SplitAxis::Rows)], Side::Right), None);
        assert_eq!(divider(&[], Side::Up), None);
    }

    #[track_caller]
    fn assert_near(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("the divider moved");
        assert!((actual - expected).abs() < 1e-6, "{actual} is not {expected}");
    }

    #[test]
    fn a_step_is_capped_and_stops_short_of_the_edge() {
        assert_near(stepped(0.5, Side::Right, None), 0.55);
        assert_near(stepped(0.5, Side::Up, Some(0.1)), 0.4);
        assert_near(stepped(0.5, Side::Down, Some(10.0)), 0.9);
        assert_eq!(stepped(0.9, Side::Down, None), None);
    }
}
