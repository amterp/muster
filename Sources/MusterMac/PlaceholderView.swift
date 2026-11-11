import AppKit

/// Where a pane goes whose machine is not answering: the window opened while that machine was
/// down. It names the pane and its machine, keeps the pane's place so nothing moves when the
/// machine answers, and offers Remove from Tab for a machine that is gone for good.
@MainActor
final class PlaceholderView: NSView {
  private(set) var placeholder: PaneTree.Placeholder
  private let label = NSTextField(wrappingLabelWithString: "")

  /// What a refused Remove from Tab shows the person.
  var refused: ContextMenus.Refused = { _, _ in }

  init(_ placeholder: PaneTree.Placeholder) {
    self.placeholder = placeholder
    super.init(frame: .zero)
    wantsLayer = true
    layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
    label.alignment = .center
    label.textColor = .secondaryLabelColor
    label.isSelectable = false
    addSubview(label)
    show(placeholder)
  }

  required init?(coder: NSCoder) {
    fatalError("muster builds its views in code")
  }

  func show(_ placeholder: PaneTree.Placeholder) {
    self.placeholder = placeholder
    label.stringValue = PlaceholderView.text(for: placeholder)
  }

  /// The pane by its id, which is all a window knows of it: its callsign is kept by its machine.
  static func text(for placeholder: PaneTree.Placeholder) -> String {
    "\(placeholder.paneID)\n\(placeholder.daemon) is unreachable"
  }

  override func layout() {
    super.layout()
    let size = label.sizeThatFits(NSSize(width: max(bounds.width - 16, 0), height: bounds.height))
    label.frame = NSRect(
      x: 8, y: (bounds.height - size.height) / 2, width: max(bounds.width - 16, 0),
      height: size.height)
  }

  override func menu(for event: NSEvent) -> NSMenu? {
    ContextMenus.placeholder(placeholder, refused: refused)
  }
}
