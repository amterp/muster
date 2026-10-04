import AppKit
import Testing

@testable import MusterMac

// libghostty stops drawing a surface only when told its window cannot be seen, and stops the
// focused one's display link only when told it has lost focus. The app holds App Nap off for its
// whole life, so nothing else slows a hidden window's panes: each surface has to be told.

@Suite("a window's visibility and focus reach its surfaces")
struct SurfaceVisibilityTests {
  @MainActor
  @Test("every surface hears that its window cannot be seen, including one made while it could not")
  func occlusionReachesEverySurface() {
    let store = paneSurfaces()
    let first = borrowed("p1", from: store).surface
    store.window(visible: false)
    #expect(first.occlusions == [false])

    let later = borrowed("p2", from: store).surface
    #expect(later.occlusions == [false], "a pane made in a hidden window would draw unseen")

    store.window(visible: true)
    #expect(first.occlusions == [false, true])
    #expect(later.occlusions == [false, true])
  }

  /// A pane in a tab no region shows is parked in a hidden view of a window that may be visible,
  /// so it is told it cannot be seen until a region shows it again - and a hidden window's pane
  /// shown again stays hidden until the window is seen.
  @MainActor
  @Test("a parked pane stops drawing, and draws again once a region shows it")
  func parkingOccludes() {
    let store = paneSurfaces()
    let (_, surface) = borrowed("p1", from: store)
    store.park(everythingBut: [])
    #expect(surface.occlusions == [false], "a parked pane went on drawing in a visible window")

    _ = borrowed("p1", from: store, recording: false)
    #expect(surface.occlusions == [false, true], "a pane shown again stayed hidden")

    store.window(visible: false)
    store.park(everythingBut: [])
    _ = borrowed("p1", from: store, recording: false)
    #expect(
      surface.occlusions == [false, true, false],
      "a pane parked and shown again in a hidden window was told it could be seen")
  }

  @MainActor
  @Test("the pane with the keyboard loses focus while its window is not key, and gets it back")
  func focusFollowsTheWindow() {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 200, height: 100), styleMask: [.titled],
      backing: .buffered, defer: true)
    let store = paneSurfaces()
    let (chrome, surface) = borrowed("p1", from: store)
    window.contentView?.addSubview(chrome)
    window.makeFirstResponder(chrome.surface)

    store.window(key: true)
    #expect(surface.focuses.last == true)
    store.window(key: false)
    #expect(surface.focuses.last == false, "a focused surface keeps its display link firing")
    store.window(key: true)
    #expect(surface.focuses.last == true)
  }
}

/// A pane's chrome from `store`, drawn by a surface that records what it was told. Borrowed
/// again without `recording`, it keeps the surface it already has.
@MainActor
@discardableResult
private func borrowed(
  _ pane: String, from store: PaneSurfaces, recording: Bool = true
) -> (chrome: PaneChrome, surface: RecordingSurface) {
  let (chrome, _) = store.borrow(
    daemonID: "local", daemonSocket: "/tmp/local.sock",
    leaf: .init(paneID: pane, linkSocketPath: "/tmp/link.sock", bridgeRestarts: 0),
    focus: { _ in }, pointer: { _, _ in })
  let surface = RecordingSurface()
  if recording { chrome.surface.attach(surface, typeable: true) }
  return (chrome, surface)
}
