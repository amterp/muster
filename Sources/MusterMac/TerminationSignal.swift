import AppKit
import Dispatch

/// Quits on SIGTERM the way ⌘Q does, rather than dying where the signal lands.
///
/// A quit is how the core hears that the app is going (`Core.quitting`) and how every window's
/// claim on its arrangement is given up, and `kill <pid>` - or anything else that ends the app
/// with the default signal - is somebody asking it to quit. Dying instead was a crash as far as
/// the next launch could tell; that costs nothing now that a crash reopens every window too, but
/// it skips what a quit says on the way out.
@MainActor
public enum TerminationSignal {
  /// Held for the life of the app: dropping it stops the watch.
  private static var source: DispatchSourceSignal?

  public static func quitsTheApp() {
    // Handled rather than ignored, so the signal still arrives as an event below and does not end
    // the process first. Not `SIG_IGN`: an ignored signal stays ignored across `exec`, so every
    // ssh master and bridge this app starts would shrug off the SIGTERM meant to end it, where a
    // handled one goes back to the default in the child.
    signal(SIGTERM, noticeTermination)
    let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
    source.setEventHandler {
      MainActor.assumeIsolated {
        Core.info("app.terminate.signalled", [:])
        NSApp.terminate(nil)
      }
    }
    source.resume()
    self.source = source
  }
}

/// Does nothing, so that the dispatch source above is what acts on the signal.
private func noticeTermination(_ signal: Int32) {}
