import AppKit

/// The strip above the agent list that says who wants you (Fencil `needs-you-strip`).
///
/// One card per agent or group the core says is asking, in the core's order. An agent in it keeps
/// its row in the list below, so the list never reorders. Absent, with no height, while nothing
/// asks. Folded it is one line: the icons of whoever is asking and how many there are.
///
/// A scroll view holds the cards, because the list can be as long as the window has agents and the
/// strip must not push the list off screen: it is held to a share of the sidebar and the rest is
/// reached by scrolling.
@MainActor
final class NeedsYouStripView: NSView {
  static let headerHeight: CGFloat = 26

  /// Called when somebody picks a pane's card, or a group's.
  var onPanePicked: ((PaneKey) -> Void)?
  var onGroupPicked: ((String, String) -> Void)?
  /// Called when somebody clicks the header, with whether they are asking to fold it.
  var onFoldToggled: ((Bool) -> Void)?

  private let header = NeedsYouHeader()
  private let scroll = NSScrollView()
  private let cardsView = FlippedView()
  private var cardViews: [NeedsYouCardView] = []

  private(set) var cards: [NeedsYouModel.Card] = []
  private(set) var isFolded = false

  /// How many cards have been drawn since this was made. What a change costs: one card for one
  /// agent's change, none for a republish that changed nothing.
  private(set) var redrawCount = 0

  /// Whether the strip is there at all: it is absent while nothing asks.
  var isPresent: Bool { !cards.isEmpty }

  override init(frame: NSRect) {
    super.init(frame: frame)
    isHidden = true
    scroll.documentView = cardsView
    scroll.hasVerticalScroller = true
    scroll.drawsBackground = false
    scroll.autohidesScrollers = true
    addSubview(scroll)
    addSubview(header)
    header.onPressed = { [weak self] in
      guard let self else { return }
      onFoldToggled?(!isFolded)
    }
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  /// What a test reads: the cards drawn now, and the header's words and icons.
  var shownCards: [NeedsYouCardView] { cardViews }
  var shownHeaderIcons: [String] { header.shownIcons }
  func pressHeader() { header.onPressed?() }
  var shownCount: Int? { header.shownCount }

  /// Draws `cards`, redrawing only the cards that differ from the ones already there.
  func show(cards fresh: [NeedsYouModel.Card], folded: Bool) {
    let sameShape = fresh.count == cards.count
    let foldChanged = folded != isFolded
    let before = cards
    cards = fresh
    isFolded = folded
    isHidden = fresh.isEmpty
    guard !fresh.isEmpty else {
      for view in cardViews { view.removeFromSuperview() }
      cardViews = []
      header.show(cards: [], folded: folded)
      return
    }
    if sameShape, !cardViews.isEmpty {
      for index in fresh.indices where before[index] != fresh[index] {
        cardViews[index].show(fresh[index])
        redrawCount += 1
      }
    } else {
      for view in cardViews { view.removeFromSuperview() }
      cardViews = fresh.map { card in
        let view = NeedsYouCardView()
        view.show(card)
        view.onPressed = { [weak self] card in self?.picked(card) }
        cardsView.addSubview(view)
        return view
      }
      redrawCount += fresh.count
    }
    if sameShape, !foldChanged, before == fresh { return }
    header.show(cards: fresh, folded: folded)
    needsLayout = true
  }

  private func picked(_ card: NeedsYouModel.Card) {
    switch card.subject {
    case .pane(let key): onPanePicked?(key)
    case .group(let daemon, let group): onGroupPicked?(daemon, group)
    }
  }

  /// How tall the strip is at this width, held to `limit`: nothing while it is absent, one line
  /// folded, and otherwise the header and every card, or as many as the limit leaves room for.
  func height(forWidth width: CGFloat, limit: CGFloat) -> CGFloat {
    guard !cards.isEmpty else { return 0 }
    if isFolded { return Self.headerHeight }
    return min(Self.headerHeight + contentHeight, max(Self.headerHeight, limit))
  }

  private var contentHeight: CGFloat {
    cards.reduce(Self.bottomPadding) { $0 + NeedsYouCardView.height(of: $1) }
  }

  static let bottomPadding: CGFloat = 6

  override func layout() {
    super.layout()
    header.frame = CGRect(
      x: 0, y: bounds.height - Self.headerHeight, width: bounds.width, height: Self.headerHeight)
    scroll.isHidden = isFolded
    guard !isFolded else { return }
    scroll.frame = CGRect(
      x: 0, y: 0, width: bounds.width, height: max(0, bounds.height - Self.headerHeight))
    cardsView.frame = CGRect(x: 0, y: 0, width: bounds.width, height: contentHeight)
    var top: CGFloat = 0
    for (view, card) in zip(cardViews, cards) {
      let height = NeedsYouCardView.height(of: card)
      view.frame = CGRect(x: 0, y: top, width: bounds.width, height: height)
      top += height
    }
  }
}

/// A view that measures from its top, which is how a stack of cards in a scroll view reads.
final class FlippedView: NSView {
  override var isFlipped: Bool { true }
}

/// The strip's heading: the fold chevron, `NEEDS YOU`, and how many are asking - and, folded, the
/// icon of each that fits between the two.
@MainActor
final class NeedsYouHeader: NSView {
  var onPressed: (() -> Void)?

