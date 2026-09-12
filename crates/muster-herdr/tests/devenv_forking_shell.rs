//! An attach against a machine whose `/bin/sh` forks, end to end.
//!
//! kan a_2I5yedXos. `remote_start_script.rs` proves the start script lets go of ssh's pipes under
//! bash, at the level of the script. What it cannot prove is the attach, and neither can
//! `devenv.rs`: Muster sends its scripts through `sh`, the container's `sh` is dash, and dash
//! execs the last command of a backgrounded list - so the pipes close whatever the script says,
//! and that test passes against the broken script as readily as the fixed one. So this one points
//! the container's `/bin/sh` at bash, the way a RHEL-family box has it, and attaches.
//!
//! Its own file rather than a second test in `devenv.rs`, because both install and start a
//! daemon in one container's `~/.muster`, and the tests in one binary run in parallel. cargo runs
//! the binaries one after another.
//!
//! Out of the default gate like the other devenv tests: `./dev --ssh` runs it.

use std::sync::mpsc;
use std::time::Duration;

use muster_core::config::Panes;
use muster_herdr::daemon::Reached;
use muster_herdr::{configuration_text, own_socket_path, remote};
use muster_ssh::{Forward, Remote, Tunnel, remote_environment};

mod support;

use support::{cache, devenv};

/// How long the attach gets before it counts as held open.
///
/// Longer than the thirty seconds `ensure_running` itself gives a started daemon to answer, so a
/// daemon that is merely slow fails with that function's own error, which names the cause, rather
/// than here. The broken script never returns at all.
const ALLOWANCE: Duration = Duration::from_mins(1);

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn attaching_a_machine_whose_sh_forks_returns_once_the_daemon_answers() {
    let (host, options) = devenv();
    let environment = remote_environment(&host, &options).expect("the devenv should answer ssh");
    let remote_socket = own_socket_path(&environment)
        .expect("the container's environment should say where a herdr socket would go");

    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(
        Forward {
            host: host.clone(),
            options,
            control_path: temporary
                .join("muster-devenv-forking.ctl")
                .to_string_lossy()
                .into_owned(),
            local_socket: temporary
                .join("muster-devenv-forking.sock")
                .to_string_lossy()
                .into_owned(),
            remote_socket,
        },
        std::sync::Arc::new(|_| {}),
    )
    .expect("the tunnel should open against the devenv");
    let far = tunnel.remote();

    // Back to nothing, so what is timed below is an install and a start rather than an adopt.
    // `pkill -x` rather than `-f`, for the reason `devenv.rs` gives.
    clear(&far).expect("the far end should let its own home be cleared");

    // Declared after the tunnel so it drops first, while the master it talks through is open.
    let _bash = BashAsSh::swap_in(&far);
    assert!(
        !far.shell("echo ${BASH_VERSION:-}").expect("the far end should answer").trim().is_empty(),
        "`sh` over there is still not bash after the swap, so this attach would pass against the \
         broken start script too - the thing it exists not to do"
    );

    let (done, finished) = mpsc::channel();
    {
        let far = far.clone();
        let socket = tunnel.local_socket_path().to_string();
        let configuration = configuration_text(&Panes::default());
        std::thread::spawn(move || {
            let _ = done.send(remote::ensure_running(
                &far,
                &environment,
                &socket,
                Some(&cache()),
                &configuration,
            ));
        });
    }

    let Ok(started) = finished.recv_timeout(ALLOWANCE) else {
        // The far shell is waiting on the daemon, so ending the daemon is what lets the
        // stuck ssh session go.
        let _ = far.shell("pkill -x herdr >/dev/null 2>&1; true");
        panic!(
            "the attach had not returned after {} seconds against a machine whose sh forks. \
             That is what a start script looks like when the shell running it keeps ssh's \
             stdout and stderr open for as long as the daemon runs: `Remote::shell` waits for \
             end of file on both, so an attach against a RHEL-family box blocks in `start`, \
             and a Quit queued behind it fires the moment it clears (kan a_2HpkpQlZP). Check \
             what `start_script` in crates/muster-herdr/src/remote.rs backgrounds.",
            ALLOWANCE.as_secs()
        );
    };
    assert_eq!(
        started.expect("Muster should put a daemon on a machine whose sh forks"),
        Reached::Started,
        "there was nothing to adopt"
    );
}

/// `/bin/sh` pointed at bash for as long as this is held, and back at dash afterwards.
///
/// Put back on drop so that a failing test leaves the container as the other tiers expect it,
/// daemon cleared too - the corpus probe that runs after this starts its own.
struct BashAsSh {
    far: Remote,
}

impl BashAsSh {
    fn swap_in(far: &Remote) -> BashAsSh {
        // Checked first, because drop puts dash back whatever was there before.
        assert_eq!(
            far.shell("readlink /bin/sh").expect("the far end should answer").trim(),
            "dash",
            "the devenv's /bin/sh is not dash, and this test would put dash back afterwards"
        );
        far.shell("sudo -n /usr/bin/ln -sf bash /bin/sh").expect(
            "dev should be allowed to point /bin/sh at bash - devenv/Dockerfile grants exactly \
             that in /etc/sudoers.d/devenv-sh. A container started from an image older than that \
             rule refuses; ./devenv/devenv down && ./devenv/devenv up recreates it.",
        );
        BashAsSh { far: far.clone() }
    }
}

impl Drop for BashAsSh {
    fn drop(&mut self) {
        // Ignored rather than unwrapped: this can run while a failed assertion is already
        // unwinding, and a second panic would abort the test binary before it reports.
        let _ = self.far.shell("sudo -n /usr/bin/ln -sf dash /bin/sh");
        let _ = clear(&self.far);
    }
}

fn clear(far: &Remote) -> Result<String, String> {
    far.shell("pkill -x herdr >/dev/null 2>&1; rm -rf \"$HOME/.muster\"; true")
}
