import AppKit
import MusterRenderer
import Testing

@testable import MusterMac

private let canvasPane = PaneKey(daemon: "local", pane: "canvas")
private let canvasPolicy = CanvasPolicy(
  loadSchemes: ["data", "blob", "about"], scriptsEnabled: false,
  externalLinkSchemes: ["http", "https", "mailto"])

private func page(_ revision: UInt64, kind: CanvasPage.Kind = .markdown) -> CanvasPage {
  CanvasPage(
    revision: revision, kind: kind, title: "Page \(revision)", html: "<p>Revision \(revision)</p>",
    policy: canvasPolicy)
}

@Suite("canvas view", .ownsTheSeam)
@MainActor
struct CanvasViewTests {
  @Test("a hidden canvas keeps its view and loads revisions without starting a terminal")
  func paneLifetime() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    let canvas = makeCanvas(source, browser)
    let window = KeyboardWindow(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.titled],
      backing: .buffered, defer: true)
    let container = try #require(window.contentView)
    let store = PaneSurfaces(
      parkedIn: container, makeContent: { _ in canvas },
      startPane: { _, _, _ in Issue.record("a canvas asked for a terminal") })
    defer { store.releaseAll() }
    let tab = TabView(frame: container.bounds, surfaces: store)
    container.addSubview(tab)
    let contents = WindowContents(
      tab: "t1", tree: .pane(.init(paneID: canvasPane.pane, linkSocketPath: nil)),
      keyboard: canvasPane)
    tab.apply(contents)
    let chrome = try #require(tab.chrome(for: canvasPane))
    #expect(chrome.header.shownIcon == "canvas")
    #expect(chrome.header.shownName == "Opening")
    #expect(window.firstResponder === canvas)

    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("the initial title") { chrome.header.shownName == "Page 1" }
    #expect(window.firstResponder === browser.view)

    store.park(everythingBut: [])
    tab.apply(.init(tab: nil, tree: nil, keyboard: nil))
    let hidden = Roster.Tab(
      id: "t1", place: 1, label: "Hidden", onScreen: false,
      panes: [
        .init(
          key: canvasPane, label: "Canvas", onScreen: false,
          canvas: CanvasInfo(revision: 2, kind: .markdown, title: "Page 2"))
      ])
    store.apply(Roster(tabs: [], hidden: [hidden]))
    #expect(store.parked == [canvasPane])
    source.notify(2)
    await until("a page request while parked") { source.requested == [1, 2] }
    try source.answer(
      2, with: CanvasPage(revision: 2, kind: .markdown, title: nil, html: "", policy: canvasPolicy))
    await until("the parked title") { chrome.header.shownName == "Page 2" }

    tab.apply(contents)
    #expect(tab.chrome(for: canvasPane) === chrome)
    #expect(window.firstResponder === browser.view)
    #expect(browser.loads.map(\.revision) == [1, 2])

    store.park(everythingBut: [])
    tab.apply(.init(tab: nil, tree: nil, keyboard: nil))
    store.apply(Roster(tabs: []))
    #expect(store.chrome(for: canvasPane) == nil)
    #expect(browser.teardowns == 1)
    #expect(browser.view.window == nil)
  }

  @Test("only the latest revision reaches the browser while a page fetch is in flight")
  func coalescesPageFetches() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }

    await until("the initial page request") { source.requested == [1] }
    canvas.request(2)
    canvas.request(3)
    #expect(source.requested == [1])

    try source.answer(1, with: page(1))
    await until("the latest page request") { source.requested == [1, 3] }
    #expect(browser.loads.isEmpty)
    try source.answer(3, with: page(3))
    await until("the latest title") { canvas.title == "Page 3" }
    #expect(browser.loads.map(\.revision) == [3])
  }

  @Test("an attachment reset discards the old reply and keeps one fetch in flight")
  func restoresOlderRevision() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }

    await until("the initial page request") { source.requested == [1] }
    canvas.apply(canvas: CanvasInfo(revision: 1, kind: .markdown, title: "Page 1"))
    try source.answer(1, with: page(1))
    await until("the initial page") { canvas.title == "Page 1" }

    source.notify(4)
    await until("the request on the old attachment") { source.requested == [1, 4] }
    canvas.apply(canvas: CanvasInfo(revision: 4, kind: .markdown, title: "Old attachment"))
    source.notify(2, reset: true)
    #expect(browser.view.isHidden)
    #expect(source.requested == [1, 4])
    try source.answer(4, with: page(4))
    await until("the reset request") { source.requested == [1, 4, 2] }
    #expect(browser.loads.map(\.revision) == [1])
    try source.answer(
      2, with: CanvasPage(revision: 2, kind: .markdown, title: nil, html: "", policy: canvasPolicy))
    await until("the restored page") { !browser.view.isHidden }
    #expect(canvas.title == "Page 1")
    #expect(browser.loads.map(\.revision) == [1, 2])

    var restored = Muster_RosterPane()
    restored.canvas.revision = 2
    restored.canvas.kind = .markdown
    restored.canvas.title = "Restored"
    let info = try #require(Roster.Pane(restored).canvas)
    canvas.apply(canvas: info)
    #expect(canvas.title == "Restored")
  }

  @Test("a leaf keeps canvas metadata even when its kind is newer than this shell")
  func recognizesCanvasLeaves() throws {
    var pane = Muster_ViewPane()
    pane.paneID = canvasPane.pane
    var info = Muster_CanvasInfo()
    info.revision = 12
    info.kind = .UNRECOGNIZED(42)
    info.title = "Canvas title"
    pane.canvas = info
    var branch = Muster_ViewTreePane()
    branch.daemonID = "remote"
    branch.pane = pane
    var tree = Muster_ViewTree()
    tree.pane = branch
    let leaf = try #require(PaneTree(tree).leaves.first)
    #expect(leaf.key == PaneKey(daemon: "remote", pane: canvasPane.pane))
    #expect(leaf.canvas == CanvasInfo(revision: 12, kind: nil, title: "Canvas title"))
  }

  @Test("the live page source addresses the pane and uses the returned policy and revision")
  func decodesARealSeamRequest() async throws {
    let dispatcher = RecordingDispatcher { request in
      guard case .canvasPage = request.payload else { return nil }
      #expect(!Thread.isMainThread)
      var reply = Muster_CanvasRendered()
      reply.revision = 5
      reply.kind = .image
      reply.html = "<p>From the canvas renderer</p>"
      reply.loadSchemes = ["data", "about"]
      reply.externalLinkSchemes = ["https"]
      return .canvasRendered(reply)
    }
    let source = CoreCanvasPageSource(window: "window-2", dispatcher: dispatcher)
    let browser = TestCanvasBrowser()
    let remote = PaneKey(daemon: "remote", pane: "canvas")
    let canvas = CanvasView(
      pane: remote, revision: 3, title: "Opening", source: source,
      terminalAppearance: Appearance(), browser: browser)
    defer { canvas.tearDown() }
    await until("the core's page") { browser.loads.count == 1 }
    let request = try #require(dispatcher.requests.first)
    #expect(request.window == "window-2")
    #expect(request.canvasPage.daemonID == remote.daemon)
    #expect(request.canvasPage.paneID == remote.pane)
    let rendered = try #require(browser.loads.first)
    #expect(rendered.revision == 5)
    #expect(rendered.kind == .image)
    #expect(rendered.html == "<p>From the canvas renderer</p>")
    #expect(
      rendered.policy
        == CanvasPolicy(
          loadSchemes: ["data", "about"], scriptsEnabled: false, externalLinkSchemes: ["https"]))
  }

  @Test("a canvas key carries its pane across both press and release")
  func addressesCanvasKeys() throws {
    let dispatcher = seam(
      RecordingDispatcher { request in
        var handled = Muster_CanvasKeyHandled()
        if case .keyUp = request.payload { handled.toView = true }
        return .canvasKeyHandled(handled)
      })
    let down = try #require(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
        context: nil, characters: " ", charactersIgnoringModifiers: " ", isARepeat: false,
        keyCode: 49))
    let up = try #require(
      NSEvent.keyEvent(
        with: .keyUp, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
        context: nil, characters: " ", charactersIgnoringModifiers: " ", isARepeat: false,
        keyCode: 49))
    Core.speaking(for: "window-2") {
      #expect(!Core.sendCanvasKey(down, to: canvasPane))
      #expect(Core.sendCanvasKey(up, to: canvasPane))
    }

    let requests = dispatcher.requests
    #expect(requests.map(\.window) == ["window-2", "window-2"])
    let pressed = try #require(requests.first?.keyDown)
    let released = try #require(requests.last?.keyUp)
    #expect(pressed.canvasTarget.daemonID == canvasPane.daemon)
    #expect(pressed.canvasTarget.paneID == canvasPane.pane)
    #expect(released.canvasTarget == pressed.canvasTarget)
    #expect(pressed.key.action == "press")
    #expect(released.key.action == "release")
  }

  @Test("the browser sees only the latest HTML when a revision arrives during rule installation")
  func supersedesPendingRuleInstall() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    browser.holdRules = true
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }

    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("the rule compilation") { browser.isInstallingRules }
    #expect(browser.loads.isEmpty)
    canvas.request(2)

    try browser.finishRules()
    await until("the latest page request") { source.requested == [1, 2] }
    #expect(browser.loads.isEmpty)
    #expect(browser.installed == nil)
    try source.answer(2, with: page(2))
    await until("the latest title") { canvas.title == "Page 2" }
    #expect(browser.loads.map(\.revision) == [2])
  }

  @Test("a content rule failure shows an error before WebKit sees any HTML")
  func ruleFailureDoesNotLoadPage() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    browser.holdRules = true
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }

    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("the rule compilation") { browser.isInstallingRules }
    try browser.finishRules(error: TestCanvasError.failed)
    await until("an error in place of the browser") { browser.view.isHidden }

    #expect(browser.loads.isEmpty)
    #expect(canvas.title == "Opening")
    let message = canvas.subviews.compactMap { $0 as? NSTextField }.first
    #expect(message?.stringValue == "Could not install the canvas content rules.")
  }

  @Test("the title follows the actual page and focus survives an error and recovery")
  func showsCompletedNavigationAndRecovers() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }
    let bounds = NSRect(x: 0, y: 0, width: 400, height: 300)
    let window = NSWindow(
      contentRect: bounds, styleMask: [.titled], backing: .buffered, defer: true)
    window.contentView?.addSubview(canvas)
    canvas.focus(in: window)

    browser.holdNavigation = true
    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("a pending navigation") { browser.isLoading }
    canvas.request(2)
    canvas.request(3)
    try browser.finishNavigation()
    await until("the title of the actual page") { canvas.title == "Page 1" }
    await until("the newest page request") { source.requested == [1, 3] }
    #expect(window.firstResponder === browser.view)

    browser.holdRules = true
    try source.answer(3, with: page(3))
    await until("the rule compilation") { browser.isInstallingRules }
    try browser.finishRules(error: TestCanvasError.failed)
    await until("an error in place of the browser") { browser.view.isHidden }
    #expect(window.firstResponder === canvas)

    canvas.request(4)
    await until("the recovery request") { source.requested == [1, 3, 4] }
    try source.answer(4, with: page(4))
    await until("the recovered page") { canvas.title == "Page 4" }
    #expect(!browser.view.isHidden)
    #expect(window.firstResponder === browser.view)
  }

  @Test("external opening follows the displayed page's revision and link policy")
  func approvedLinksOpenOutside() async throws {
    let recorder = recorder()
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    var opened: [URL] = []
    let canvas = makeCanvas(source, browser, open: { opened.append($0) })
    defer { canvas.tearDown() }
    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("the displayed page") { canvas.title == "Page 1" }

    let allowed = try #require(URL(string: "https://example.com/allowed"))
    let other = try #require(URL(string: "other://example.com/private?token=redact"))
    browser.onLink?(0, allowed)
    browser.onLink?(1, other)
    browser.onLink?(1, allowed)
    #expect(opened == [allowed])

    let blocked = recorder.requests.map(\.logRecord).filter { $0.event == "canvas.blocked" }
    #expect(
      blocked.last?.fields == [
        "pane": canvasPane.pane, "scheme": "other", "host": "example.com",
      ])
  }

  @Test("same-kind writes keep scroll, and appearance changes update the displayed page")
  func keepsScrollAndAppliesStyle() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }
    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("the displayed page") { canvas.title == "Page 1" }

    browser.scroll = NSPoint(x: 3, y: 100)
    canvas.request(2)
    await until("the second page request") { source.requested == [1, 2] }
    try source.answer(2, with: page(2))
    await until("the second title") { canvas.title == "Page 2" }
    #expect(browser.updates.last?.scroll == NSPoint(x: 3, y: 100))

    canvas.request(3)
    await until("the third page request") { source.requested == [1, 2, 3] }
    try source.answer(3, with: page(3, kind: .image))
    await until("the third title") { canvas.title == "Page 3" }
    #expect(browser.updates.last?.scroll == nil)

    canvas.apply(terminalAppearance: Appearance(fontFamily: "Menlo", fontSize: 10))
    _ = canvas.setFontSizeOffset(2)
    #expect(browser.updates.last?.style.css["--muster-mono"] == "\"Menlo\", monospace")
    #expect(browser.zoom == 1.2)
    #expect(browser.loads.map(\.revision) == [1, 2, 3])
  }

  @Test("window key routing follows the canvas's actual first responder")
  func keysFollowAppKitFocus() async throws {
    let source = TestCanvasSource()
    let browser = TestCanvasBrowser()
    let canvas = makeCanvas(source, browser)
    defer { canvas.tearDown() }
    let bounds = NSRect(x: 0, y: 0, width: 400, height: 300)
    let window = KeyboardWindow(
      contentRect: bounds, styleMask: [.titled], backing: .buffered, defer: true)
    let container = try #require(window.contentView)
    canvas.frame = bounds
    container.addSubview(canvas)
    canvas.focus(in: window)
    #expect(window.firstResponder === canvas)

    await until("the initial page request") { source.requested == [1] }
    try source.answer(1, with: page(1))
    await until("the displayed page") { canvas.title == "Page 1" }
    #expect(window.firstResponder === browser.view)

    let event = try #require(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: window.windowNumber, context: nil, characters: " ",
        charactersIgnoringModifiers: " ",
        isARepeat: false, keyCode: 49))
    var offered = 0
    canvas.sendKey = { _ in
      offered += 1
      return false
    }
    window.sendEvent(event)
    #expect(offered == 1)
    #expect(browser.keys == 0)

    canvas.sendKey = { _ in
      offered += 1
      return true
    }
    window.sendEvent(event)
    #expect(offered == 2)
    #expect(browser.keys == 1)

    let other = CanvasTestResponder(frame: .zero)
    container.addSubview(other)
    window.makeFirstResponder(other)
    window.sendEvent(event)
    #expect(offered == 2)

    var clicks = 0
    canvas.onClick = { clicks += 1 }
    let click = try #require(
      NSEvent.mouseEvent(
        with: .leftMouseDown, location: NSPoint(x: 100, y: 100), modifierFlags: [],
        timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 1,
        clickCount: 1, pressure: 1))
    window.sendEvent(click)
    #expect(clicks == 1)

    canvas.perform(.selectAll)
    canvas.perform(.scrollPageDown)
    canvas.perform(.jumpToNextPrompt)
    #expect(browser.selections == 1)
    #expect(browser.movements == [.pageDown])
  }

  @Test("the scheme rules block by default and encode only exact exceptions")
  func ruleTranslation() throws {
    let text = try WKCanvasBrowser.ruleJSON(allowing: ["data", "BLOB", "about", "data"])
    let json = try JSONSerialization.jsonObject(with: Data(text.utf8))
    let rules = try #require(json as? [[String: [String: String]]])
    #expect(rules.first?["action"]?["type"] == "block")
    #expect(
      rules.dropFirst().map { $0["trigger"]?["url-filter"] } == ["^about:", "^blob:", "^data:"])
    #expect(throws: (any Error).self) {
      try WKCanvasBrowser.ruleJSON(allowing: ["*"])
    }
  }

  private func makeCanvas(
    _ source: TestCanvasSource, _ browser: TestCanvasBrowser,
    open: @escaping (URL) -> Void = { _ in }
  ) -> CanvasView {
    CanvasView(
      pane: canvasPane, revision: 1, title: "Opening", source: source,
      terminalAppearance: Appearance(), browser: browser, openLink: open)
  }
}