  private let title = NSTextField(labelWithString: "NEEDS YOU")
  private var control: FoldControl?
  private var badge: CountBadge?
  private var icons: [NSView] = []
  private var states: [String] = []
  private var cards: [NeedsYouModel.Card] = []

  var shownIcons: [String] { icons.filter { !$0.isHidden }.map(iconName(of:)) }
  var shownCount: Int? { badge?.count }

  override init(frame: NSRect) {
    super.init(frame: frame)
    title.font = .systemFont(ofSize: 10, weight: .semibold)
    title.textColor = .secondaryLabelColor
    addSubview(title)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  func show(cards: [NeedsYouModel.Card], folded: Bool) {
    self.cards = cards
    control?.removeFromSuperview()
    badge?.removeFromSuperview()
    for icon in icons { icon.removeFromSuperview() }
    icons = []
    guard !cards.isEmpty else { return }
    let chevron = FoldControl(folded: folded)
    chevron.onPressed = { [weak self] in self?.onPressed?() }
    addSubview(chevron)
    control = chevron
    let count = CountBadge(count: cards.count, saying: "\(cards.count) need you")
    addSubview(count)
    badge = count
    if folded {
      icons = cards.map { card in
        let icon = card.isGroup ? Self.envelope() : StateIconView(state: card.state)
        addSubview(icon)
        return icon
      }
    }
    toolTip = folded ? cards.map(NeedsYouModel.details(of:)).joined(separator: "\n\n") : nil
    setAccessibilityElement(true)
    setAccessibilityRole(.button)
    setAccessibilityLabel(folded ? "show who needs you" : "fold who needs you")
    needsLayout = true
  }

  private static func envelope() -> NSView {
    let image = NSImage(systemSymbolName: "envelope.fill", accessibilityDescription: "message")?
      .withSymbolConfiguration(.init(pointSize: 10, weight: .regular))
    let view = NSImageView(image: image ?? NSImage())
    view.contentTintColor = .controlAccentColor
    return view
  }

  private func iconName(of view: NSView) -> String {
    (view as? StateIconView)?.state ?? "message"
  }

  /// Every point of the header folds the strip, so a press on the label is not a press on nothing.
  override func hitTest(_ point: NSPoint) -> NSView? {
    bounds.contains(convert(point, from: superview)) ? self : nil
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    onPressed?()
  }

  override func layout() {
    super.layout()
    guard let control, let badge else { return }
    control.frame = CGRect(
      x: 4, y: (bounds.height - FoldControl.height) / 2, width: FoldControl.width,
      height: FoldControl.height)
    let titleSize = title.fittingSize
    title.frame = CGRect(
      x: 22, y: (bounds.height - titleSize.height) / 2, width: titleSize.width,
      height: titleSize.height)
    let badgeSize = badge.fittingSize
    badge.frame = CGRect(
      x: bounds.width - badgeSize.width - 12, y: (bounds.height - badgeSize.height) / 2,
      width: badgeSize.width, height: badgeSize.height)
    // Folded, whoever is asking, in the core's order, as many as fit between the label and the
    // count. The order is urgency, so what is cut is the least of them.
    let step: CGFloat = 18
    let room = max(0, badge.frame.minX - 8 - title.frame.maxX - 10)
    let capacity = Int(room / step)
    var x = title.frame.maxX + 10
    for (index, icon) in icons.enumerated() {
      icon.isHidden = index >= capacity
      icon.frame = CGRect(
        x: x, y: ((bounds.height - StateIcon.size) / 2).rounded(), width: StateIcon.size,
        height: StateIcon.size)
      x += step
    }
  }
}

/// A `StateIcon` as a view, for the places that need one where a layer will not do.
@MainActor
final class StateIconView: NSView {
  private let icon = StateIcon()
  var state: String { icon.state }

