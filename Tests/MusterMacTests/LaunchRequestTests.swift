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
