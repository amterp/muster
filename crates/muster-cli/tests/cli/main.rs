//! muster-cli's integration tests, as one binary rather than one per file: see docs/testing.md.

mod argv;
mod asking_for_a_window;
mod docs;
mod driving_a_window;
mod exit_codes;
mod harness;
#[cfg(target_os = "macos")]
mod install_main;
mod messaging;
mod two_windows;
mod upgrading;
mod waiting_on_agents;
mod without_a_window;
