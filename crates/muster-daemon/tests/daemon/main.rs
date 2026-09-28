//! muster-daemon's integration tests, as one binary rather than one per file: see docs/testing.md.

mod arranging;
mod claude_code;
mod claude_code_inbox;
mod claude_code_live;
mod detection;
mod devenv_terminfo;
mod environment;
mod facts;
mod flood;
mod handoff;
mod handshake;
mod lifecycle;
mod log;
mod migration;
mod panes;
mod persistence;
mod relay;
mod ssh_terminfo;
mod streams;
mod subscribing;
mod support;
mod terminal;
mod typing;
