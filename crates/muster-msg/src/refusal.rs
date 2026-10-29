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
    /// A pane other than the caller's goes by this name, which a post's `--to` reads as that
    /// pane.
    NameOfPane {
        name: String,
        pane: String,
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
    /// Nothing is called this, and it cannot be a participant's name, but it is what these
    /// panes here show as their title: what a harness writes in the terminal's title bar, which
    /// is not an address because the harness rewrites it as its agent works.
    Titled {
        name: String,
        panes: Vec<String>,
    },
    /// Several agents go by this name; each candidate is a name that reaches one of them alone.
    WhichParticipant {
        name: String,
        candidates: Vec<String>,
    },
    /// Nothing goes by this name now, and these panes here did before they were renamed: each as
    /// what it is called now, its callsign and its id. Never followed to the pane, because a
    /// name a rename freed is free for another to take.
    Renamed {
        name: String,
        panes: Vec<String>,
    },
    /// Agents the candidates reach go by this name, and so does a participant here with no open
    /// pane, which nothing but the name reaches while a pane goes by it too.
    Shadowed {
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
    /// A question offers more answers than [`crate::MOST_ANSWERS`].
    TooManyAnswers {
        count: usize,
    },
    /// An offered answer is longer than [`crate::LONGEST_ANSWER`] bytes.
    AnswerTooLong {
        answer: String,
    },
    /// An offered answer is empty, holds a line break, or repeats another: each is one button,
    /// and a reply names the one it picks by its words.
    BadAnswer {
        answer: String,
    },
    /// A reply that picks a question's answer offers answers of its own: it is an answer, and a
    /// question is a post of its own.
    AnswerOffersAnswers,
    /// A reply names a question that is not a message in its group offering answers.
    NoSuchQuestion {
        group: String,
        question: u64,
    },
    /// A reply picks an answer its question did not offer; `answers` are those it did.
    NotOffered {
        group: String,
        question: u64,
        answer: String,
        answers: Vec<String>,
    },
    /// A reply picks an answer to a question not asked of its author: its own, or one addressed
    /// to others.
    NotAskedOf {
        name: String,
        group: String,
        question: u64,
    },
    /// The question has its answer already: `answer`, picked by `by`.
    AlreadyAnswered {
        group: String,
        question: u64,
        by: String,
        answer: String,
    },
    /// A program named a post by an operation id it used for another request in the retention
    /// period: a retry must ask exactly what it asked the first time.
    OperationReused {
        id: String,
    },
    /// An acknowledgement named a number past the newest in its group, which nobody can have
    /// been shown.
    PastNewest {
        group: String,
        through: u64,
        newest: u64,
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
            Refusal::NameOfPane { .. } => "name_of_pane",
            Refusal::NoSuchGroup { .. } => "no_such_group",
            Refusal::GroupNameClash { .. } => "group_name_clash",
            Refusal::PairTooLong { .. } => "pair_too_long",
            Refusal::NoSuchParticipant { .. } => "no_such_participant",
            Refusal::Titled { .. } => "titled",
            Refusal::WhichParticipant { .. } => "which_participant",
            Refusal::Shadowed { .. } => "shadowed",
            Refusal::Renamed { .. } => "renamed",
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
            Refusal::TooManyAnswers { .. } => "too_many_answers",
            Refusal::AnswerTooLong { .. } => "answer_too_long",
            Refusal::BadAnswer { .. } => "bad_answer",
            Refusal::AnswerOffersAnswers => "answer_offers_answers",
            Refusal::NoSuchQuestion { .. } => "no_such_question",
            Refusal::NotOffered { .. } => "not_offered",
            Refusal::NotAskedOf { .. } => "not_asked_of",
            Refusal::AlreadyAnswered { .. } => "already_answered",
            Refusal::OperationReused { .. } => "operation_reused",
            Refusal::PastNewest { .. } => "past_newest",
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
    /// Make the group under a policy whose `membership` leaves its maker out.
    Make,
    /// Set a policy whose `membership` names neither whoever sets it nor any member.
    Strand,
}
