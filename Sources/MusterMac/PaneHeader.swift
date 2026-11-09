import AppKit

/// What a pane's header says and when it is there, decided apart from the view that draws it.
///
/// The header (Fencil `pane-headers`) is a line across the top of a pane carrying what the agent
/// list's row for it carries: the chord, the state as a shape, the callsign and name, the state's
/// word and how long it has held, and how full the agent's context is. A split then says which
/// agent is which without the list.
public enum PaneHeaderModel {
  /// A header costs one line of the pane, and nothing else: panes stay flush.
  public static let height: CGFloat = 24

  /// A pane shorter than this draws no header. `PaneTree.minimumPaneSize` is 8 points, and a header
  /// in a pane that short would leave its terminal a negative height; at 60 it leaves three lines.
  public static let minimumPaneHeight: CGFloat = 60

  /// Whether a pane of this height draws a header.
  public static func isShown(paneHeight: CGFloat) -> Bool {
    paneHeight >= minimumPaneHeight
  }

  /// Below these widths the header gives something up, the one that says least first. The chord,
  /// the icon and the callsign never go: they are what the header is for.
  static let directoryWidth: CGFloat = 360
  static let contextWidth: CGFloat = 260

  /// What a header of this width has room for.
  public struct Parts: Equatable {
    public var directory: Bool
    public var context: Bool
  }

  /// A pane's directory is on its header only when the pane is named: an unnamed one's name is its
  /// directory and harness already, and one line cannot afford saying the same thing twice. The
  /// agent list's rows keep both (Fencil `agent-list-rows`).
  public static func parts(width: CGFloat, for row: SidebarModel.Row) -> Parts {
    Parts(
      directory: width >= directoryWidth && !row.namedDirectory.isEmpty,
      context: width >= contextWidth)
  }

  /// How full the agent's context is, when it has said and its machine can be reached.
  public static func contextUsed(of row: SidebarModel.Row) -> Float? {
    for case .context(let used) in SidebarModel.accessories(of: row) { return used }
    return nil
  }

  /// Whether the header is tinted and carries a colored edge: blocked and stuck, as the rows are.
  public static func isTinted(_ row: SidebarModel.Row) -> Bool { row.isTinted }
}

/// The line across the top of a pane, which is also what the pane is dragged by.
///
/// The one place a drag in a pane is Muster's rather than the program's: everywhere below it a drag
/// is a selection or a report to a program that asked for the mouse, and a modifier that turned it
/// into a move would be one more key a program in the pane can no longer have. A press here never
/// reaches the terminal underneath.
///
/// Built once and shown many times, as a row is. Its colors come from the palette's header surface,
/// which is the terminal's own background moved toward its foreground (Fencil `chrome-colors`).
@MainActor
final class PaneHeaderView: NSView, NSDraggingSource {
  /// The pane this drags. Nil for a chrome that has not been told which pane it is.
  var pane: PaneKey?

  /// Called on a press, which asks for the keyboard here as a click on the pane does - so the pane
  /// that was moved is the one being typed into, as Ghostty leaves it.
  var onPressed: (() -> Void)?

  /// Builds the pane's context menu, as a right-click on the terminal does.
  var onMenu: (@MainActor () -> NSMenu?)?

  private let tintLayer = CALayer()
  private let edgeBar = CALayer()
  private let icon = StateIcon()
  private let name = NSTextField(labelWithString: "")
  private let directory = NSTextField(labelWithString: "")
  private let stateWord = NSTextField(labelWithString: "")
  private let grip = NSImageView()
  private var keys: [KeycapView] = []
  private var contextBar: ContextBar?
  private var shown: SidebarModel.Row?
  private var hovering = false
  private(set) var dragging = false

  /// Whether the grip is drawn: while the pointer is over the header, and mid-drag.
  var gripShown: Bool { hovering || dragging }

  /// How many times a row that differed from the last was drawn. What the cost of a state change
  /// is measured in: one header, and none for a republish that changed nothing.
  private(set) var redrawCount = 0

  // What a test reads, which is what somebody would see rather than what was meant.
  var shownRow: SidebarModel.Row? { shown }
  var shownKeys: [Int] { keys.map(\.value) }
  var shownIcon: String? { icon.superlayer == nil ? nil : icon.state }
  var showsTint: Bool { tintLayer.superlayer != nil }
  private(set) var tintColor: NSColor?
  /// What the whole header is painted in, under its tint.
  private(set) var fillColor: NSColor?
  var showsDirectory: Bool { directory.superview != nil && !directory.isHidden }
  var showsContext: Bool { contextBar.map { $0.superview != nil && !$0.isHidden } ?? false }
  var shownName: String { name.stringValue }
  var shownStateWord: String? { stateWord.superview == nil ? nil : stateWord.stringValue }

