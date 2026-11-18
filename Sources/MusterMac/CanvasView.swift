import AppKit
import MusterRenderer

struct CanvasPolicy: Equatable, Sendable {
  let loadSchemes: [String]
  let scriptsEnabled: Bool
  let externalLinkSchemes: [String]
}

public struct CanvasInfo: Equatable, Sendable {
  public enum Kind: Equatable, Sendable {
    case markdown
    case html
    case image
  }

  public let revision: UInt64
  public let kind: Kind?
  public let title: String
}

struct CanvasPage: Sendable {
  typealias Kind = CanvasInfo.Kind

  let revision: UInt64
  let kind: Kind
  let title: String?
  let html: String
  let policy: CanvasPolicy
}

struct CanvasDocumentVersion {
  let revision: UInt64
  let generation: UInt64
}

protocol CanvasPageSource: Sendable {
  @MainActor
  func observeRevisions(
    for pane: PaneKey, receive: @escaping (UInt64, Bool) -> Void
  ) -> @MainActor () -> Void

  func page(for pane: PaneKey, revision: UInt64) async throws -> CanvasPage
}

enum CanvasScroll: String {
  case top
  case bottom
  case pageUp
  case pageDown
}

struct CanvasAppearance: Equatable {
  let background: RGB
  let foreground: RGB
  let accent: RGB
  let fontFamily: String?
  let fontSize: Double

  @MainActor
  static func resolved(_ appearance: NSAppearance, terminal: MusterRenderer.Appearance)
    -> CanvasAppearance
  {
    let colors = ChromePalette.current(for: appearance).pane
    return CanvasAppearance(
      background: colors.background, foreground: colors.text, accent: colors.accentText,
      fontFamily: terminal.fontFamily, fontSize: Double(terminal.fontSize ?? 13))
  }

  var css: [String: String] {
    [
      "--muster-background": background.hex,
      "--muster-foreground": foreground.hex,
      "--muster-accent": accent.hex,
      "--muster-font": "system-ui",
      "--muster-mono": fontFamily.map {
        let escaped = $0.replacingOccurrences(of: "\\", with: "\\\\")
          .replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\", monospace"
      } ?? "monospace",
    ]
  }
}

@MainActor
protocol CanvasBrowser: AnyObject {
  var view: NSView { get }
  var focusTarget: NSView? { get }
  var onLink: ((UInt64, URL) -> Void)? { get set }
  var onBlockedNavigation: ((URL?) -> Void)? { get set }
  var onContentLost: ((CanvasDocumentVersion) -> Void)? { get set }

  func prepare(_ policy: CanvasPolicy) async throws
  func load(_ page: CanvasPage, generation: UInt64) async throws
  func scrollPosition() async -> NSPoint?
  func update(appearance: CanvasAppearance, scrollTo: NSPoint?)
  func scroll(_ movement: CanvasScroll)
  func selectAll()
  func setZoom(_ zoom: CGFloat)
  func tearDown()
}

@MainActor
final class CanvasView: NSView, PaneContent {
  private struct ProcessRecovery {
    let revision: UInt64
    var loadedAt: ContinuousClock.Instant?
    var exhausted = false
  }

  let pane: PaneKey
  private let source: any CanvasPageSource
  private let browser: any CanvasBrowser
  private let now: () -> ContinuousClock.Instant
  private let openLink: (URL) -> Void
  private let errorLabel = NSTextField(wrappingLabelWithString: "")
  private var stopObserving: (@MainActor () -> Void)?
  private var work: Task<Void, Never>?
  private var latest: UInt64?
  private var generation: UInt64 = 0
  private var processRecovery: ProcessRecovery?
  private var displayed: (revision: UInt64, kind: CanvasPage.Kind, policy: CanvasPolicy)?
  private var metadata: CanvasInfo?
  private var terminalAppearance: MusterRenderer.Appearance
  private var style: CanvasAppearance
  private var fontOffset: Int32 = 0
  private var closed = false

  var view: NSView { self }
  var onClick: (@MainActor () -> Void)?
  var onMenu: (@MainActor () -> NSMenu?)?
  var onTitleChanged: ((String) -> Void)?
  var sendKey: ((NSEvent) -> Bool)?
  private(set) var title: String

