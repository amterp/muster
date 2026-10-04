//! A window's life: opening onto what the daemons hold and what it left last time, asking a
//! machine for a tab when it has none, and closing, which keeps its tabs for a reopen.
//!
//! Separate from the live session because it is the order things happen in rather than what the
//! session holds: each step here reads or settles the session and lets go of it again, and the
//! order is the subject - a window says it is open before anything may write over the
//! arrangement it was reopened from.

use super::{
    ATTACHES, BTreeSet, BackendIntent, Daemon, DaemonId, Event, Health, Implicitly, Keyboard,
    Lifecycle, PaneId, Refusal, SESSION, Sent, ShutWindow, TabId, Window, WindowId,
    announce_presentation, arrangement_path, daemon_holding, daemon_holding_tab, event, ffi,
    fields, focus, focus_tab, follow_implicitly_if_nothing_else, has_spoken,
    local_daemon_to_fill_from, log, mint_tab, move_tab, poison, publish,
    reconcile_sidebar_with_problems, save, saved_arrangement, saved_arrangement_at,
    saved_presentation, settle_what_the_window_shows, submit,
};

/// Opens this window onto whatever the daemons hold.
///
/// What a bare `muster` means. No pane is named, so nothing decides which tab beyond the saved
/// arrangement and, failing that, the first tab this window is free to open onto.
///
/// Three steps, and each is the reason the next can be simple: be following something, put back
/// what was left, and settle what to show - which for a window somebody asked for means keeping
/// off the tabs another window has, and for a window with nothing at all means asking a machine
/// for a tab. That last one is what a fresh machine needs, where Muster has just started a
/// daemon that has not answered anything yet.
pub(crate) fn open(window: WindowId) -> Result<(), String> {
    // First, so the window takes its tabs back as the rest of this restores them.
    poison::lock(&SESSION, "session").windows[window].lifecycle = Lifecycle::Unopened;
    follow_implicitly_if_nothing_else(Implicitly::InBackground)?;
    restore_presentation(window);
    restore_font_sizes(window);
    say_this_window_is_open(window);
    reopen_what_was_left(window);
    // After the file has been read and before anything writes over it. Everything above reads
    // the arrangement; everything from here on is entitled to replace it - which is why this
    // sits above the last two steps rather than below them, and it has to. Asking for a tab
    // is answered by the daemon on its own thread, and the region for it is opened
    // by the standing rule when that answer lands; a window that had not yet said it was open
    // would turn that rule off and wait forever for a region nothing else will make.
    let brings = {
        let mut session = poison::lock(&SESSION, "session");
        session.windows[window].lifecycle = Lifecycle::Open;
        session.windows[window].brings.take()
    };
    // Before settling, so a window opened onto a tab is not empty and asks no machine for one.
    if let Some(tab) = brings
        && let Err(refusal) = move_tab(window, Some(tab.clone()), "")
    {
        log::warn(
            "window.tab.failed",
            fields! {
                "tab" => tab.to_string(),
                "detail" => refusal.to_string(),
                "impact" => "the window opened onto a tab of its own rather than the one asked for",
                "check" => "whether that tab closed while the window was opening",
            },
        );
    }
    settle_what_the_window_shows(window);
    // A window just opened has been sent nothing, whatever an earlier one was.
    {
        let mut session = poison::lock(&SESSION, "session");
        let window = &mut session.windows[window];
        window.sent = Sent::default();
        log::info(
            "window.opened",
            fields! {
                "window" => window.name.to_string(),
                "arrangement" => arrangement_path(window).unwrap_or_default(),
            },
        );
    }
    publish("open");
    show_what_was_asked_for(window);
    Ok(())
}

