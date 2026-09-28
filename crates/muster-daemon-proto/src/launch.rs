//! How long a daemon just started is given to answer, the one number for both places that start
//! one: the app launching its own, and a daemon launching the successor it hands its panes to.

use std::time::Duration;

/// How long a newly started `muster-daemon` may take before it is taken to have failed.
///
/// It answers in milliseconds once it runs. The wait is for macOS, which holds a binary it has
/// not run before while it scans it: 12.6 s for a successor on a busy machine, 16 s for a debug
/// daemon, and 44 s for the first launch of a freshly built app. That first launch is exactly the
/// one an update makes, whether the app starts the new daemon or the running one hands over to
/// it, so both wait the same and neither gives up on a daemon the other would have waited for.
pub const LAUNCH_PATIENCE: Duration = Duration::from_mins(1);
