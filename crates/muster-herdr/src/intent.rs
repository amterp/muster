//! Muster's requested changes, in herdr's words.
//!
//! The write half of the adapter, and the mirror image of `events.rs`: everything that knows
//! what a `pane.split` envelope looks like is here, so a second backend is a second file
//! rather than a search for every place a JSON key leaked.
//!
//! Three translations are worth naming. A split's side becomes the direction herdr names it
//! for - a new pane goes on the `second` side of a `right` or a `down` - which is the same
//! translation `layout.rs` does in reverse when it reads a tree back. A path down the tree
//! becomes an array of booleans, herdr's own spelling for the turns. And two of Muster's four
//! sides have no request behind them at all, so this is also where one intent becomes two.

use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::find::{Found, Needle, found_in};
use muster_core::intent::{
    BackendChannel, BackendIntent, Branch, Outcome, Refusal, SettledLayout, Side,
};
use muster_core::mirror::backend::{PaneId, TabId, Viewport};
use muster_core::names::{BackendPaneId, Names};
use serde_json::{Value, json};

use crate::client::{Failure, HerdrClient};
use crate::env::PaneEnvironment;
use crate::layout::{read_exported_layout, read_layout};

/// One daemon, as the thing Muster sends changes to.
///
/// A client, the environment every pane it makes is handed, and this daemon's half of the name
/// registry. They are together because a pane-creating request needs all three and nothing else
/// does: the snapshot, the subscription and the pane channels all hold a bare [`HerdrClient`],
/// for which a pane environment would be a field that never means anything.
#[derive(Debug)]
pub struct HerdrBackend {
    client: HerdrClient,
    panes: PaneEnvironment,
    names: Names,
}

impl HerdrBackend {
    pub fn new(client: HerdrClient, panes: PaneEnvironment, names: Names) -> HerdrBackend {
        HerdrBackend { client, panes, names }
    }
}

impl BackendChannel for HerdrBackend {
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, Refusal> {
        // Minted before the request rather than read off the answer, because it has to travel
        // *in* the request: the pane's own environment is sent with the split and herdr names
        // the pane in its reply, so this is the only order in which a pane can be told what it
        // is called (see `muster_core::names`).
        let minted = makes_a_pane(intent).then(|| self.names.reserve());
        let panes = match &minted {
            Some(name) => self.panes.with_pane_name(name),
            None => self.panes.clone(),
        };

        let (method, params) = request(intent, &panes, &self.names)?;
        let result = match self.client.request(method, &params) {
            Ok(result) => result,
            Err(failure) => {
                // Nothing was made, so nothing answers to the name. Released rather than left
                // reserved, so a daemon that refuses splits all afternoon does not fill the
                // registry with names for panes that never existed.
                if let Some(name) = &minted {
                    self.names.release(name);
                }
                return Err(refusal(&failure));
            }
        };

        let created = self.settle(created(&result).as_deref(), minted.as_ref());
        let settled = match (rearranges(intent), &created) {
            (Some(pane), Some(created)) => self.rearrange(pane, created),
            // A split herdr made and would not name. Worth saying, because the consequence is
            // no longer only a keyboard that stays put: the pane cannot be rearranged either,
            // so a leftward split quietly becomes a rightward one.
            (Some(pane), None) => {
                log::warn(
                    "herdr.split.unnamed",
                    fields! {
                        "pane" => pane.to_string(),
                        "impact" => "the split happened and the new pane is on the opposite \
                                     side from the one that was asked for, because nothing \
                                     can name it to rearrange it.",
                        "next" => "whether pane.split still answers with the pane it made, \
                                   under `pane.pane_id` - `created` in this file reads it.",
                    },
                );
                None
            }
            _ => {
                // A mutation the daemon considered and did not perform. It is a success on the
                // wire, so nothing above notices, and the only symptom is a window that does
                // not change - which is exactly what a bug in the request looks like too.
                if let Some(reason) = declined(&result) {
                    log::warn(
                        "herdr.intent.declined",
                        fields! {
                            "method" => method,
                            "reason" => reason,
                            "impact" => "the window is unchanged and correct - the daemon did \
                                         not do this, and nothing is being rendered that did \
                                         not happen. What was asked for did not take effect.",
                            "next" => "the reason is herdr's own word for why. For a swap those \
                                       are no_neighbor, same_pane, not_found and cross_tab; a \
                                       cross_tab here means the caller should have sent a move.",
                        },
                    );
                }
                // Applied even when declined, because the layout an answer carries is the
                // daemon's current arrangement either way, and a mirror already holding it
                // settles to a no-op.
                settled(&result, None, &self.names)
            }
        };
        // Naming it and starting something in it, once it exists and is where it was asked to
        // be. After the rearrange, because a leftward split moves the pane and there is no
        // point typing into something that is about to be swapped out from under the user's
        // eyes; before the answer goes back, because a caller told the pane's name will use it.
        let equipped = match &created {
            Some(created) => self.equip(intent, created),
            None => None,
        };

        // Return, for text that asked for one. Its own request because herdr encodes a named key
        // against the pane's live modes, and a newline in the text above is only a newline - which
        // a program reading a bracketed paste buffers as more text instead of acting on.
        if let BackendIntent::SendText { pane, enter: true, .. } = intent {
            self.press_enter(pane);
        }

        Ok(Outcome {
            created,
            created_tab: created_tab(&result),
            settled,
            // Either the rename this intent *was*, or the one a split asked for on the way. Both
            // are the only route there is - herdr announces a rename to nobody
            // (`observations/herdr-0.8.0.md` section 16) - so a name dropped here is a name the
            // window never shows.
            renamed: renamed(intent, &result).or(equipped),
        })
    }

