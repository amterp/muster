//! Files more than one Muster writes, as far as the core is concerned.
//!
//! A window is a process, so two windows are two writers. Two records are shared that way: the
//! names Muster gives panes and tabs (`crate::names`), and which window holds each tab
//! (`crate::composition::holding`). Both are read, changed and written inside one hold, and
//! this trait is the whole of what the core asks for to do that. Where a record lives and how it
//! is locked are OS questions, answered in the seam.

/// A written record another Muster may be holding open too.
///
/// **The reason this exists is that a window is a process.** Two Musters attached to one daemon
/// both see every pane it holds, and both would name one nobody had named yet - so the same
/// pane ends up called two things, each window's `muster window` disagrees with the other's,
/// and the one that writes the file last takes the other's bindings with it. Measured, not
/// feared: two windows on one daemon agreed only about the pane that existed before the second
/// one opened.
///
/// So naming something new is done while holding the record, rather than in memory and written
/// out afterwards. `exclusively` is the whole of what the core asks for; where the record lives
/// and how it is locked is the shell's business, like every other file.
pub trait SharedRecord: Send + Sync + std::fmt::Debug {
    /// Runs `while_held` with nobody else able to read or write the record.
    ///
    /// `while_held` is given what the record says at that moment and answers with what to write
    /// back, or `None` to leave it alone. A record that cannot be reached does not stop
    /// anything: `while_held` still runs, given nothing, and this Muster names things the way
    /// it did before there was a record to share - which is right, because a window that
    /// refused to name a pane would be a window that cannot draw one.
    fn exclusively(&self, while_held: &mut dyn FnMut(&str) -> Option<String>);

    /// Whether the record has moved since this Muster last read it.
    ///
    /// What keeps the common answer cheap. Naming something this window has already named is
    /// the overwhelming majority of these calls - every pane of every layout the daemon
    /// describes - and taking a lock for each would be a lock per keystroke somebody typed
    /// into an agent. So a name already held is handed straight back, unless the record has
    /// changed underneath, in which case another Muster has written something and this window
    /// takes the hold to find out what.
    ///
    /// Needed because a window can be *wrong* rather than merely ignorant: a pane created in
    /// another window may be seen here, and named here, before that window has settled the
    /// name it already put in the pane's environment. Without this, the guess would stand
    /// forever, since nothing else would ever look at the record again.
    ///
    /// False by default, which is right for a record nothing else writes.
    fn moved(&self) -> bool {
        false
    }
}