/// Closes one window, leaving the others and the app running.
///
/// It keeps its tabs: the record still names it as their holder, so going to one asks for it back
/// (`reopened_for`), and the next launch leaves it closed. Saved once more first, so a reopen
/// comes back to what was on screen when it closed rather than the last arrangement that happened
/// to differ.
pub(crate) fn close_window(window: WindowId) {
    {
        let mut guard = poison::lock(&SESSION, "session");
        let session = &mut *guard;
        if !session.windows[window].is_open() {
            return;
        }
        save(session, window);
        let name = session.windows[window].name.clone();
        session.holding.close(&name);
        session.windows[window] = session.windows[window].closed();
        if session.front == window {
            session.front = session.front_after_closing(window);
        }
        log::info("window.closed", fields! { "window" => name.to_string() });
    }
    publish("window_closed");
}

/// Asks the shell to close a window as its close button would, for somebody outside the shell:
/// `muster window close`.
///
/// A window already closed has nothing to ask. The last window open is refused rather than
/// asked for, because closing it is a quit (mip/0006-one-process.md, section 4): a script that
/// meant to close one window should not end them all, and a person who means to quit has cmd+q.
pub(crate) fn ask_to_close_window(window: WindowId) -> Result<(), String> {
    let name = {
        let mut session = poison::lock(&SESSION, "session");
        let asked = &session.windows[window];
        if asked.lifecycle != Lifecycle::Open {
            return Ok(());
        }
        let staying =
            session.windows.values().filter(|open| open.lifecycle == Lifecycle::Open).count();
        if staying == 1 {
            return Err(format!(
                "{} is the only window open, and closing the last window quits Muster, so \
                 nothing was closed. Quit with cmd+q if that is what you meant: the agents keep \
                 running, and every window comes back on the next launch.",
                asked.name
            ));
        }
        let name = asked.name.to_string();
        // Marked under the same lock as the count, so a second close asked at once counts this one.
        session.windows[window].lifecycle = Lifecycle::Closing;
        name
    };
    log::info("window.close.asked", fields! { "window" => name.clone() });
    ffi::emit(&Event::new(event::Payload::ShutWindow(ShutWindow {})).for_window(name));
    Ok(())
}

/// The shell was asked to close a window and it is still open, so it is counted open again.
///
/// Without this the window would stay closing for good: asking to close it again would do
/// nothing, and it would be left out of the count that refuses closing the last window, so the
/// window beside it could not be closed either.
pub(crate) fn still_open(window: WindowId) {
    let name = {
        let mut session = poison::lock(&SESSION, "session");
        let kept = &mut session.windows[window];
        if kept.lifecycle != Lifecycle::Closing {
            return;
        }
        kept.lifecycle = Lifecycle::Open;
        kept.name.to_string()
    };
    log::warn(
        "window.close.kept",
        fields! {
            "window" => name,
            "impact" => "the window was asked to close and is still open, though `muster window \
                         close` answered that it had asked",
            "check" => "whether a sheet or a dialog is up in that window, which disables its \
                        close button; close it there, or ask again once it is answered",
        },
    );
}

/// Which window an `OpenWindow` means, and whether it is one this process has just taken on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opening {
    /// A window here that has not opened yet: the one Startup described, on an ordinary launch.
    Unopened(WindowId),
    /// A window here already open, which there is nothing more to do for.
    AlreadyOpen(WindowId),
    /// A window taken on for this request, beside the ones already here.
    Added(WindowId),
}

