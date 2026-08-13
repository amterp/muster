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

  #expect(command == "/build/debug/muster-bridge w1:p1 --control-socket /tmp/s.sock")
}

@Test("no socket means no socket argument, not an empty one")
func absentSocketIsOmitted() {
  // `--control-socket` with nothing after it would make the bridge read the next argument
  // as a path, or reject the whole command line.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "w1:p1", controlSocketPath: nil)

  #expect(command == "/build/debug/muster-bridge w1:p1")
  #expect(!command.contains("--control-socket"))
}