private enum TestCanvasError: Error {
  case failed
}

private final class TestCanvasSource: CanvasPageSource, @unchecked Sendable {
  @MainActor private var receiver: ((UInt64, Bool) -> Void)?
  private let lock = NSLock()
  private var requests: [UInt64] = []
  private var replies: [UInt64: CheckedContinuation<CanvasPage, any Error>] = [:]

  @MainActor init() {}

  var requested: [UInt64] { lock.withLock { requests } }

  @MainActor
  func observeRevisions(
    for pane: PaneKey, receive: @escaping (UInt64, Bool) -> Void
  ) -> @MainActor () -> Void {
    receiver = receive
    return { [weak self] in self?.receiver = nil }
  }

  func page(for pane: PaneKey, revision: UInt64) async throws -> CanvasPage {
    try await withTaskCancellationHandler {
      try await withCheckedThrowingContinuation { continuation in
        lock.withLock {
          if Task.isCancelled {
            continuation.resume(throwing: CancellationError())
          } else {
            requests.append(revision)
            replies[revision] = continuation
          }
        }
      }
    } onCancel: {
      let reply = lock.withLock { replies.removeValue(forKey: revision) }
      reply?.resume(throwing: CancellationError())
    }
  }

  func answer(_ requested: UInt64, with page: CanvasPage) throws {
    let reply = lock.withLock { replies.removeValue(forKey: requested) }
    try #require(reply).resume(returning: page)
  }

