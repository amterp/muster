//! muster-ssh against a stand-in `ssh` on `PATH`, for what only timing can show.
//!
//! A real master needs a real sshd, which is the `--ssh` tier's container. What is judged here
//! is what the supervisor does with the answers it gets - a check that is slow, a check that
//! fails - and those are easier to stage exactly with a script than with a real master that
//! has to be made slow on cue. The script keeps a log of what it was asked, and that log is the
//! oracle.

mod fake;
mod left_behind;
mod slow_check;
