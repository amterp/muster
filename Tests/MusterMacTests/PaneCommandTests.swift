import Testing

@testable import MusterMac

// A wrong command string here does not crash. It produces a window that renders and
// ignores the keyboard - which is exactly what shipped, twice.

@Test("the bridge is found next to the app, not on PATH")
func bridgeSitsBesideTheExecutable() {
  // Both binaries come out of the same build directory, and resolving by name instead
  // would find whatever an older install left on PATH.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock")

  #expect(command == "'/build/debug/muster-bridge' 'w1:p1' '--control-socket' '/tmp/s.sock'")
}

@Test("no socket means no socket argument, not an empty one")
func absentSocketIsOmitted() {
  // `--control-socket` with nothing after it would make the bridge read the next argument
  // as a path, or reject the whole command line.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: nil)

  #expect(command == "'/build/debug/muster-bridge' 'w1:p1'")
  #expect(!command.contains("--control-socket"))
}

@Test("a remote pane runs its frame stream through the master the core opened")
func remotePaneRidesTheMaster() {
  // The one difference between a local pane and a devenv one, end to end. Reusing the
  // master is what keeps a pane cheap: a window of fifteen remote panes pays for one
  // handshake rather than fifteen.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock",
    sshHost: "dev@localhost", sshControlPath: "/tmp/muster-1-devenv.ctl")

  #expect(
    command.hasSuffix("'--via-ssh' 'dev@localhost' '--ssh-control' '/tmp/muster-1-devenv.ctl'"))
}

@Test("half an ssh target is no ssh target")
func halfATargetIsIgnored() {
  // A host with no master would open a connection of its own, and a master with no host
  // names nothing. Either way the pane would render the wrong machine or nothing at all, so
  // the command is built as though it were local and the bridge says so.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock",
    sshHost: "dev@localhost", sshControlPath: nil)

  #expect(!command.contains("--via-ssh"))
}

@Test("a local pane's bridge is told which herdr to run")
func localPaneCarriesTheDaemon() {
  // The whole of kan a_2Hnh3g0Y5. Left to find one for itself the bridge looks beside its own
  // executable and then on PATH, and a shipped bundle has neither: the daemon lives in
  // Contents/Library, and a Launch Services app is handed launchd's PATH, every entry of it
  // SIP-protected. Every pane of the 0.3.0 cask rendered nothing.
  let command = PaneCommand.bridge(
    executable: "/Applications/Muster.app/Contents/MacOS/muster", paneID: "w1:p1",
    controlSocketPath: "/tmp/s.sock",
    herdrBinaryPath:
      "/Applications/Muster.app/Contents/Library/MusterSessions.app/Contents/MacOS/herdr"
  )

  #expect(
    command.contains(
      "'--herdr-binary' "
        + "'/Applications/Muster.app/Contents/Library/MusterSessions.app/Contents/MacOS/herdr'"))
}

@Test("a remote pane is not told a path from this machine")
func remotePaneResolvesItsOwnDaemon() {
  // A local path names nothing on a devenv, and the bridge's far-side script already prefers
  // the herdr Muster installed over there. Sending one would either find nothing or, worse,
  // find some unrelated binary of the same name.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock",
    herdrBinaryPath: "/build/debug/herdr",
    sshHost: "dev@localhost", sshControlPath: "/tmp/muster-1-devenv.ctl")

  #expect(!command.contains("--herdr-binary"))
}

@Test("half an ssh target still gets the daemon, because it builds a local command")
func halfATargetKeepsTheDaemon() {
  // The two decisions read the same answer. Split apart, a host with no master would build a
  // command that runs here and leaves the daemon out - a pane that renders nothing, by a
  // second route to the same bug.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock",
    herdrBinaryPath: "/build/debug/herdr",
    sshHost: "dev@localhost", sshControlPath: nil)

  #expect(!command.contains("--via-ssh"))
  #expect(command.contains("'--herdr-binary' '/build/debug/herdr'"))
}

@Test("no daemon path means no argument, not an empty one")
func absentDaemonIsOmitted() {
  // A bridge run by hand is handed nothing and falls back to looking, which is how this gets
  // debugged. An empty value would make it try to exec "".
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock",
    herdrBinaryPath: nil)

  #expect(!command.contains("--herdr-binary"))
}

@Test("an argument with a space in it stays one argument")
func spacesSurviveTheCommandLine() {
  // Everything here reaches libghostty as one string and is split on spaces on the way to a
  // process. A path with a space in it would become two arguments, and the pane would render
  // nothing for a reason no log line would name.
  let command = PaneCommand.bridge(
    executable: "/Users/some one/build/muster", paneID: "w1:p1",
    controlSocketPath: "/tmp/a b.sock")

  #expect(command.contains("'/Users/some one/build/muster-bridge'"))
  #expect(command.contains("'/tmp/a b.sock'"))
}

@Test("a replacement bridge takes the terminal over, and a first one does not")
func onlyAReattachTakesOver() {
  // Only one client may hold a herdr terminal, and a client whose transport died goes on
  // holding one - measured at 53 minutes after its ssh was gone. So a pane whose connection
  // was lost cannot be re-attached without this, and every network change locked another one.
  // A first attach never takes over: that terminal could be one another window is showing.
  let first = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock")
  let again = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: "/tmp/s.sock",
    reattaching: true)

  #expect(!first.contains("--takeover"))
  #expect(again.hasSuffix("'--takeover'"))
}
