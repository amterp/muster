import AppKit
import UserNotifications

/// What a banner about a pane says, decided apart from the framework that posts it.
///
/// Pure and separate from the delivery below, on the same terms as `PaneAppearance`: these
/// are the words somebody reads at the moment they are deciding whether to get up, and a
/// decision inside a `UNUserNotificationCenter` delegate is a decision no test can reach.
///
/// The three lines are the roster's two plus the reason. That is not a coincidence and is
/// the reason the core sends a label at all: a notification naming an agent differently
/// from the row it appears on is two names for one thing, and the person reading the banner
/// is about to go looking for that row.
public enum PaneNotification {
  /// The one line that has to be enough on its own, because a banner is often all somebody
  /// sees. The pane's id stands in when the core sent no label, which is a pane the mirror
  /// had already let go of - rare, and better than a blank line.
  public static func title(label: String, paneID: String) -> String {
    label.isEmpty ? paneID : label
  }

  /// Why this pane is asking, in the fewest words that are still true.
  ///
  /// `done` says only that the agent finished, not that nobody saw it. That it is being
  /// said at all is what "nobody saw it" means here, and spelling out Muster's own rule
  /// would explain the product to somebody who wants to know whether their build is done.
  public static func reason(state: String) -> String {
    switch state {
    case "blocked": return "is waiting on you"
    case "done": return "has finished"
    // Never sent by the core, which raises only those two. A banner is the wrong place to
    // report a seam disagreement, so this says the honest minimum rather than nothing.
    default: return "needs you"
    }
  }

  /// One banner per pane, so a pane that asks again replaces its own rather than stacking,
  /// and a withdrawal has something to find. Two daemons hand out the same pane ids, which
  /// is why the machine is in it.
  public static func identifier(daemon: String, pane: String) -> String {
    "\(daemon)/\(pane)"
  }

  /// Where a posted banner carries the pane it is about, so an activated one can be turned
  /// back into a focus request. Here rather than beside the poster because they are part of
  /// what a notification *is*, and because the delegate reading them is not on the main actor.
  public static let daemonKey = "daemon"
  public static let paneKey = "pane"
}

/// Posts the core's attention events as macOS notifications, and turns an activated one back
/// into an ordinary focus request.
///
/// The delivery half of attention routing, and nothing more. Which panes are asking, which
/// states are worth interrupting for, and whether the window was already showing this one are
/// all decided in the core (`crates/muster-core/src/attention.rs`); what is left here is the
/// part that genuinely needs an OS - posting the thing, taking it down, and noticing a click.
///
/// **Needs a bundle, and says so rather than dying.** `UNUserNotificationCenter.current()`
/// wants a bundle identifier to attach a permission grant to, and raises rather than returning
/// nil for a process with none - which is every `swift build` binary out of `.build`. So the
/// whole path is behind one check, and a bare binary keeps working with its notifications off.
///
/// A bundle is necessary and not sufficient: macOS refuses to grant the permission to an
/// ad-hoc signature, which is what `./dev --bundle` produces unless `MUSTER_SIGN_IDENTITY`
/// names a Developer ID (`docs/observations/macos-26.4.1.md`). So a contributor sees the same
/// refusal a person who said no would, which is why the log line names both causes.
@MainActor
public final class PaneNotifier: NSObject, UNUserNotificationCenterDelegate {
  public static let shared = PaneNotifier()

  /// Nil when this process has no bundle to be granted permission as. Held rather than asked
  /// for each time, because asking is what raises.
  private let center: UNUserNotificationCenter?

  override init() {
    center = Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
    super.init()
    center?.delegate = self
  }

