//! What becomes of a wake after the service hands it over: delivered outside the host's lock, so
//! the world can move between the post and the report of how the wake went.

use std::cell::RefCell;
use std::collections::BTreeSet;

use muster_msg::{Caller, Inbox, Liveness, Memory, Messaging, Participant, Presence};

#[derive(Default)]
struct Sessions {
    dead: RefCell<BTreeSet<String>>,
}

impl Presence for Sessions {
    fn alive(&self, participant: &Participant) -> bool {
        participant.inbox.as_ref().is_none_or(|inbox| !self.dead.borrow().contains(&inbox.socket))
    }
}

fn session(name: &str) -> Caller {
    Caller {
        inbox: Some(Inbox { socket: format!("/tmp/cc-socks/{name}.sock"), inode: 1 }),
        ..Caller::default()
    }
}

/// The wake went to an old session; before its failure was reported, a new session took over
/// the name. The failure is the old session's, and marks nobody gone.
#[test]
fn a_wake_that_failed_marks_gone_only_the_session_it_was_for() {
    let sessions = Sessions::default();
    let mut service = Messaging::new(Memory::default());
    service.join(&session("s1"), Some("builder"), Some("g"), &sessions, 1).unwrap();
    service.join(&session("s2"), Some("director"), Some("g"), &sessions, 2).unwrap();
    let posted = service.post(&session("s2"), None, &[], "one", &sessions, 3).unwrap();
    let wake = posted.wakes[0].clone();

    sessions.dead.borrow_mut().insert("/tmp/cc-socks/s1.sock".to_string());
    service.join(&session("s3"), Some("builder"), None, &sessions, 4).unwrap();
    service.delivered(&wake, false).unwrap();

    let who = service.who(Some("g"), &sessions).unwrap();
    let builder = who.iter().find(|member| member.name == "builder").unwrap();
    assert_eq!(builder.liveness, Liveness::Alive, "{who:?}");
}
