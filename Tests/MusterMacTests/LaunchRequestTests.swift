import Testing

@testable import MusterMac

// What a command line means. The bare case is the one that matters: it is what double-clicking
// an app sends, and it used to mean a window that renders and drops every keystroke.

@Test("no arguments opens the window, rather than checking the renderer")
func bareLaunchOpens() {
  #expect(launchRequest(arguments: []) == .open)
}

@Test("a pane id starts the keyboard there")
func aPaneIsNamed() {
  #expect(launchRequest(arguments: ["w1:p1"]) == .pane("w1:p1"))
}

@Test("the renderer check is still reachable, behind its flag")
func theRendererCheckHasAFlag() {
  // The only thing that separates "the renderer works" from "the daemon works". Losing it
  // would mean debugging the two together forever.
  #expect(launchRequest(arguments: ["--renderer-check"]) == .rendererCheck)
}

@Test("a flag Muster does not read is refused, not ignored")
func anUnknownFlagIsRefused() {
  // A mistyped flag that opened an ordinary window would look exactly like one that worked.
  #expect(launchRequest(arguments: ["--render-check"]) == .unknown("--render-check"))
}

@Test("a window somebody asked for says so, and still opens")
func aFreshWindowIsStillAnOrdinaryLaunch() {
  // Both options describe how the launch was arranged rather than what it should do, so a
  // window opened by `muster window new` still means "show whatever the daemons hold". A
  // `--fresh` that reached the request would read as a flag Muster does not know, and the
  // window would refuse to open at all.
  #expect(launchIsFresh(arguments: ["--fresh", "--home", "/tmp/somewhere"]))
  #expect(launchRequest(arguments: ["--fresh", "--home", "/tmp/somewhere"]) == .open)
  #expect(launchHome(arguments: ["--fresh", "--home", "/tmp/somewhere"]) == "/tmp/somewhere")
}

@Test("a launch from the Dock is not one somebody asked for")
func anOrdinaryLaunchIsNotFresh() {
  // The difference decides where the window starts, so the default has to be the launch
  // nobody flagged: that one comes back onto the tabs it was left on.
  #expect(!launchIsFresh(arguments: []))
  #expect(!launchIsFresh(arguments: ["--home", "/tmp/somewhere"]))
}