/// Works out which window an `OpenWindow` is about, taking another on when it names one this
/// process has not got.
///
/// `arrangement` is the file the window remembers itself in: a window here already writing
/// there is that window, and an empty one means `window` itself. Only a window that names an
/// arrangement nobody here writes is taken on: one asked for with no arrangement could never be
/// told apart from the first window opened twice, and a stray window holding tabs is a window that
/// takes the next tab nobody asked for. Every daemon this process follows is one the new window
/// follows too - there is one set of daemons for every window - so its composition starts
/// attached to each of them, in the order the others have them.
pub(crate) fn window_to_open(
    window: WindowId,
    arrangement: &str,
    show: &str,
    first_tab_on: &str,
    brings: &str,
) -> Opening {
    let mut session = poison::lock(&SESSION, "session");
    let existing = if arrangement.is_empty() {
        Some(window)
    } else {
        session
            .windows
            .iter()
            .find(|(_, held)| arrangement_path(held).as_deref() == Some(arrangement))
            .map(|(id, _)| id)
    };
    if let Some(existing) = existing {
        if session.windows[existing].is_open() {
            return Opening::AlreadyOpen(existing);
        }
        if !show.is_empty() {
            session.windows[existing].show = Some(show.to_string());
        }
        session.windows[existing].first_tab_on =
            (!first_tab_on.is_empty()).then(|| DaemonId::new(first_tab_on));
        session.windows[existing].brings = (!brings.is_empty()).then(|| TabId::new(brings));
        return Opening::Unopened(existing);
    }
    let name = session.holding.register(arrangement);
    let daemons: Vec<Daemon> =
        session.windows[session.front].composition.daemons().cloned().collect();
    let mut added = Window {
        name,
        arrangement: (!arrangement.is_empty()).then(|| (arrangement.to_string(), String::new())),
        show: (!show.is_empty()).then(|| show.to_string()),
        first_tab_on: (!first_tab_on.is_empty()).then(|| DaemonId::new(first_tab_on)),
        brings: (!brings.is_empty()).then(|| TabId::new(brings)),
        ..Window::default()
    };
    for daemon in daemons {
        added.composition.attach_daemon(daemon);
    }
    log::info(
        "window.added",
        fields! { "window" => added.name.to_string(), "arrangement" => arrangement },
    );
    Opening::Added(session.windows.add(added))
}

/// What the first window was launched to go to, as Startup says.
pub(crate) fn set_show(show: &str) {
    let mut session = poison::lock(&SESSION, "session");
    let first = session.front;
    session.windows[first].show = (!show.is_empty()).then(|| show.to_string());
}

pub(super) fn show_what_was_asked_for(window: WindowId) {
    let Some(show) = poison::lock(&SESSION, "session").windows[window].show.take() else { return };
    let tab = TabId::new(&show);
    let went = if daemon_holding_tab(&tab).is_some() {
        focus_tab(window, &tab)
    } else {
        let pane = PaneId::new(&show);
        match daemon_holding(&pane) {
            Some(daemon) => focus(window, &daemon, &pane).map_err(|refusal| refusal.to_string()),
            None => Err(format!("no daemon holds a pane or tab called {show}")),
        }
    };
    if let Err(detail) = went {
        log::warn(
            "window.show.failed",
            fields! {
                "show" => &show,
                "detail" => detail,
                "impact" => "the window opened where it was left rather than on what it was \
                             reopened for",
                "check" => "whether that tab or pane closed while the window was opening",
            },
        );
    }
}

/// Puts back the window's own chrome, and tells the shell either way.
///
/// Separate from the regions, and ahead of them, because it survives conditions they do not.
/// A saved region is a wish about a session that may be gone, so restoring one can come to
/// nothing; nobody else has an opinion about whether a list was open, so this always applies.
/// Folding it into `reopen_what_was_left` would tie it to that function's early return, and a
/// person who put the sidebar away would find it back whenever their tabs did not survive.
///
/// Announced unconditionally, including when there was nothing to read, so the shell is told
/// the default rather than holding one.
pub(super) fn restore_presentation(window: WindowId) {
    let saved = saved_presentation(window);
    let presentation = {
        let mut session = poison::lock(&SESSION, "session");
        // The frame is the one field the shell may already have answered. It asks where to open
        // before showing the window and reports where it actually opened, and either of those
        // can land before this runs - so a wholesale assignment here would throw away the
        // rectangle the window is currently at and write the wish back over the answer. Keeping
        // what is already set makes the two orders agree, which is what `open()` overwriting
        // presentation wholesale has caught out before.
        let presentation = match session.windows[window].presentation.frame {
            Some(_) => saved.with_frame(
                session.windows[window].presentation.frame,
                session.windows[window].presentation.full_screen,
            ),
            None => saved,
        };
        session.windows[window].presentation = presentation;
        presentation
    };
    // Then let anything already wrong have its say. A config refused during `Startup` raised
    // its problem before this ran, so without this the saved answer would quietly win and a
    // window would come back with the roster away and nowhere to report a broken file. It
    // announces for itself when it moves anything, which is why this one is conditional.
    if !reconcile_sidebar_with_problems(window) {
        let name = poison::lock(&SESSION, "session").windows[window].name.clone();
        announce_presentation(&name, presentation);
    }
}