  init(state: String) {
    super.init(frame: .zero)
    wantsLayer = true
    icon.show(state: state, color: PaneAppearance.borderColor(state: state))
    layer?.addSublayer(icon)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    icon.frame = bounds
    CATransaction.commit()
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    effectiveAppearance.performAsCurrentDrawingAppearance {
      icon.show(state: icon.state, color: PaneAppearance.borderColor(state: icon.state))
    }
  }
}

/// One card of the strip: an icon, who it is, what it wants and for how long, the keys that reach
/// it, and what it said.
@MainActor
final class NeedsYouCardView: NSView {
  /// Called when somebody picks the card.
  var onPressed: ((NeedsYouModel.Card) -> Void)?

  private let background = CALayer()
  private let edge = CALayer()
  private let icon = StateIcon()
  private var envelope: NSImageView?
  private let title = NSTextField(labelWithString: "")
  private let word = NSTextField(labelWithString: "")
  private let detail = NSTextField(wrappingLabelWithString: "")
  private var keys: [KeycapView] = []
  private(set) var card: NeedsYouModel.Card?

  /// What a test reads: the icon's state (nil on a group), the words and the keys drawn.
  var shownIcon: String? { icon.superlayer == nil ? nil : icon.state }
  var shownTitle: String { title.stringValue }
  var shownWord: String { word.stringValue }
  var shownDetail: String? { detail.superview == nil ? nil : detail.stringValue }
  var shownKeys: [Int] { keys.map(\.value) }
  var showsTint: Bool { card?.isTinted == true }

  static let padding: CGFloat = 14
  static let textLeft: CGFloat = 34

  /// A card is two heights, with what it said and without, for the reason a row is: a list read by
  /// scanning should not move because of what an agent happened to write.
  static func height(of card: NeedsYouModel.Card) -> CGFloat {
    card.detail.isEmpty ? 32 : 60
  }

