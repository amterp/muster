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
    NoSuchParticipant {
        name: String,
    },
    /// The caller is not a participant at all, so there is nothing to leave.
    NotAParticipant {
        name: String,
    },
    NotAMember {
        name: String,
        group: String,
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
            Refusal::NoSuchParticipant { .. } => "no_such_participant",
            Refusal::NotAParticipant { .. } => "not_a_participant",
            Refusal::NotAMember { .. } => "not_a_member",
            Refusal::AddresseeNotInGroup { .. } => "addressee_not_in_group",
            Refusal::AddressedSelf => "addressed_self",
            Refusal::NoGroup => "no_group",
            Refusal::WhichGroup { .. } => "which_group",
            Refusal::Unread { .. } => "unread",
            Refusal::EmptyBody => "empty_body",
            Refusal::BodyTooLarge { .. } => "body_too_large",
            Refusal::Store { .. } => "store",
        }
    }
}