  init(
    pane: PaneKey, revision: UInt64, title: String, source: any CanvasPageSource,
    terminalAppearance: MusterRenderer.Appearance, browser: any CanvasBrowser = WKCanvasBrowser(),
    now: @escaping () -> ContinuousClock.Instant = { .now },
    openLink: @escaping (URL) -> Void = { NSWorkspace.shared.open($0) }
  ) {
    self.pane = pane
    self.source = source
    self.browser = browser
    self.now = now
    self.title = title
    self.terminalAppearance = terminalAppearance
    self.style = .resolved(
      NSApp?.effectiveAppearance ?? .currentDrawing(), terminal: terminalAppearance)
    self.openLink = openLink
    super.init(frame: .zero)
    addSubview(browser.view)
    errorLabel.alignment = .center
    errorLabel.isHidden = true
    addSubview(errorLabel)

    browser.onLink = { [weak self] revision, url in self?.activated(url, revision: revision) }
    browser.onBlockedNavigation = { [weak self] url in self?.blocked(url) }
    browser.onContentLost = { [weak self] lost in self?.recoverWebContent(lost) }

    stopObserving = source.observeRevisions(for: pane) { [weak self] revision, reset in
      self?.request(revision, reset: reset)
    }
    request(revision)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override var acceptsFirstResponder: Bool { true }

  override func layout() {
    super.layout()
    browser.view.frame = bounds
    errorLabel.frame = bounds.insetBy(dx: min(16, bounds.width / 4), dy: 0)
  }

  func contains(_ view: NSView?) -> Bool {
    guard let view else { return false }
    return view === self || view.isDescendant(of: self)
  }

  func focus(in window: NSWindow) {
    let current = window.firstResponder as? NSView
    if !browser.view.isHidden, let target = browser.focusTarget {
      if current !== target, current?.isDescendant(of: target) != true {
        window.makeFirstResponder(target)
      }
    } else if current !== self {
      window.makeFirstResponder(self)
    }
  }

  func apply(windowIsKey: Bool) {}
  func apply(windowIsVisible: Bool) {}
  func apply(parked: Bool) {}
  func apply(covered: Bool) {}
  func matchBackingScale() {}

  func setFontSizeOffset(_ points: Int32) -> [String] {
    fontOffset = points
    updateZoom()
    return []
  }

  func perform(_ action: SurfaceAction) {
    guard displayed != nil, errorLabel.isHidden else { return }
    switch action {
    case .selectAll: browser.selectAll()
    case .scrollToTop: browser.scroll(.top)
    case .scrollToBottom: browser.scroll(.bottom)
    case .scrollPageUp: browser.scroll(.pageUp)
    case .scrollPageDown: browser.scroll(.pageDown)
    case .jumpToPreviousPrompt, .jumpToNextPrompt: break
    }
  }

  func apply(terminalAppearance: MusterRenderer.Appearance) {
    self.terminalAppearance = terminalAppearance
    adoptAppearance()
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    adoptAppearance()
  }

  private func adoptAppearance() {
    let resolved = CanvasAppearance.resolved(effectiveAppearance, terminal: terminalAppearance)
    guard style != resolved else { return }
    style = resolved
    browser.update(appearance: style, scrollTo: nil)
    updateZoom()
  }

  private func updateZoom() {
    let base = max(1, style.fontSize)
    browser.setZoom(max(1, base + Double(fontOffset)) / base)
  }

  func tearDown() {
    guard !closed else { return }
    closed = true
    stopObserving?()
    stopObserving = nil
    work?.cancel()
    work = nil
    displayed = nil
    browser.tearDown()
  }

  func request(_ revision: UInt64, reset: Bool = false) {
    guard !closed else { return }
    if reset {
      processRecovery = nil
      invalidateDocument()
    } else if let latest, revision <= latest {
      return
    }
    latest = revision
    fetchIfIdle()
  }

  private func recoverWebContent(_ lost: CanvasDocumentVersion) {
    guard !closed, lost.generation == generation, let revision = latest else { return }
    invalidateDocument()
    if lost.revision == revision {
      if let recovery = processRecovery, recovery.revision == revision,
        recovery.loadedAt.map({ $0.duration(to: now()) < .seconds(30) }) ?? true
      {
        processRecovery?.exhausted = true
        failed("The canvas web content stopped.")
        return
      }
      processRecovery = ProcessRecovery(revision: revision)
    }
    errorLabel.isHidden = true
    fetchIfIdle()
  }

  private func invalidateDocument() {
    generation &+= 1
    displayed = nil
    hideBrowser()
  }

  private func fetchIfIdle() {
    guard !closed, work == nil, let revision = latest else { return }
    if let processRecovery, processRecovery.revision == revision, processRecovery.exhausted {
      return
    }
    let source = self.source
    let pane = self.pane
    let generation = self.generation
    let started = ContinuousClock.now
    let fetch = Task.detached { try await source.page(for: pane, revision: revision) }
    work = Task { [weak self] in
      let result = await withTaskCancellationHandler {
        await fetch.result
      } onCancel: {
        fetch.cancel()
      }
      guard let self, !Task.isCancelled else { return }
      await self.render(result, requested: revision, generation: generation, started: started)
    }
  }

  private func render(
    _ result: Result<CanvasPage, any Error>, requested: UInt64, generation: UInt64,
    started: ContinuousClock.Instant
  ) async {
    var handled = requested
    defer {
      work = nil
      if self.generation != generation || (latest.map { $0 > handled } ?? false) { fetchIfIdle() }
    }
    guard self.generation == generation else { return }
    guard case .success(let page) = result, page.revision >= requested else {
      if latest == requested { failed("Could not read the canvas page.") }
      return
    }
    guard let latest, page.revision >= latest else { return }
    self.latest = page.revision
    handled = page.revision

    let scroll = displayed?.kind == page.kind ? await browser.scrollPosition() : nil
    guard isLatest(page.revision, generation: generation) else { return }
    do {
      try await browser.prepare(page.policy)
    } catch {
      if isLatest(page.revision, generation: generation) {
        failed("Could not install the canvas content rules.")
      }
      return
    }
    guard isLatest(page.revision, generation: generation) else { return }
    updateZoom()
    do {
      try await browser.load(page, generation: generation)
    } catch {
      if isLatest(page.revision, generation: generation) {
        failed("Could not display the canvas page.")
      }
      return
    }
    guard !closed, self.generation == generation else { return }

    if processRecovery?.revision == page.revision {
      // WebKit can finish navigation before it decodes or paints everything in the page.
      processRecovery?.loadedAt = now()
    } else {
      processRecovery = nil
    }
    // Keep track of what WebKit displayed even if another revision arrived during navigation.
    displayed = (page.revision, page.kind, page.policy)
    browser.update(appearance: style, scrollTo: scroll)
    errorLabel.isHidden = true
    browser.view.isHidden = false
    if let window, window.firstResponder === self { focus(in: window) }
    if let metadata, metadata.revision <= page.revision {
      updateTitle(metadata.title)
    } else if let title = page.title {
      updateTitle(title)
    }
    guard isLatest(page.revision, generation: generation) else { return }
    let duration = started.duration(to: .now).components
    Core.info(
      "canvas.loaded",
      [
        "pane": pane.pane,
        "revision": String(page.revision),
        "bytes": String(page.html.utf8.count),
        "ms": String(duration.seconds * 1000 + duration.attoseconds / 1_000_000_000_000_000),
      ])
  }

  private func isLatest(_ revision: UInt64, generation: UInt64) -> Bool {
    !closed && self.generation == generation && latest == revision
  }

  func apply(canvas info: CanvasInfo) {
    let restored = metadata.map { info.revision < $0.revision } ?? false
    metadata = info
    guard let displayed else { return }
    if displayed.revision == info.revision || (!restored && displayed.revision > info.revision) {
      updateTitle(info.title)
    }
  }

  private func updateTitle(_ title: String) {
    guard self.title != title else { return }
    self.title = title
    onTitleChanged?(title)
  }

  private func activated(_ url: URL, revision: UInt64) {
    guard !closed, errorLabel.isHidden, let displayed, displayed.revision == revision,
      let scheme = url.scheme?.lowercased(),
      displayed.policy.externalLinkSchemes.contains(where: { $0.lowercased() == scheme })
    else {
      blocked(url)
      return
    }
    openLink(url)
  }

  private func blocked(_ url: URL?) {
    Core.info(
      "canvas.blocked",
      [
        "pane": pane.pane, "scheme": url?.scheme ?? "", "host": url?.host ?? "",
      ])
  }

  private func failed(_ reason: String) {
    guard !closed else { return }
    hideBrowser()
    errorLabel.stringValue = reason
    errorLabel.isHidden = false
    Core.warn("canvas.failed", ["pane": pane.pane, "reason": reason])
  }

  private func hideBrowser() {
    if let window, let current = window.firstResponder as? NSView,
      current !== self, contains(current)
    {
      window.makeFirstResponder(self)
    }
    browser.view.isHidden = true
  }
}
