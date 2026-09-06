//! A pane whose frames stopped arriving says so, and stops saying it when they come back.
//!
//! The gap kan a_2LMRCug0P is about, from the end nothing else can reach. The grid ceiling names
//! one cause of a frozen pane and this is the net under the rest: a wedged bridge, a transport
//! that dropped without closing, a daemon still answering while one of its terminals went quiet.
//! All of them leave the same picture - the last screen, still drawn, with the client connected,
//! the bridge a live process, the agent working and `muster window` reporting the pane `idle`.
//!
//! `corpus/conformance/pane-painting.json` drives the rule and `bridge_report`'s unit tests drive
//! the line's spelling, so both ends were coverable and the wire between them was not. It is also
//! the part most likely to break silently: a bridge whose paint report nothing reads behaves
//! exactly like a window that never watched for this at all - and *that* failure is a false
//! silence, which is the one nobody notices.
//!
//! So this freezes a real bridge with a real daemon behind it and asks the window what it thinks
//! is wrong. `SIGSTOP` rather than a kill, because a bridge that died is a different condition
//! with its own sentence: what is being proved here is the one where every layer below reports
//! health.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use herdr_harness::until;
use support::{Press, Typing, named_pane, problems};

/// Short enough that the gate does not wait out the shipped ten seconds, and the number is not
/// the subject - `pane-painting.json` argues about the real one. What is being proved is that a
/// paint crosses the wire at all and reaches the rule.
const DEADLINE: &str = "500";

#[test]
fn a_pane_whose_frames_stopped_arriving_is_reported() {
    // SAFETY: nothing else in this process reads the environment concurrently. This runs before
    // the core is started, which is when it reads this.
    unsafe { std::env::set_var("MUSTER_PAINTING_DEADLINE_MS", DEADLINE) };

    let typing = Typing::start("");
    let key = format!("pane:local/{}:painting", named_pane(&typing.pane));

    // Nothing has been asked of this pane, so its quiet is not worth a word - which is the
    // condition rather than a detail of it: an idle agent paints nothing all afternoon, and a
    // watch on silence alone would accuse most of a window most of the time.
    assert!(
        !problems().contains(&key),
        "a pane nobody has typed into should not be accused of anything, and the window says {:?}",
        problems()
    );

    // Alive, connected, and painting nothing. Everything below the app still reports health.
    typing.bridge.freeze();
    Press::new("KeyA", "a").send();

    until(
        "the window to say the pane has stopped painting",
        || problems().contains(&key),
        || {
            format!(
                "  Impact: a pane that answers nothing looks exactly like a pane whose agent has \
                 nothing to say, so the window would go on reporting this one `idle` while \
                 somebody typed into a screen that had stopped moving.\n  What the window says \
                 is wrong: {:?}\n  Look for `bridge.painted` in the run log, which is the bridge \
                 saying it repainted, and `pane:...:painting` among the problems, which is the \
                 window acting on the absence of one. Paints arriving with no problem raised \
                 means the keystroke never counted as something asked of the pane; neither \
                 means the report is not crossing the bridge's control socket.",
                problems()
            )
        },
    );

    // And it is taken back the moment frames arrive again, because a problem is a condition
    // rather than a message: the buffered keystroke reaches the daemon, the echo comes back, and
    // watching the row go is the confirmation that the pane recovered.
    typing.bridge.thaw();
    until(
        "the window to take it back once the pane paints again",
        || !problems().contains(&key),
        || format!("the window still says: {:?}", problems()),
    );
}