  override init(frame: NSRect) {
    super.init(frame: frame)
    isHidden = true
    wantsLayer = true
    tintLayer.actions = ["position": NSNull(), "bounds": NSNull()]
    edgeBar.actions = ["position": NSNull(), "bounds": NSNull()]
    for label in [name, directory, stateWord] {
      label.lineBreakMode = .byTruncatingTail
    }
    directory.lineBreakMode = .byTruncatingMiddle
    name.font = .systemFont(ofSize: 12, weight: .regular)
    directory.font = .systemFont(ofSize: 11, weight: .regular)
    grip.image = NSImage(systemSymbolName: "ellipsis", accessibilityDescription: "Move pane")?
      .withSymbolConfiguration(.init(pointSize: 10, weight: .semibold))
    grip.isHidden = true
    addSubview(name)
    addSubview(grip)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// Draws `row`, which is the pane's row as the list would draw it, taking away whatever the row
  /// this last showed had and this one does not.
  func show(_ row: SidebarModel.Row?) {
    guard row != shown else { return }
    shown = row
    redrawCount += 1
    draw()
  }

  private func draw() {
    for layer in [tintLayer, edgeBar, icon] { layer.removeFromSuperlayer() }
    for key in keys { key.removeFromSuperview() }
    keys = []
    contextBar?.removeFromSuperview()
    contextBar = nil
    for label in [directory, stateWord] { label.removeFromSuperview() }
    guard let row = shown else {
      name.stringValue = ""
      toolTip = nil
      fillColor = nil
      layer?.backgroundColor = nil
      needsLayout = true
      return
    }
    let palette = ChromePalette.current(for: effectiveAppearance).header
    // Every header is painted, not only a tinted one: its text was worked out against this color
    // (the terminal's background moved toward its foreground), and on the window's own background
    // a light terminal under a dark appearance, or the reverse, is unreadable.
    fillColor = palette.background.color
    layer?.backgroundColor = palette.background.color.cgColor
    let tinted = PaneHeaderModel.isTinted(row)
    var layers: [CALayer] = []
    tintColor = nil
    if tinted, let tint = palette.tint[row.state] {
      tintColor = tint.color
      tintLayer.backgroundColor = tint.color.cgColor
      layers.append(tintLayer)
    }
    if tinted {
      edgeBar.backgroundColor = PaneAppearance.borderColor(state: row.state).cgColor
      layers.append(edgeBar)
    }
    for (index, layer) in layers.enumerated() {
      self.layer?.insertSublayer(layer, at: UInt32(index))
    }

    if case .pane(let first, let second) = row.kind {
      for press in [first, second] where SidebarRowView.pressable(press) {
        let key = KeycapView(digit: press)
        key.show(digit: press, live: false, surface: palette)
        addSubview(key)
        keys.append(key)
      }
    }
    icon.show(state: row.state, color: PaneAppearance.borderColor(state: row.state))
    layer?.addSublayer(icon)

    name.attributedStringValue = SidebarRowView.firstLine(for: row, color: palette.text.color)
    if !row.stateLine.isEmpty {
      let loud = row.tier == .asks || row.tier == .look
      stateWord.attributedStringValue = NSAttributedString(
        string: row.stateLine,
        attributes: [
          .font: NSFont.systemFont(ofSize: 11, weight: loud ? .semibold : .regular),
          .foregroundColor: loud
            ? (palette.stateText[row.state]?.color ?? palette.dimText.color)
            : palette.dimText.color,
        ])
      addSubview(stateWord)
    }
    if !row.directoryName.isEmpty {
      directory.stringValue = row.directoryName
      directory.textColor = palette.dimText.color
      addSubview(directory)
    }
    if let used = PaneHeaderModel.contextUsed(of: row) {
      let bar = ContextBar(used: used)
      addSubview(bar)
      contextBar = bar
    }
    grip.contentTintColor = palette.dimText.color

    // The whole of what is known, on hover and to VoiceOver, which otherwise hears a name and
    // nothing of the icon beside it.
    let details = SidebarModel.details(of: row)
    toolTip = details
    setAccessibilityElement(true)
    setAccessibilityRole(.group)
    setAccessibilityLabel(details)
    needsLayout = true
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    recolor()
  }

  /// Draws the header again from the colors now in force, for a config file read again or an
  /// appearance changed: what depends on them is computed per row, and one header is cheap.
  func recolor() {
    guard shown != nil else { return }
    draw()
  }

  // MARK: Layout

  static let padding: CGFloat = 8
  static let gripWidth: CGFloat = 16
  static let edgeWidth: CGFloat = 3

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    defer { CATransaction.commit() }
    tintLayer.frame = bounds
    edgeBar.frame = CGRect(x: 0, y: 0, width: Self.edgeWidth, height: bounds.height)
    guard let row = shown else { return }
    let parts = PaneHeaderModel.parts(width: bounds.width, for: row)
    var left = Self.padding
    for key in keys {
      key.frame = CGRect(
        x: left, y: ((bounds.height - KeycapView.height) / 2).rounded(), width: KeycapView.width,
        height: KeycapView.height)
      left += KeycapView.width + SidebarRowView.keyGap
    }
    if !keys.isEmpty { left += 5 }
    icon.frame = CGRect(
      x: left, y: ((bounds.height - StateIcon.size) / 2).rounded(), width: StateIcon.size,
      height: StateIcon.size)
    left += StateIcon.size + 7

    var right = bounds.width - Self.padding
    grip.frame = CGRect(
      x: right - Self.gripWidth, y: (bounds.height - 12) / 2, width: Self.gripWidth, height: 12)
    right -= Self.gripWidth + 4
    if let bar = contextBar {
      bar.isHidden = !parts.context
      if parts.context {
        bar.frame = CGRect(
          x: right - ContextBar.width, y: (bounds.height - ContextBar.height) / 2,
          width: ContextBar.width, height: ContextBar.height)
        right -= ContextBar.width + 8
      }
    }
    if stateWord.superview != nil {
      let width = min(ceil(stateWord.fittingSize.width), max(0, (right - left) * 0.45))
      place(stateWord, x: right - width, width: width)
      right -= width + 8
    }
    directory.isHidden = !parts.directory
    if directory.superview != nil, parts.directory {
      let width = min(ceil(directory.fittingSize.width), max(0, (right - left) * 0.35))
      place(directory, x: right - width, width: width)
      right -= width + 8
    }
    place(name, x: left, width: max(0, right - left))
  }

