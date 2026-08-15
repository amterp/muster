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