/// Puts back how big each pane's text was.
///
/// Beside the chrome rather than folded into it, because the two are different answers: the
/// chrome is one statement about the window and this is a row per pane somebody sized.
///
/// Restored whole and unchecked, unlike a region. An entry naming a pane that is gone costs a
/// row nobody reads until the next publish drops it; a region naming a tab that is gone is a
/// square on screen that never fills in. The pruning belongs where a daemon that is actually
/// answering can say what it still holds, which is `forget_what_closed`.
///
/// Nothing is announced. Every size here reaches the shell on the pane it belongs to, and the
/// publish that ends `open()` is what carries them.
pub(super) fn restore_font_sizes(window: WindowId) {
    let Some(saved) = saved_arrangement(window) else { return };
    if saved.font_sizes.entries().next().is_none() {
        return;
    }
    log::info(
        "state.font_size.restored",
        fields! { "panes" => saved.font_sizes.entries().count().to_string() },
    );
    // Merged rather than replaced, and only where nothing here has sized the pane already: every
    // window writes every size it knows, so this file may hold another window's pane as it was
    // when this one last saved, and the size that pane has now is the one somebody chose.
    let mut session = poison::lock(&SESSION, "session");
    for (pane, offset) in saved.font_sizes.entries() {
        if !session.font_sizes.entries().any(|(sized, _)| sized == pane) {
            session.font_sizes.set(pane, offset);
        }
    }
}

/// Tells the other windows this one is open, and brings this window's idea of who holds what up
/// to date.
///
/// Before the restore, which reads the answer: a tab another window took while this one was
/// closed is not this window's to reopen.
pub(super) fn say_this_window_is_open(window: WindowId) {
    {
        let mut session = poison::lock(&SESSION, "session");
        if session.holding.has_opened(&session.windows[window].name) {
            return;
        }
        // A daemon still restoring has not described every tab it will hold.
        let mut answered = BTreeSet::new();
        let mut described = BTreeSet::new();
        for (daemon, backend) in &session.backends {
            let mirror = poison::lock(&backend.mirror, "mirror");
            if mirror.health() == Health::Connected && !mirror.restoring() {
                answered.insert(daemon.clone());
                described.extend(mirror.tabs().map(|tab| tab.id.clone()));
            }
        }
        let followed = session.followed_or_attaching();
        session.holding.follow(followed);
        let me = session.windows[window].name.clone();
        session.holding.open(&me, &answered, |tab| described.contains(tab));
    }
    // The daemons described their tabs before this window had said it was open, when it could
    // not be the one to take them. On a first launch that is every tab there is.
    take_what_nobody_holds();
}

/// Takes the tabs nobody holds that are this window's to take, on every machine it follows, and
/// says whether it took any.
///
/// What a tab nobody holds needs asked again whenever the answer to "whose is it" may have become
/// this window: when it says it is open, and when it comes to the front. Otherwise nothing asks
/// until the daemon next says something, which on a quiet session is never.
pub(super) fn take_what_nobody_holds() -> bool {
    let mut session = poison::lock(&SESSION, "session");
    let daemons: Vec<DaemonId> = session.backends.keys().cloned().collect();
    let mut took = false;
    for daemon in &daemons {
        if session.settle_holding(daemon) {
            session.reconcile(daemon);
            took = true;
        }
    }
    took
}