  private func place(_ label: NSTextField, x: CGFloat, width: CGFloat) {
    let height = min(bounds.height, ceil(label.fittingSize.height))
    label.frame = CGRect(
      x: x, y: ((bounds.height - height) / 2).rounded(), width: width, height: height)
  }

  // MARK: The mouse

  /// Every point of the header is the header's, so that a press on a label is a press on the
  /// header and nothing under it.
  override func hitTest(_ point: NSPoint) -> NSView? {
    bounds.contains(convert(point, from: superview)) ? self : nil
  }

  /// A press into a window that is not key still grabs, as a click into one still picks a pane.
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    onPressed?()
  }

  override func menu(for event: NSEvent) -> NSMenu? {
    onPressed?()
    return onMenu?()
  }

  override func mouseDragged(with event: NSEvent) {
    guard !dragging, let pane, let chrome = superview else { return }
    let item = NSPasteboardItem()
    item.setString(PaneDrop.payload(pane), forType: PaneDrop.type)
    let dragged = NSDraggingItem(pasteboardWriter: item)
    let preview = Self.preview(for: chrome.bounds.size)
    let at = convert(event.locationInWindow, from: nil)
    dragged.setDraggingFrame(
      CGRect(
        x: at.x - preview.size.width / 2, y: at.y - preview.size.height / 2,
        width: preview.size.width, height: preview.size.height),
      contents: preview)
    dragging = true
    showGrip()
    let session = beginDraggingSession(with: [dragged], event: event, source: self)
    // A drop nowhere ends there, as Ghostty's does, rather than sliding back to remind
    // somebody of a drag they already know they abandoned.
    session.animatesToStartingPositionsOnCancelOrFail = false
  }

  func draggingSession(
    _ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext
  ) -> NSDragOperation {
    context == .withinApplication ? .move : []
  }

  func draggingSession(_ session: NSDraggingSession, movedTo screenPoint: NSPoint) {
    NSCursor.closedHand.set()
  }

  func draggingSession(
    _ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation
  ) {
    dragging = false
    showGrip()
  }

  override func resetCursorRects() {
    addCursorRect(bounds, cursor: .openHand)
  }

  override func updateTrackingAreas() {
    trackingAreas.forEach(removeTrackingArea)
    addTrackingArea(
      NSTrackingArea(
        rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
        owner: self))
    super.updateTrackingAreas()
  }

  override func mouseEntered(with event: NSEvent) {
    hovering = true
    showGrip()
  }

  override func mouseExited(with event: NSEvent) {
    hovering = false
    showGrip()
  }

  private func showGrip() {
    grip.isHidden = !gripShown
  }

  /// What follows the pointer: the pane's outline at a fifth of its size.
  ///
  /// An outline rather than Ghostty's snapshot of the pane, because a terminal here draws into a
  /// Metal layer that `cacheDisplay` cannot read, and a snapshot would come out blank.
  static func preview(for size: CGSize) -> NSImage {
    let scaled = CGSize(width: max(size.width * 0.2, 24), height: max(size.height * 0.2, 16))
    return NSImage(size: scaled, flipped: false) { rect in
      let outline = NSBezierPath(roundedRect: rect.insetBy(dx: 1, dy: 1), xRadius: 4, yRadius: 4)
      PaneAppearance.focusColor.withAlphaComponent(0.25).setFill()
      outline.fill()
      PaneAppearance.focusColor.setStroke()
      outline.lineWidth = 2
      outline.stroke()
      return true
    }
  }
}