    /// Read the pane back, then match it here.
    ///
    /// herdr has no search of its own - `pane.read` and the `pane.output_matched` event are
    /// the whole surface area (`observations/herdr-0.8.0.md` section 17, kan a_29Ayr1P8F) -
    /// so this is the read-and-match half of the seam. The day a daemon-side search lands,
    /// this body becomes one request and `find::found_in` stops being called from here.
    ///
    /// A read that fails is a refusal rather than an empty answer. "Nothing found" and
    /// "nobody looked" are different things to put under a search box, and a pane that has
    /// gone away is `NotThere`, which is how the window learns it is showing something the
    /// daemon no longer holds.
    fn find(&self, pane: &PaneId, needle: &Needle) -> Result<Found, Refusal> {
        let (method, params) = read_request(&self.names.backend(pane)?);
        let result = self.client.request(method, &params).map_err(|failure| refusal(&failure))?;
        let text = nested(&result, "text").and_then(Value::as_str).unwrap_or_default();
        // Absent means "there is no more", which is the safe way round: claiming a search
        // was partial when it was whole only costs a caveat nobody needed, while the
        // reverse is the confident wrong answer this feature exists to avoid.
        let truncated = nested(&result, "truncated").and_then(Value::as_bool).unwrap_or(false);
        Ok(found_in(text, needle, truncated))
    }

    fn viewport(&self, pane: &PaneId) -> Result<Viewport, Refusal> {
        let params = json!({ "pane_id": self.names.backend(pane)?.as_str() });
        let result =
            self.client.request("pane.get", &params).map_err(|failure| refusal(&failure))?;
        let scroll = nested(&result, "scroll");
        let rows = |field: &str| {
            scroll.and_then(|scroll| scroll.get(field)).and_then(Value::as_u64).unwrap_or_default()
        };
        // Saturating rather than wrapping. These are row counts of a terminal's history, so a
        // value that did not fit is a daemon reporting something impossible - and a wrapped
        // one would scroll a pane to the far end of itself.
        let count = |field: &str| u32::try_from(rows(field)).unwrap_or(u32::MAX);
        Ok(Viewport {
            rows_from_bottom: count("offset_from_bottom"),
            rows: count("viewport_rows"),
            deepest: count("max_offset_from_bottom"),
        })
    }

    fn description(&self) -> &str {
        self.client.socket_path()
    }
}

/// How far back a find reads, in rows.
///
/// herdr's own ceiling: `lines` is clamped to a thousand rather than refused, so asking for
/// more returns the same rows while making the request claim an expectation it does not
/// have (`observations/herdr-0.8.0.md` section 17). Raising this is therefore a decision
/// about what reading more rows per keystroke costs, to be made when there is more to read.
const ROWS_READ: u32 = 1000;

/// How long a new pane's shell gets to print something before Muster types into it anyway.
///
/// Long enough to cover a shell that sources a slow profile, which is where this actually bites -
/// a login shell reading a config that runs `nvm` or a version manager takes well over a second
/// on a cold cache. Short enough that a pane whose prompt draws nothing visible does not leave a
/// caller waiting: that case cannot be waited for at all and always spends the whole allowance.
const READY_WITHIN: Duration = Duration::from_secs(5);

