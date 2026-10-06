/// Why a request was not done. Structured rather than worded, because the words name the
/// command to run next and the command's spelling belongs to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    BadName {
        name: String,
    },
    /// Someone alive already goes by this name; `inbox` is where, when it has one.
    NameInUse {
        name: String,
        inbox: Option<String>,
    },
    NoSuchGroup {
        group: String,
    },
    /// A group already goes by this name in another case. On a filesystem that ignores case,
    /// the two would share one log.
    GroupNameClash {
        group: String,
        existing: String,
    },
    /// A post to agents who share no group would create the group of exactly them, and its
    /// name, made from theirs, would be longer than a group name may be.
    PairTooLong {
        group: String,
    },
    NoSuchParticipant {
        name: String,
    },
    /// Members on several machines go by this name; `name@machine` says which.
    WhichParticipant {
        name: String,
        candidates: Vec<String>,
    },
    /// The group is kept on a machine this one has no link to now.
    Unreachable {
        group: String,
        machine: String,
    },
    /// No group here has the name, and machines this one has linked to, which may keep one,
    /// cannot be asked now: a new group here would take the name the other means.
    Unchecked {
        group: String,
        machines: Vec<String>,
    },
    /// A change to a group kept on `machine` was sent there and never answered: it may have been
    /// made there all the same, and only the answer lost.
    Unanswered {
        group: String,
        machine: String,
    },
    /// The group is kept on another machine, whose daemon changes its policy and members: this
    /// one holds a replica (MIP-4, section 11).
    KeptElsewhere {
        group: String,
        machine: String,
    },
    /// The caller is the person at the window, whose messages are kept on `machine`, where the
    /// app runs: this machine's daemon was dialed from there, so it has no human of its own
    /// (MIP-4, section 10). `calls_us` is what that machine calls this one.
    HumanElsewhere {
        machine: String,
        calls_us: String,
    },
    /// The caller is not a participant at all, so there is nothing to leave. `name` is what it
    /// called itself, when it did.
    NotAParticipant {
        name: Option<String>,
    },
    NotAMember {
        name: String,
        group: String,
        /// Who may add `name`, when the group's policy does not let it join on its own: a join
        /// would be refused too.
        permitted: Option<Vec<String>>,
    },
    AddresseeNotInGroup {
        name: String,
        group: String,
    },
    AddressedSelf,
    /// An unaddressed post from a caller in no group.
    NoGroup,
    /// Several groups would fit; the caller has to say which.
    WhichGroup {
        candidates: Vec<String>,
    },
    /// The stale-context guard: the author has not read what others posted.
    Unread {
        group: String,
        count: u64,
    },
    EmptyBody,
    BodyTooLarge {
        bytes: usize,
    },
    /// The group's policy does not let the author address this participant (MIP-4, section 8).
    NotAllowed {
        addressee: String,
        group: String,
        /// Whom the author may address there.
        allowed: Vec<String>,
    },
    /// The group's policy does not let the author post urgently there (MIP-4, section 8).
    NotUrgent {
        group: String,
        /// Who may.
        urgent: Vec<String>,
    },
    /// The group's policy does not let `name` do this: join or leave it, add or remove a
    /// member, or change its policy.
    NotPermitted {
        name: String,
        group: String,
        action: Action,
        /// Who may.
        permitted: Vec<String>,
    },
    GroupExists {
        group: String,
    },
    /// The store could not keep what was asked, so it was not done.
    Store {
        error: String,
    },
}

impl Refusal {
    /// A stable name for the refusal, for anything reading answers as data.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::BadName { .. } => "bad_name",
            Refusal::NameInUse { .. } => "name_in_use",
            Refusal::NoSuchGroup { .. } => "no_such_group",
            Refusal::GroupNameClash { .. } => "group_name_clash",
            Refusal::PairTooLong { .. } => "pair_too_long",
            Refusal::NoSuchParticipant { .. } => "no_such_participant",
            Refusal::WhichParticipant { .. } => "which_participant",
            Refusal::Unreachable { .. } => "unreachable",
            Refusal::Unchecked { .. } => "unchecked",
            Refusal::Unanswered { .. } => "unanswered",
            Refusal::KeptElsewhere { .. } => "kept_elsewhere",
            Refusal::HumanElsewhere { .. } => "human_elsewhere",
            Refusal::NotAParticipant { .. } => "not_a_participant",
            Refusal::NotAMember { .. } => "not_a_member",
            Refusal::AddresseeNotInGroup { .. } => "addressee_not_in_group",
            Refusal::AddressedSelf => "addressed_self",
            Refusal::NoGroup => "no_group",
            Refusal::WhichGroup { .. } => "which_group",
            Refusal::Unread { .. } => "unread",
            Refusal::EmptyBody => "empty_body",
            Refusal::BodyTooLarge { .. } => "body_too_large",
            Refusal::NotAllowed { .. } => "not_allowed",
            Refusal::NotUrgent { .. } => "not_urgent",
            Refusal::NotPermitted { .. } => "not_permitted",
            Refusal::GroupExists { .. } => "group_exists",
            Refusal::Store { .. } => "store",
        }
    }
}

/// What a group's `membership` governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Join,
    Leave,
    Add,
    Remove,
    SetPolicy,
    Pause,
    Resume,
    Delete,
}