  override init(frame: NSRect) {
    super.init(frame: frame)
    wantsLayer = true
    background.cornerRadius = 8
    edge.cornerRadius = 1.5
    title.lineBreakMode = .byTruncatingTail
    word.lineBreakMode = .byTruncatingTail
    detail.maximumNumberOfLines = 2
    detail.lineBreakMode = .byTruncatingTail
    detail.font = .systemFont(ofSize: 11)
    layer?.addSublayer(background)
    layer?.addSublayer(edge)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  func show(_ card: NeedsYouModel.Card) {
    self.card = card
    let palette = ChromePalette.current(for: effectiveAppearance).list
    for key in keys { key.removeFromSuperview() }
    keys = []
    icon.removeFromSuperlayer()
    envelope?.removeFromSuperview()
    envelope = nil
    detail.removeFromSuperview()

    let edgeColor: NSColor
    if card.isGroup {
      edgeColor = .controlAccentColor
      let image = NSImage(systemSymbolName: "envelope.fill", accessibilityDescription: "message")?
        .withSymbolConfiguration(.init(pointSize: 11, weight: .regular))
      let view = NSImageView(image: image ?? NSImage())
      view.contentTintColor = .controlAccentColor
      addSubview(view)
      envelope = view
    } else {
      edgeColor = PaneAppearance.borderColor(state: card.state)
      icon.show(state: card.state, color: edgeColor)
      layer?.addSublayer(icon)
    }
    let fill = card.isTinted ? palette.tint[card.state] ?? palette.raised : palette.raised
    background.backgroundColor = fill.color.cgColor
    edge.backgroundColor = edgeColor.cgColor

    title.stringValue = card.title
    title.font =
      card.isGroup
      ? .systemFont(ofSize: 12, weight: .semibold)
      : .monospacedSystemFont(ofSize: 11.5, weight: .semibold)
    title.textColor = palette.text.color
    word.stringValue = card.word
    word.font = .systemFont(ofSize: 11, weight: .semibold)
    word.textColor =
      card.isGroup || card.state.isEmpty
      ? palette.text.color : palette.stateText[card.state]?.color ?? palette.text.color
    addSubview(title)
    addSubview(word)
    for press in card.keys {
      let key = KeycapView(digit: press)
      key.show(digit: press, live: false, surface: palette)
      addSubview(key)
      keys.append(key)
    }
    if !card.detail.isEmpty {
      detail.stringValue = card.detail
      detail.textColor = palette.dimText.color
      addSubview(detail)
    }
    toolTip = NeedsYouModel.details(of: card)
    setAccessibilityElement(true)
    setAccessibilityRole(.button)
    setAccessibilityLabel(
      NeedsYouModel.details(of: card).replacingOccurrences(of: "\n", with: ", "))
    needsLayout = true
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    if let card { show(card) }
  }

  /// Every point of the card picks it, so a press on its words is not a press on nothing.
  override func hitTest(_ point: NSPoint) -> NSView? {
    bounds.contains(convert(point, from: superview)) ? self : nil
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseUp(with event: NSEvent) {
    guard let card, bounds.contains(convert(event.locationInWindow, from: nil)) else { return }
    onPressed?(card)
  }

  /// A press, for a test that has no event to give.
  func press() {
    if let card { onPressed?(card) }
  }

  override func layout() {
    super.layout()
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    defer { CATransaction.commit() }
    let panel = bounds.insetBy(dx: 6, dy: 2)
    background.frame = panel
    edge.frame = CGRect(x: panel.minX, y: panel.minY + 7, width: 3, height: panel.height - 14)

    let top = bounds.height - 8
    let first = CGRect(x: 0, y: top - 18, width: bounds.width, height: 18)
    let iconFrame = CGRect(
      x: Self.padding + 2, y: first.midY - StateIcon.size / 2, width: StateIcon.size,
      height: StateIcon.size)
    icon.frame = iconFrame.integral
    envelope?.frame = iconFrame

    var right = bounds.width - Self.padding - 4
    for key in keys.reversed() {
      key.frame = CGRect(
        x: right - KeycapView.width, y: first.midY - KeycapView.height / 2,
        width: KeycapView.width, height: KeycapView.height)
      right -= KeycapView.width + SidebarRowView.keyGap
    }
    if !keys.isEmpty { right -= 6 }
    let titleSize = title.fittingSize
    let titleWidth = min(ceil(titleSize.width), max(0, (right - Self.textLeft) * 0.6))
    title.frame = CGRect(
      x: Self.textLeft, y: first.midY - titleSize.height / 2, width: titleWidth,
      height: titleSize.height)
    let wordSize = word.fittingSize
    let wordX = title.frame.maxX + 8
    word.frame = CGRect(
      x: wordX, y: first.midY - wordSize.height / 2, width: max(0, right - wordX),
      height: wordSize.height)
    detail.frame = CGRect(
      x: Self.textLeft, y: 8, width: max(0, bounds.width - Self.textLeft - Self.padding),
      height: max(0, first.minY - 4 - 8))
  }
}