/// Puts back the regions this window was showing when it last closed.
///
/// Before the two rules under it rather than instead of them, which is what makes this an
/// addition and not a special case: a daemon whose saved regions all turned out to be gone
/// falls through to getting a region of its own, and a window where every daemon did falls
/// through to asking for a tab. So a first launch, a launch after a reboot took
/// everything, and a launch onto a session still running are one path with different amounts
/// of it doing anything.
///
/// Checked against the mirror, which by now holds each attached daemon's snapshot: a saved
/// region is a wish, and a tab nobody holds any more would render as a square that never
/// fills in.
pub(super) fn reopen_what_was_left(window: WindowId) {
    let mut session = poison::lock(&SESSION, "session");
    // Tabs given to this window while it was closed are its own as much as the ones it was left
    // on, and come after them.
    let given: Vec<TabId> =
        session.holding.holders().held_by(&session.windows[window].name).cloned().collect();
    for tab in given {
        session.windows[window].composition.hold(tab);
    }
    // Read under the lock this already holds, which `saved_arrangement` would take again.
    let Some(saved) =
        arrangement_path(&session.windows[window]).and_then(|path| saved_arrangement_at(&path))
    else {
        return;
    };

    // Only the tabs that are still this window's. One another window has taken since is that
    // window's now, and one nobody holds - every tab, the first launch after holding was written
    // down - is taken here, which is how a window that was alone comes back exactly as it was.
    let listed: Vec<TabId> = saved.tabs.iter().map(|tab| tab.id.clone()).collect();
    let me = session.windows[window].name.clone();
    session.holding.keep(&me, &listed);
    // A daemon whose attach finished while this waited for the session has been restored here
    // already, and one still restoring has more tabs to describe than it has so far.
    let attaching = poison::lock(&ATTACHES, "attaches").under_way.clone();
    let still_describing = |daemon: &DaemonId| match session.backends.get(daemon) {
        Some(backend) => {
            let mirror = poison::lock(&backend.mirror, "mirror");
            mirror.health() == Health::Disconnected || mirror.restoring()
        }
        None => attaching.contains(daemon),
    };
    let awaiting: BTreeSet<DaemonId> = saved
        .tabs
        .iter()
        .flat_map(|tab| tab.regions.iter().map(|region| region.daemon.clone()))
        .filter(|daemon| still_describing(daemon))
        .collect();
    session.windows[window].awaiting = awaiting;
    if !session.windows[window].awaiting.is_empty() {
        session.windows[window].left = Some(saved.clone());
    }
    let restorable = saved.restorable(|daemon, tab| {
        session.holding.holds(&session.windows[window].name, tab)
            && session
                .backends
                .get(daemon)
                .is_some_and(|backend| poison::lock(&backend.mirror, "mirror").tab(tab).is_some())
    });
    if restorable.tabs.is_empty() {
        return;
    }

    let mut regions = 0usize;
    for tab in &restorable.tabs {
        for region in &tab.regions {
            // Idempotent per machine per tab, so a file holding the same region twice heals
            // here rather than opening a second one. Two regions on one machine's half of a
            // tab render the same pane twice, and only one of the two surfaces can have the
            // pane - the other is refused, and becomes a panel that cannot be closed, because
            // closing it would close the pane the live one is using (kan a_2Ht74jTXV).
            let Some(id) =
                session.windows[window].composition.open_region(&region.daemon, tab.id.clone())
            else {
                continue;
            };
            session.windows[window].composition.set_weight(id, region.weight);
            if let Some(pane) = &region.pane {
                session.windows[window].composition.focus_pane(id, pane.clone());
            }
            if region.keyboard {
                session.windows[window].composition.focus_region(id);
            }
            regions += 1;
        }
    }
    if let Some(showing) = &restorable.showing {
        session.windows[window].composition.show(showing);
    }

    log::info(
        "composition.restored",
        fields! {
            "tabs" => restorable.tabs.len().to_string(),
            "regions" => regions.to_string(),
            "showing" => restorable.showing.as_ref().map(ToString::to_string).unwrap_or_default(),
            "dropped" => (saved.tabs.len() - restorable.tabs.len()).to_string(),
        },
    );
}

