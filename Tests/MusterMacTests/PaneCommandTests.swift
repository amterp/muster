import Testing

@testable import MusterMac

// A wrong command string here does not crash. It produces a window that renders nothing and
// ignores the keyboard - which is exactly what shipped, twice.

@Test("a local pane's bridge dials the daemon and reports to the window")
func aLocalPaneDialsTheDaemon() {
  // The bridge is found beside the app rather than on PATH, because both binaries come out of
  // the same build and resolving by name would find whatever an older install left behind.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "p1w3r07bsd", daemonSocket: "/tmp/d.sock",
    linkSocketPath: "/tmp/p1.sock")

  #expect(
    command
      == "'/build/debug/muster-bridge' 'p1w3r07bsd' '--daemon-socket' '/tmp/d.sock' "
      + "'--app-socket' '/tmp/p1.sock'")
}

@Test("a remote pane's bridge says so, and dials the near end of the forward")
func aRemotePaneSaysSo() {
  // The one difference between a local pane and a devenv one, end to end. The socket is the
  // local end of the ssh forward the core opened, so it is dialled the same way; what changes
  // is the window of unacknowledged output the stream asks for, which is sized for the link.
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "p1w3r07bsd",
    daemonSocket: "/tmp/muster-devenv.sock", linkSocketPath: "/tmp/p1.sock", remote: true)

  #expect(
    command
      == "'/build/debug/muster-bridge' 'p1w3r07bsd' '--daemon-socket' "
      + "'/tmp/muster-devenv.sock' '--app-socket' '/tmp/p1.sock' '--remote'")
}

@Test("a replacement bridge takes the pane over, and a first one does not")
func onlyAReattachTakesOver() {
  // A bridge whose connection died can go on holding its pane, so a replacement that could not
  // displace it would leave the pane locked after every network change. A first bridge never
  // takes over: the pane could be one another window is legitimately showing.
  let first = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "p1", daemonSocket: "/tmp/d.sock",
    linkSocketPath: "/tmp/p1.sock")
  let again = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "p1", daemonSocket: "/tmp/d.sock",
    linkSocketPath: "/tmp/p1.sock", remote: true, reattaching: true)

  #expect(!first.contains("--takeover"))
  #expect(again.hasSuffix("'--remote' '--takeover'"))
}

@Test("an argument with a space in it stays one argument")
func spacesSurviveTheCommandLine() {
  // Everything here reaches libghostty as one string and is split on spaces on the way to a
  // process. A path with a space in it would become two arguments, and the pane would render
  // nothing for a reason no log line would name.
  let command = PaneCommand.bridge(
    executable: "/Users/some one/build/muster", paneID: "p1", daemonSocket: "/tmp/d.sock",
    linkSocketPath: "/tmp/a b.sock")

  #expect(command.contains("'/Users/some one/build/muster-bridge'"))
  #expect(command.contains("'/tmp/a b.sock'"))
}

@Test("a quote in an argument cannot end it early")
func quotesSurviveTheCommandLine() {
  let command = PaneCommand.bridge(
    executable: "/build/debug/muster", paneID: "p1", daemonSocket: "/tmp/it's.sock",
    linkSocketPath: "/tmp/p1.sock")

  #expect(command.contains("'/tmp/it'\\''s.sock'"))
}
