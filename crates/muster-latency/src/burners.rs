//! Load of the run's own making: a build's shape, every core busy at nice 10.

use std::process::{Child, Command, Stdio};

/// Two busy shells per logical core, at nice 10 as `./dev` builds run, ended when dropped.
/// Two, so that no core idles while a burner waits its turn: a daemon thread demoted below
/// the build's priority then has nowhere to run, which is what this load is there to show.
pub(crate) struct Burners(Vec<Child>);

impl Burners {
    pub(crate) fn start() -> Burners {
        let cores = std::thread::available_parallelism().map_or(8, std::num::NonZero::get);
        let burners = (0..2 * cores)
            .map(|_| {
                Command::new("nice")
                    .args(["-n", "10", "/bin/sh", "-c", "while :; do :; done"])
                    .stdin(Stdio::null())
                    .spawn()
                    .expect("a burner")
            })
            .collect();
        Burners(burners)
    }

    pub(crate) fn count(&self) -> usize {
        self.0.len()
    }
}

impl Drop for Burners {
    fn drop(&mut self) {
        for burner in &mut self.0 {
            let _ = burner.kill();
            let _ = burner.wait();
        }
    }
}
