//! Whether the two ends of a pane's environment still spell it the same way.
//!
//! The app sets `MUSTER_PANE` and `MUSTER_SOCKET` from `muster-herdr`, and this crate reads them
//! from its own constants. That duplication is deliberate - the herdr adapter must not depend on
//! the wire schema, so there is nowhere both could import from that neither of them should reach -
//! and this is what makes it safe rather than merely tolerated.
//!
//! What a drift costs, if this ever fails: every pane already running keeps the old name, so an
//! agent asks which pane it is, gets nothing, and silently acts on whichever pane the window's
//! keyboard happens to be on. Nothing errors.

#[test]
fn a_pane_reads_the_variables_the_app_writes() {
    assert_eq!(
        muster_cli::environment::PANE_NAME,
        muster_herdr::env::PANE_NAME,
        "the CLI and the app disagree about which variable tells a pane its own name, so `muster \
         pane send` run inside a pane would address the focused pane instead of itself"
    );
    assert_eq!(
        muster_cli::environment::WINDOW_SOCKET,
        muster_herdr::env::WINDOW_SOCKET,
        "the CLI and the app disagree about which variable names the window, so a pane would look \
         for a Muster to talk to, find none, and refuse every command with `no Muster window is \
         listening`"
    );
}