/// The read a find runs on, as the method and parameters herdr wants for it.
///
/// Public for the reason `request` is: herdr ignores a key it does not recognise, so a
/// misspelling is a request that quietly answers about the wrong thing rather than a
/// refusal, and the spelling is pinned by name in the corpus.
///
/// `recent` rather than `recent_unwrapped`, and the difference is a long line. `recent`
/// returns grid rows, so a match's place in the answer is already the offset to scroll to;
/// `recent_unwrapped` returns whole logical lines, which would match a needle spanning a
/// wrap and leave nothing able to say where it is. A hit nobody can scroll to is worse than
/// a hit nobody found.
pub fn read_request(pane: &BackendPaneId) -> (&'static str, Value) {
    (
        "pane.read",
        json!({
            "pane_id": pane.as_str(),
            "source": "recent",
            "lines": ROWS_READ,
            // herdr's own default, sent anyway: what this asks for is text to match, and a
            // default that changed underneath would silently start matching escape codes.
            "strip_ansi": true,
        }),
    )
}

impl HerdrBackend {
    /// Binds the name Muster minted to the pane herdr says it made.
    ///
    /// A pane made under no reserved name is named on sight rather than dropped, because a pane
    /// the mirror cannot name is a split missing from the window. It also means [`makes_a_pane`]
    /// and [`request`] disagree about which intents make one, which is a bug here.
    fn settle(&self, backend: Option<&str>, minted: Option<&PaneId>) -> Option<PaneId> {
        match (backend, minted) {
            (Some(backend), Some(name)) => {
                self.names.settle(name, backend);
                Some(name.clone())
            }
            (Some(backend), None) => Some(self.names.name(backend)),
            (None, Some(name)) => {
                self.names.release(name);
                None
            }
            (None, None) => None,
        }
    }

    /// Gives a pane the name and the program the request asked it to be made with.
    ///
    /// Both are extra requests because herdr's `pane.split` takes neither a label nor a command
    /// (`observations/herdr-0.8.0.md` section 18). A backend that could do either with the split
    /// would leave this empty, which is why the compound lives here and not above: what a caller
    /// asked for is "a pane called this, running that", and how many requests that costs is one
    /// daemon's business.
    ///
    /// Returns the rename to report, because herdr announces one to nobody.
    ///
    /// **Nothing here can fail the split.** The pane exists - the daemon said so - and a pane
    /// that came up nameless, or with its command untyped, is worth strictly more than no pane
    /// at all. Each part says so in the log instead, because each is silent in the window: an
    /// unnamed pane looks like one nobody named, and an untyped command looks like a shell
    /// sitting at a prompt.
    fn equip(&self, intent: &BackendIntent, pane: &PaneId) -> Option<(PaneId, Option<String>)> {
        let BackendIntent::SplitPane { run, name, .. } = intent else { return None };

        let renamed = name.as_ref().and_then(|label| self.label(pane, label));
        if let Some(command) = run {
            self.start(pane, command);
        }
        renamed
    }

    /// Calls a pane what the request asked it to be called.
    fn label(&self, pane: &PaneId, label: &str) -> Option<(PaneId, Option<String>)> {
        let backend = self.names.backend(pane).ok()?;
        let params = json!({ "pane_id": backend.as_str(), "label": label });
        match self.client.request("pane.rename", &params) {
            Ok(_) => Some((pane.clone(), Some(label.to_string()))),
            Err(failure) => {
                log::warn(
                    "herdr.split.unnamed_pane",
                    fields! {
                        "pane" => pane.to_string(),
                        "detail" => failure.to_string(),
                        "impact" => "the pane was made and is running, and is listed under no \
                                     name - so it looks like a pane nobody bothered to name \
                                     rather than one whose name did not take",
                        "check" => "whether the daemon still holds this pane; a rename is the \
                                    one change herdr announces to nobody, so the reply above \
                                    is all there was",
                    },
                );
                None
            }
        }
    }