  @MainActor
  func notify(_ revision: UInt64, reset: Bool = false) { receiver?(revision, reset) }
}

@MainActor
private final class CanvasTestResponder: NSView {
  var keys = 0
  override var acceptsFirstResponder: Bool { true }
  override func keyDown(with event: NSEvent) { keys += 1 }
}

@MainActor
private final class TestCanvasBrowser: CanvasBrowser {
  private let responder = CanvasTestResponder(frame: .zero)
  var view: NSView { responder }
  var focusTarget: NSView? { installed == nil ? nil : responder }
  var onLink: ((UInt64, URL) -> Void)?
  var onBlockedNavigation: ((URL?) -> Void)?
  var onContentLost: (() -> Void)?

  var holdRules = false
  private var ruleReply: CheckedContinuation<Void, any Error>?
  private var prepared: CanvasPolicy?
  var isInstallingRules: Bool { ruleReply != nil }
  var installed: CanvasPolicy?
  var holdNavigation = false
  var isLoading: Bool { pendingNavigation != nil }
  private var pendingNavigation: CheckedContinuation<Void, any Error>?
  var loads: [CanvasPage] = []
  var scroll: NSPoint?
  var updates: [(style: CanvasAppearance, scroll: NSPoint?)] = []
  var zoom: CGFloat = 1
  var teardowns = 0
  var movements: [CanvasScroll] = []
  var selections = 0
  var keys: Int { responder.keys }

