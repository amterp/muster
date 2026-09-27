//! The fake agent is detected, and paints the state it is told.

use muster_daemon_proto as proto;
use muster_harness::Daemon;
use muster_harness::requests::{create, in_new_tab, make};

#[test]
fn a_pane_running_the_fake_agent_takes_the_state_it_is_told() {
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Working);
    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    daemon.set_agent_state("p1", proto::AgentState::Idle);
}