    /// Types a command into a pane, once its shell has printed something.
    ///
    /// The wait is why this belongs in Muster rather than in every caller: whatever it is worth,
    /// it is worth doing once and in one place.
    ///
    /// **What it protects against, honestly.** A pty buffers, so a plain shell handed input
    /// before it has drawn a prompt still runs it - that case needs no wait and does not get one,
    /// because the screen already has content and the wait returns at once. The case it is for is
    /// a program that resets the terminal as it starts, which is what a full-screen agent harness
    /// does: pending input is discarded by the reset, and what is left is a pane sitting in a
    /// harness that was never told anything. That is not reproducible with `sh`, so no test here
    /// asserts the wait changes an outcome - `client_connection.rs` asserts the mechanism works,
    /// and this is a precaution rather than a demonstrated fix.
    fn start(&self, pane: &PaneId, command: &str) {
        let Ok(backend) = self.names.backend(pane) else { return };
        self.ready(&backend);

        let text = json!({ "pane_id": backend.as_str(), "text": command });
        if let Err(failure) = self.client.request("pane.send_text", &text) {
            log::warn(
                "herdr.split.command_unsent",
                fields! {
                    "pane" => pane.to_string(),
                    "detail" => failure.to_string(),
                    "impact" => "the pane was made and is sitting at its own prompt, which looks \
                                 exactly like a command that ran and printed nothing",
                    "check" => "whether the daemon still holds this pane",
                },
            );
            return;
        }
        self.press_enter(pane);
    }

    /// Presses Return in a pane, which is what submits whatever was just typed into it.
    ///
    /// A named key rather than a newline in the text before it, so herdr encodes it against the
    /// pane's live modes - which is what a program reading a bracketed paste needs in order to
    /// treat this as a submission rather than as more text to buffer.
    fn press_enter(&self, pane: &PaneId) {
        let Ok(backend) = self.names.backend(pane) else { return };
        let enter = json!({ "pane_id": backend.as_str(), "keys": ["enter"] });
        if let Err(failure) = self.client.request("pane.send_input", &enter) {
            log::warn(
                "herdr.pane.unsubmitted",
                fields! {
                    "pane" => pane.to_string(),
                    "detail" => failure.to_string(),
                    "impact" => "the text reached the pane and was never submitted, so it sits at \
                                 the prompt unexecuted - which reads as a program that ignored it",
                    "check" => "whether the daemon still holds this pane",
                },
            );
        }
    }

    /// Waits until a pane's shell has printed something, or until it is not worth waiting more.
    ///
    /// "Anything at all on the visible screen" rather than a prompt Muster would have to
    /// recognize: every shell's prompt is different and configurable, and a pattern per shell
    /// would be a list to keep in step with other people's dotfiles.
    ///
    /// **Timing out is not a refusal.** A prompt that draws nothing visible is a real
    /// configuration, and refusing to type into that pane would be worse than typing early -
    /// the command would never run at all, rather than possibly running. So this says so in the
    /// log and returns, and the caller sends anyway.
    fn ready(&self, pane: &BackendPaneId) {
        let waiting = json!({
            "pane_id": pane.as_str(),
            // Any non-space. herdr's match is a substring or a regex and has no "anything at
            // all", so this is how that is spelled.
            "match": { "type": "regex", "value": r"\S" },
            "source": "visible",
            "timeout_ms": u64::try_from(READY_WITHIN.as_millis()).unwrap_or(u64::MAX),
        });
        // Given longer than herdr is, so that the daemon's own deadline is the one that decides.
        // The other way round, the socket would give up first and a wait that was about to
        // succeed would read as a daemon that had gone.
        let allowance = READY_WITHIN + HerdrClient::DEFAULT_TIMEOUT;
        if let Err(failure) =
            self.client.request_within("pane.wait_for_output", &waiting, allowance)
        {
            log::warn(
                "herdr.pane.never_ready",
                fields! {
                    "pane" => pane.to_string(),
                    "detail" => failure.to_string(),
                    "impact" => "the command was sent anyway, which is the better of two bad \
                                 answers - a shell that had not finished starting may have \
                                 swallowed it, so the pane may sit at a prompt having run \
                                 nothing",
                    "check" => "whether this pane's shell draws a visible prompt. One that \
                                prints nothing cannot be waited for and always lands here",
                },
            );
        }
    }