/// Asks for one tab when this window has no tab it may open onto.
///
/// The one rule that makes a window out of nothing, and the only one left: a window is not a
/// window if it is showing nothing, so something has to fill it. It picks the first local
/// machine the config names, waiting for it while it attaches, because a remote one is somebody
/// else's and choosing it uninvited is a bigger claim than filling a window.
///
/// **Once per launch, per machine, and never again.** Nothing takes a machine back out of
/// `tabs_asked_of`, so a machine whose panes all close later stays empty - which is the
/// answer `a_2I6h18OU6` settled on, now that `muster pane new --daemon <id>` and the machines at
/// the foot of the agent list are both ways back in. The rule this replaces gave every attached
/// machine a column and refilled it the moment it emptied, so somebody finished with a devenv
/// for the afternoon got a fresh ssh shell every time they closed one.
///
/// Nothing is asked of a machine still restoring what it held before it last stopped. Its tabs
/// are on their way, and a tab asked for now would be one more beside them; it is asked once
/// it says it has finished, if the window is still empty then.
pub(super) fn open_a_tab_if_the_window_is_empty(window: WindowId) {
    let empty = {
        let session = poison::lock(&SESSION, "session");
        session.windows[window].composition.showing().is_none()
    };
    if !empty {
        return;
    }

    let asked_for = poison::lock(&SESSION, "session").windows[window].first_tab_on.clone();
    let Some(daemon) = asked_for.or_else(|| local_daemon_to_fill_from(window)).filter(has_spoken)
    else {
        log::info(
            "window.empty",
            fields! {
                "impact" => "this window shows nothing yet, because no machine on this one has \
                             both answered and offered a tab it may open onto",
                "check" => "nothing, on the way up - this runs again on every machine's first \
                            snapshot. If it stays this way, `muster pane new --daemon <id>` \
                            names a machine directly, and so does the row for it at the foot \
                            of the agent list. What this rule will not do is pick somebody \
                            else's machine to fill a window with",
            },
        );
        return;
    };

    // Recorded before the ask rather than after: this runs again on the reconcile behind every
    // event, and the machine holds nothing until its answer arrives - so a record written
    // afterwards would ask two or three times over.
    {
        let mut session = poison::lock(&SESSION, "session");
        if !session.windows[window].tabs_asked_of.insert(daemon.clone()) {
            return;
        }
    }
    ask_for_a_tab(window, &daemon);
}

/// Asks one machine for a tab, and says what a refusal costs.
///
/// One caller for the empty window and one for a machine holding nothing, so what happens when
/// a daemon says no is written down once.
pub(super) fn ask_for_a_tab(window: WindowId, daemon: &DaemonId) {
    log::info("tab.first.creating", fields! { "daemon" => daemon.to_string() });
    let intent = BackendIntent::CreateTab { tab: mint_tab(), cwd: None, run: None, name: None };
    let asked = submit(window, daemon, &intent, Keyboard::Follows);
    if let Err(Refusal::Unanswered { detail, .. }) = &asked {
        log::warn(
            "tab.first.unanswered",
            fields! {
                "daemon" => daemon.to_string(),
                "detail" => detail.clone(),
                "impact" => "the daemon may have made the tab, and if it did it arrives on \
                             its own events and this machine fills in. If it did not, this \
                             machine shows nothing in this window, and nothing will ask again - \
                             asking twice could make two",
                "check" => "whether the daemon is keeping up at all: it received the request and \
                            did not answer in time. `muster pane new --daemon <id>` asks by hand \
                            once `muster window` shows the machine still holds nothing",
            },
        );
    } else if let Err(refusal) = asked {
        log::error(
            "tab.first.refused",
            fields! {
                "daemon" => daemon.to_string(),
                "detail" => refusal.to_string(),
                "impact" => "this machine shows nothing in this window and stays that way \
                             until something makes a pane on it. Nothing will ask again - a \
                             rule that retried a refusal would retry it on every event that \
                             arrives from anywhere",
                "check" => "the daemon's own log - it answered its socket, so this is a \
                            refusal rather than an absence. `muster pane new --daemon <id>` \
                            asks once more by hand",
            },
        );
    }
}