  /// Asks for permission, once, at a moment somebody can attribute to launching Muster.
  ///
  /// At launch rather than at the first blocked agent, because the alternative is a system
  /// prompt appearing at an arbitrary moment hours later - and the banner that prompt was
  /// about is lost behind it. macOS asks the person once ever and remembers the answer, so
  /// this is free on every launch after the first.
  public func start() {
    guard let center else {
      Core.warn(
        "notifications.unbundled",
        [
          "impact": "an agent that needs you will show it on its row and its border and will "
            + "not interrupt you, so a pane no region is showing can wait unnoticed",
          "cause": "this process has no bundle identifier, which is what a notification "
            + "permission is granted against - so it is a binary run straight out of .build",
          "fix": "`./dev --bundle` and launch .build/muster.app, which is also how a release "
            + "runs",
        ])
      return
    }
    center.requestAuthorization(options: [.alert, .sound]) { granted, error in
      Task { @MainActor in
        if let error {
          Core.warn(
            "notifications.refused",
            [
              "error": "\(error)",
              "impact": "no agent will interrupt you; the roster and the borders still say "
                + "who needs you",
              // Two very different causes and one message from the OS, so both are named.
              // A local `./dev --bundle` is ad-hoc signed, and macOS refuses to grant a
              // notification permission to an ad-hoc signature at all - measured, and the
              // whole of it is in docs/observations/macos-26.4.1.md. That is the one a
              // contributor hits, and it looks exactly like a person having said no.
              "check": "System Settings > Notifications > Muster, if this build was signed "
                + "with a Developer ID. An ad-hoc `./dev --bundle` cannot be granted this at "
                + "all, whatever the settings say - set MUSTER_SIGN_IDENTITY to try it",
            ])
          return
        }
        Core.info("notifications.authorized", ["granted": String(granted)])
      }
    }
  }

  /// One pane started asking for somebody, or stopped.
  ///
  /// An empty state is the withdrawal. Taking a delivered banner down matters more here than
  /// in most apps: activating one focuses the pane that raised it, so a stale banner is a
  /// keystroke that lands somebody on an agent which stopped needing them.
  public func apply(
    daemon: String, pane: String, state: String, label: String, subtitle: String
  ) {
    guard let center else { return }
    let id = PaneNotification.identifier(daemon: daemon, pane: pane)
    guard !state.isEmpty else {
      center.removeDeliveredNotifications(withIdentifiers: [id])
      center.removePendingNotificationRequests(withIdentifiers: [id])
      return
    }

    let content = UNMutableNotificationContent()
    content.title = PaneNotification.title(label: label, paneID: pane)
    content.subtitle = subtitle
    content.body = PaneNotification.reason(state: state)
    content.sound = .default
    // Read back when somebody activates it. The pane's name alone would do on one machine
    // and would reach the wrong pane on two.
    content.userInfo = [
      PaneNotification.daemonKey: daemon, PaneNotification.paneKey: pane,
    ]

    // No trigger: posted now. A request with the same identifier replaces the one on screen,
    // which is what a pane that asks twice should do.
    center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil)) { error in
      guard let error else { return }
      Task { @MainActor in
        Core.warn(
          "notifications.post.failed",
          [
            "pane": id,
            "error": "\(error)",
            "impact": "this one agent's request for you went unannounced; its row and its "
              + "border still carry the state",
            "check": "whether notifications are allowed for Muster in System Settings",
          ])
      }
    }
  }

  /// Shows the banner even while Muster is the app in front.
  ///
  /// macOS suppresses a foreground app's own notifications by default, which is exactly wrong
  /// here: the core has already decided this window is not showing this pane, and "Muster is
  /// frontmost" is not the same claim. Somebody reading one agent while fourteen others work
  /// is the ordinary case, and it is the case that suppression would silence.
  nonisolated public func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    willPresent notification: UNNotification
  ) async -> UNNotificationPresentationOptions {
    [.banner, .sound]
  }

  /// Somebody activated a banner, so send them to the pane that raised it.
  ///
  /// An ordinary focus request through the one action path, which is what makes the pane
  /// reachable at all: a focus naming a pane no region shows retargets a region onto it, so
  /// this reaches a hidden pane the same way a chord or the CLI would. Nothing here is a
  /// second way to move the keyboard.
  nonisolated public func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    didReceive response: UNNotificationResponse
  ) async {
    let info = response.notification.request.content.userInfo
    guard let daemon = info[PaneNotification.daemonKey] as? String,
      let pane = info[PaneNotification.paneKey] as? String
    else { return }
    await MainActor.run {
      Core.info("notifications.activated", ["daemon": daemon, "pane": pane])
      // Before the focus request, so the window is in front by the time the core answers with
      // an arrangement that may have had to change to show this pane at all.
      NSApp.activate(ignoringOtherApps: true)
      Core.focus(daemonID: daemon, paneID: pane)
    }
  }
}