  func prepare(_ policy: CanvasPolicy) async throws {
    if holdRules {
      holdRules = false
      try await withCheckedThrowingContinuation { (reply: CheckedContinuation<Void, any Error>) in
        ruleReply = reply
      }
    }
    prepared = policy
  }

  func finishRules(error: (any Error)? = nil) throws {
    let reply = try #require(ruleReply)
    ruleReply = nil
    if let error {
      reply.resume(throwing: error)
    } else {
      reply.resume()
    }
  }

  func load(_ page: CanvasPage) async throws {
    #expect(installed == page.policy || prepared == page.policy)
    installed = page.policy
    loads.append(page)
    if holdNavigation {
      holdNavigation = false
      try await withCheckedThrowingContinuation { (reply: CheckedContinuation<Void, any Error>) in
        pendingNavigation = reply
      }
    }
  }

  func finishNavigation() throws {
    let reply = try #require(pendingNavigation)
    pendingNavigation = nil
    reply.resume()
  }

  func scrollPosition() async -> NSPoint? { scroll }

  func update(appearance: CanvasAppearance, scrollTo: NSPoint?) {
    updates.append((appearance, scrollTo))
  }

  func setZoom(_ zoom: CGFloat) { self.zoom = zoom }
  func scroll(_ movement: CanvasScroll) { movements.append(movement) }
  func selectAll() { selections += 1 }

  func tearDown() {
    teardowns += 1
    ruleReply?.resume(throwing: CancellationError())
    ruleReply = nil
    pendingNavigation?.resume(throwing: CancellationError())
    pendingNavigation = nil
  }
}