    /// Puts a pane herdr has just made on the other side of the one it was split from.
    ///
    /// herdr's `SplitDirection` is `right` and `down`, and a split always puts the new pane on
    /// the `second` side, so leftward and upward have no request behind them: they are a split
    /// followed by `pane.swap` of the pair. The whole compound lives here rather than above,
    /// because it is a fact about one daemon's vocabulary and a backend with four directions
    /// would need none of it.
    ///
    /// **A refusal here is not a failure of the intent.** The pane exists - herdr said so
    /// before this was asked - and the honest answer to a swap that will not happen is to keep
    /// it and say where it ended up, never to close a pane the user may already be typing in.
    /// Undoing is the one thing worse than a pane on the wrong side.
    fn rearrange(&self, pane: &PaneId, created: &PaneId) -> Option<SettledLayout> {
        let params = json!({
            "source_pane_id": self.names.backend(pane).ok()?.as_str(),
            "target_pane_id": self.names.backend(created).ok()?.as_str(),
        });
        let answer = match self.client.request("pane.swap", &params) {
            Ok(answer) => answer,
            Err(failure) => {
                // `invalid_pane_swap` rather than one of the four refusal reasons, for a swap
                // herdr will not even consider (`observations/herdr-0.8.0.md` section 14).
                log::warn(
                    "herdr.swap.refused",
                    fields! {
                        "pane" => pane.to_string(),
                        "created" => created.to_string(),
                        "detail" => failure.to_string(),
                        "impact" => "the new pane is on the opposite side from the one that \
                                     was asked for. Nothing is lying - the daemon and the \
                                     window agree - and the pane is usable.",
                        "next" => "herdr's code says why it would not swap. A four-way \
                                   SplitDirection upstream would remove the second request \
                                   entirely (kan a_28XGcvXEg).",
                    },
                );
                return None;
            }
        };
        // `changed` is a success with a reason beside it rather than an error, and it nests
        // one level down like every other herdr result - read off the top level it is `null`,
        // which is indistinguishable from a swap that did nothing.
        let swap = answer.get("swap")?;
        if swap.get("changed").and_then(Value::as_bool) != Some(true) {
            log::warn(
                "herdr.swap.declined",
                fields! {
                    "pane" => pane.to_string(),
                    "created" => created.to_string(),
                    "reason" => swap.get("reason").and_then(Value::as_str).unwrap_or("(none given)"),
                    "impact" => "the new pane is on the opposite side from the one that was \
                                 asked for, and is usable.",
                    "next" => "herdr names four reasons a swap does not happen: no_neighbor, \
                               same_pane, not_found, cross_tab.",
                },
            );
            return None;
        }
        settled(swap, Some((pane, created)), &self.names)
    }
}

/// Why the daemon did nothing, for a result that says it did nothing.
///
/// `changed: false` is a success with a reason beside it rather than an error, and it nests one
/// level down like every other herdr result - read off the top level it is null, which is
/// indistinguishable from a mutation that worked. A verb that never reports one answers `None`
/// here and is left alone.
fn declined(result: &Value) -> Option<&str> {
    let changed = nested(result, "changed")?.as_bool()?;
    (!changed).then(|| nested(result, "reason").and_then(Value::as_str).unwrap_or("(none given)"))
}

/// The pane a split has to be rearranged against, for an intent herdr cannot do in one request.
fn rearranges(intent: &BackendIntent) -> Option<&PaneId> {
    match intent {
        BackendIntent::SplitPane { pane, side, .. } if swaps(*side) => Some(pane),
        _ => None,
    }
}

/// herdr's refusal, in the two kinds Muster acts on differently.
///
/// The codes it names are the ones that mean "no such thing here", which herdr spells one way
/// per kind of thing (`pane_not_found`, `tab_not_found`, `workspace_not_found`,
/// `layout_not_found`, `split_not_found`). Read by code rather than by message: the messages
/// are prose that has changed between versions and the codes have not, and being wrong here
/// means a window that keeps showing a pane nobody can reach.
///
/// Everything else - unreachable, timed out, a daemon that answered something unparseable -
/// says nothing about whether the pane exists, so it stays a refusal and nothing more.
pub fn refusal(failure: &Failure) -> Refusal {
    let detail = failure.to_string();
    match failure {
        Failure::Daemon { code, .. } if code.ends_with("not_found") => Refusal::NotThere(detail),
        _ => Refusal::Declined(detail),
    }
}

