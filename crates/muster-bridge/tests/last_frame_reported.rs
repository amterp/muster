//! A pane that echoed the last thing typed into it is not accused of having stopped painting.
//!
//! The false alarm on the other side of `frame_silence.rs` (kan a_2PeXwg4fA). A bridge reports
//! its frames at most once per interval, and it used to send that report only when a frame
//! arrived - so frames landing inside an interval waited for the next frame to carry them. The
//! last keystrokes before somebody paused were echoed, painted and never reported, and ten
//! seconds later the window said the pane had stopped painting. The next keystroke took it back.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use std::time::Duration;

use herdr_harness::until;
use support::{Press, Typing, named_pane, problems};

/// Long enough that a trailing report, which lands within a quarter of a second of the frame it
/// carries, cannot miss it on a loaded machine. Short enough that the gate does not wait out the
/// shipped ten seconds.
const DEADLINE: &str = "1000";

/// How long "and nothing was raised" waits before it counts as true. There is no event for a
/// warning not arriving, so a negative costs elapsed time (`docs/testing.md`): two deadlines,
/// measured from after the last keystroke was already on screen.
const SETTLE: Duration = Duration::from_secs(2);

#[test]
fn a_pane_that_echoed_the_last_keystroke_is_not_accused() {
    // SAFETY: nothing else in this process reads the environment concurrently. This runs before
    // the core is started, which is when it reads this.
    unsafe { std::env::set_var("MUSTER_PAINTING_DEADLINE_MS", DEADLINE) };

    let typing = Typing::start("");
    let key = format!("pane:local/{}:painting", named_pane(&typing.pane));
    typing.run("cat", "cat");

    Press::new("KeyQ", "q").send();
    until(
        "the first keystroke to be echoed",
        || typing.bridge.lines().iter().any(|line| line == "q"),
        || typing.bridge.diagnosis("the first keystroke never came back"),
    );

    // Long enough for the report of that echo to reach the core, and well inside the interval
    // it opened, so the second echo is a frame that interval has to hold. If the machine is too
    // slow for that, the second echo is reported on its own and this test passes without
    // proving anything - it cannot fail for timing, only lose its power to catch the bug.
    std::thread::sleep(Duration::from_millis(50));
    Press::new("KeyZ", "z").send();
    until(
        "the second keystroke to be echoed",
        || typing.bridge.lines().iter().any(|line| line == "qz"),
        || typing.bridge.diagnosis("the second keystroke never came back"),
    );

    std::thread::sleep(SETTLE);
    assert!(
        !problems().contains(&key),
        "the pane echoed both keystrokes and the window still says it stopped painting: {:?}\n  \
         Impact: every pause after typing raises a warning about a healthy pane, which teaches \
         somebody to ignore the one about a frozen pane.\n  Check the run log for a \
         `bridge.painted` line after the second keystroke. Without one, frames counted inside an \
         interval are waiting for another frame to carry them rather than being sent when the \
         interval ends.",
        problems()
    );
}