/// One intent as the method and parameters herdr wants for it.
///
/// Public so the corpus can pin it. herdr ignores a parameter it does not recognise, so a
/// misspelled key is not a refusal - it is a request that acts on whatever the daemon had
/// focused, which against a one-pane daemon is indistinguishable from the right answer. That
/// is not something a test with a real daemon in it can catch, so the keys are checked here
/// by name (`corpus/conformance/backend-intent.json`).
///
/// The pane environment is a parameter rather than something read here, and it is why this
/// takes an argument at all: three of these arms make a pane, and a pane Muster makes has to
/// be handed the user's own herdr config path back and the name it should call itself (see
/// [`crate::env`]). Passing it in is what lets the corpus see it - the same reason this
/// function is public.
///
/// **Every pane named in a parameter is translated here**, from the name Muster minted to the
/// id this daemon knows it by. This is the wire, and above it nothing spells a pane herdr's
/// way. A name that resolves to nothing is a refusal rather than a request built out of it,
/// because herdr ignores an id it does not recognize and acts on whatever it has focused.
// One arm per intent, which is what makes this readable: herdr's whole write vocabulary in one
// place, in the order Muster's own is declared. Split into helpers it would be the same length
// with the correspondence broken up.
#[allow(clippy::too_many_lines)]
pub fn request(
    intent: &BackendIntent,
    panes: &PaneEnvironment,
    names: &Names,
) -> Result<(&'static str, Value), Refusal> {
    Ok(match intent {
        // `run` and `name` are deliberately absent from these params: herdr's `pane.split`
        // takes neither, so both are requests of their own once the pane exists
        // (`observations/herdr-0.8.0.md` section 18). [`HerdrBackend::equip`] sends them.
        BackendIntent::SplitPane { pane, side, ratio, cwd, run: _, name: _ } => {
            let mut params = json!({
                // `target_pane_id`, not `pane_id`, and the difference is silent: herdr
                // ignores a key it does not know and splits whichever pane it has focused,
                // so the wrong name reads as a split landing in an arbitrary place rather
                // than as a refusal. Every other pane verb takes `pane_id`.
                "target_pane_id": names.backend(pane)?.as_str(),
                "direction": direction(*side),
                // The daemon's cursor follows the new pane, because a person who split
                // something is looking at what they made. Muster's own keyboard is moved
                // separately, by whoever asked for this.
                "focus": true,
            });
            if let Some(ratio) = ratio {
                // herdr's `ratio` is the *first* child's share, and Muster's is the share the
                // pane being split keeps. On a leftward or upward split that pane ends up
                // second, so the two count from opposite ends and the number has to be turned
                // round. Invisible to a keybinding, which sends no ratio at all and takes the
                // daemon's symmetric default; visible to anything that places a divider.
                params["ratio"] = json!(if swaps(*side) { 1.0 - ratio } else { *ratio });
            }
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            if let Some(env) = panes.as_params() {
                params["env"] = env;
            }
            ("pane.split", params)
        }
        BackendIntent::CreateTab { workspace, cwd } => {
            // `workspace_id`, and nothing else names where this goes. herdr ignores a key it
            // does not know, so a `pane_id` sent hopefully would be dropped in silence and
            // the tab would land in whichever workspace that daemon had focused.
            let mut params = json!({ "workspace_id": workspace.as_str(), "focus": true });
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            if let Some(env) = panes.as_params() {
                params["env"] = env;
            }
            ("tab.create", params)
        }
        BackendIntent::CreateWorkspace { cwd } => {
            let mut params = json!({
                // The daemon's cursor follows it, for the same reason a split's does: this is
                // asked for by somebody who wants to be looking at what it makes.
                "focus": true,
                // herdr labels an unlabelled workspace itself, and a name Muster invented
                // would be a second naming scheme for the same thing.
                "label": Value::Null,
            });
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            if let Some(env) = panes.as_params() {
                params["env"] = env;
            }
            ("workspace.create", params)
        }
        BackendIntent::ResizePane { pane, direction, amount } => {
            let mut params = json!({
                "pane_id": names.backend(pane)?.as_str(),
                "direction": direction.as_str(),
            });
            if let Some(amount) = amount {
                params["amount"] = json!(amount);
            }
            ("pane.resize", params)
        }
        // `mode` is left to its default, which herdr documents as toggle. Sending it would be
        // Muster restating a default it agrees with, and the day herdr changes that default
        // is the day this should notice rather than silently keep the old one.
        BackendIntent::ZoomPane { pane } => {
            ("pane.zoom", json!({ "pane_id": names.backend(pane)?.as_str() }))
        }
        BackendIntent::ClosePane { pane } => {
            ("pane.close", json!({ "pane_id": names.backend(pane)?.as_str() }))
        }
        BackendIntent::FocusPane { pane } => {
            ("pane.focus", json!({ "pane_id": names.backend(pane)?.as_str() }))
        }
        // The text only. Return is `pane.send_input` afterwards, because herdr encodes a named
        // key against the pane's live modes and a newline in the text is just a newline - which
        // a harness reading a bracketed paste treats as more text rather than as a submission.
        BackendIntent::SendText { pane, text, enter: _ } => {
            ("pane.send_text", json!({ "pane_id": names.backend(pane)?.as_str(), "text": text }))
        }
        // `source_pane_id` and `target_pane_id`, the same pair the leftward-split rearrange
        // sends. Note that herdr moves its own cursor to the *source* pane whatever was
        // focused before (`observations/herdr-0.8.0.md` section 14); Muster's keyboard is its
        // own and is not moved by this.
        BackendIntent::SwapPanes { pane, with } => (
            "pane.swap",
            json!({
                "source_pane_id": names.backend(pane)?.as_str(),
                "target_pane_id": names.backend(with)?.as_str(),
            }),
        ),
        // `destination` is a tagged object rather than a bare id: herdr's `pane.move` can also
        // make a tab or a workspace to move into, and the tag is how it tells those apart.
        // Muster only ever names an existing tab, because a drop landed on a row in one.
        //
        // "after" becomes a rightward split of the pane it lands behind. herdr puts a new pane
        // on the `second` side and reads a split first-then-second, so `right` is exactly one
        // place later in the order the agent list shows - the ordering the intent asked for,
        // spelled in the only geometry herdr's `split` accepts (it takes right and down alone).
        //
        // `focus` is left false. Moving a pane is arranging the window rather than going
        // somewhere, and a drag that also took the keyboard would interrupt whatever is being
        // typed into the pane that had it.
        BackendIntent::MovePane { pane, tab, after } => (
            "pane.move",
            json!({
                "pane_id": names.backend(pane)?.as_str(),
                "destination": {
                    "type": "tab",
                    "tab_id": tab.as_str(),
                    "target_pane_id": names.backend(after)?.as_str(),
                    "split": "right",
                },
                "focus": false,
            }),
        ),
        BackendIntent::SetSplitRatio { tab, path, ratio } => (
            "layout.set_split_ratio",
            json!({
                "tab_id": tab.as_str(),
                "path": path.iter().map(|turn| *turn == Branch::Second).collect::<Vec<bool>>(),
                "ratio": ratio,
            }),
        ),
        // Null rather than an absent key, because herdr's `label` is nullable here and null is
        // its spelling for taking the name away. Sending nothing would leave the name alone,
        // which is a different request.
        BackendIntent::RenamePane { pane, name } => (
            "pane.rename",
            json!({
                "pane_id": names.backend(pane)?.as_str(),
                "label": name.as_ref().map_or(Value::Null, |name| json!(name)),
            }),
        ),
        // An empty string where the pane above sends null, because herdr's `tab.rename` takes a
        // required string and refuses a null. It does not restore the tab's number either - the
        // tab is left holding an empty name that survives a daemon restart
        // (`observations/herdr-0.8.0.md` section 16). Muster's own caption reads that as
        // unnamed and draws the place, so the window is right and herdr's own interface is not.
        BackendIntent::RenameTab { tab, name } => (
            "tab.rename",
            json!({ "tab_id": tab.as_str(), "label": name.clone().unwrap_or_default() }),
        ),
    })
}

/// Whether this intent leaves a pane behind that Muster has to have a name for.
///
/// Has to agree with [`request`], which decides whether the environment is sent at all - an
/// intent that makes a pane and is missed here makes one that cannot say which pane it is, and
/// the symptom is a `muster` command inside that pane refusing for a pane the window is drawing.
/// Public so a test can hold the pair together against herdr's own schema, which declares `env`
/// on exactly the calls that make a pane.
pub fn makes_a_pane(intent: &BackendIntent) -> bool {
    matches!(
        intent,
        BackendIntent::SplitPane { .. }
            | BackendIntent::CreateTab { .. }
            | BackendIntent::CreateWorkspace { .. }
    )
}

/// What a pane is called now, for the request that renamed one.
///
/// Read from the answer because there is nowhere else to read it: herdr emits no event for a
/// pane rename and has no topic for one, so this reply is the only thing that ever says the
/// name changed (`observations/herdr-0.8.0.md` section 16).
///
/// Keyed off the intent rather than off the shape of the answer, because `pane.rename` replies
/// with the same pane payload several other methods do - taking it from any of them would let
/// a focus or a resize restate a name it knows nothing about.
///
/// The pane comes from the intent and not from the reply. The reply names it too and agrees,
/// but the intent is what this window asked about, and a mirror updated from the answer's own
/// idea of which pane it was is a mirror that cannot notice the two disagreeing.
fn renamed(intent: &BackendIntent, result: &Value) -> Option<(PaneId, Option<String>)> {
    let BackendIntent::RenamePane { pane, .. } = intent else { return None };
    let name = result
        .get("pane")
        .and_then(|pane| pane.get("label"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    Some((pane.clone(), name))
}

/// The pane a request made, if it made one.
///
/// `pane.split` answers with the whole new pane nested under `pane`, and `workspace.create`
/// with the one it started the workspace off with under `root_pane` - the way every herdr
/// result nests under a key beside its `type`. Only the id is read: everything else about
/// them arrives on the event stream a moment later, and reading it here would be a second
/// source for facts the mirror already owns.
///
/// A shape that does not match is `None` rather than a refusal. The split happened - the
/// daemon said so - and the only cost of not finding the id is a keyboard that stays where it
/// was, which is worth less than turning a successful split into an error.
///
/// The daemon's own id, untranslated: this is the wire, and binding it to the name Muster
/// minted is [`HerdrBackend::settle`]'s job one line later.
fn created(result: &Value) -> Option<String> {
    let pane = result.get("pane").or_else(|| result.get("root_pane"))?;
    Some(pane.get("pane_id")?.as_str()?.to_string())
}

/// The tab a request made, if it made one.
///
/// `tab.create` answers with the tab under `tab`, and `workspace.create` with the one it
/// started the workspace off with. Read for the same reason the pane is: a tab nothing is
/// showing is a tab nobody asked for twice.
fn created_tab(result: &Value) -> Option<TabId> {
    Some(TabId::new(result.get("tab")?.get("tab_id")?.as_str()?))
}

/// The axis herdr splits along to produce a side, named the way herdr names it.
///
/// Two of Muster's four sides map onto the same request as their opposite, because herdr has
/// no other: a leftward split is a rightward one that has not been rearranged yet. The pair
/// `layout.rs` reads back in the other direction is this one with the sides dropped.
fn direction(side: Side) -> &'static str {
    match side {
        Side::Left | Side::Right => "right",
        Side::Up | Side::Down => "down",
    }
}

/// Whether herdr puts a new pane on this side only after being asked a second time.
fn swaps(side: Side) -> bool {
    matches!(side, Side::Left | Side::Up)
}

/// The arrangement a herdr result states, when it states one.
///
/// Reading a mutation's own answer rather than waiting to be told is what keeps the window on
/// the arrangement that exists: herdr broadcasts the same tree about a hundred milliseconds
/// later, and a client that only listens renders the arrangement being moved away from for six
/// frames and then jumps (`observations/herdr-0.8.0.md` section 14).
///
/// Two shapes, because herdr states one in two ways. `pane.swap` and `pane.resize` answer with
/// the flat rectangles `layout_updated` carries; `layout.set_split_ratio` answers with the
/// exported tree. Tried in that order and with no branch on the verb - which shape a result
/// carries is the result's own business, and naming each one here would be a table to keep in
/// step with a daemon that ships weekly.
///
/// A result that carries neither is `None`, which is the same answer as a result that states
/// no arrangement at all - and the right one, because the caller then waits for the broadcast,
/// which is what every arrangement change did before any of this.
///
/// `swapped` names the pair a compound intent exchanged, and is what lets the arrangement
/// herdr published between the two halves be reconstructed: its swap exchanges the ids sitting
/// in two places and leaves the places alone, so the tree it was is the tree it became with
/// those two ids put back.
fn settled(
    result: &Value,
    swapped: Option<(&PaneId, &PaneId)>,
    names: &Names,
) -> Option<SettledLayout> {
    let stated = nested(result, "layout")?;
    let layout = read_layout(stated, names).or_else(|| read_exported_layout(stated, names))?;
    let stale = swapped.map(|(one, other)| layout.with_panes_exchanged(one, other));
    Some(SettledLayout { layout, stale })
}

/// A field of a herdr result, wherever the result nests it.
///
/// Every result puts its payload under a key beside its `type` rather than at the top level
/// (`observations/herdr-0.8.0.md` section 6), and which key differs per verb. Looking one
/// level down rather than naming each one means a verb that starts answering with a layout
/// needs no change here.
fn nested<'a>(result: &'a Value, field: &str) -> Option<&'a Value> {
    if let Some(found) = result.get(field) {
        return Some(found);
    }
    result
        .as_object()?
        .iter()
        .find_map(|(key, value)| (key != "type").then(|| value.get(field)).flatten())
}
