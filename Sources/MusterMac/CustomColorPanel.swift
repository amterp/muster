import AppKit

@MainActor
final class CustomColorPanel: NSObject, NSWindowDelegate {
  static let shared = CustomColorPanel()

  private let present: (NSColorPanel) -> Void
  private weak var source: NSWindow?
  private var save: (@MainActor (String) -> Void)?

  init(present: @escaping (NSColorPanel) -> Void = { $0.orderFront(nil) }) {
    self.present = present
    super.init()
  }

  func open(
    for window: NSWindow, title: String, color: RGB, save: @escaping @MainActor (String) -> Void
  ) {
    let panel = NSColorPanel.shared
    if !panel.makeFirstResponder(nil) { panel.endEditing(for: nil) }
    source = window
    self.save = save

    panel.delegate = self
    panel.title = title
    panel.showsAlpha = false
    panel.color = color.color
    panel.accessoryView = buttons
    present(panel)
  }

  func sourceClosed(_ window: NSWindow) {
    if source === window { cancel() }
  }

  func windowWillClose(_ notification: Notification) {
    discard()
  }

  static func hex(_ color: NSColor) -> String? {
    guard let srgb = color.usingColorSpace(.sRGB) else { return nil }
    return RGB(red: srgb.redComponent, green: srgb.greenComponent, blue: srgb.blueComponent).hex
  }

  private lazy var buttons: NSView = {
    let cancel = NSButton(title: "Cancel", target: self, action: #selector(Self.cancel))
    cancel.bezelStyle = .rounded
    cancel.keyEquivalent = "\u{1b}"
    let apply = NSButton(title: "Apply", target: self, action: #selector(Self.apply))
    apply.bezelStyle = .rounded
    let stack = NSStackView(views: [cancel, apply])
    stack.orientation = .horizontal
    stack.spacing = 8
    stack.edgeInsets = NSEdgeInsets(top: 4, left: 8, bottom: 8, right: 8)
    stack.setFrameSize(stack.fittingSize)
    return stack
  }()

  @objc private func apply() {
    guard source != nil, let save else { return }
    let panel = NSColorPanel.shared
    // The sliders' text fields can still be editing when Apply is clicked.
    guard panel.makeFirstResponder(nil) else { return }
    guard let color = Self.hex(panel.color) else {
      ConfirmSheet.tell(
        on: panel, heading: "Choose an RGB color",
        body: "Muster couldn’t convert this color to sRGB.")
      return
    }
    cancel()
    save(color)
  }

  @objc private func cancel() {
    discard()
    NSColorPanel.shared.orderOut(nil)
  }

  private func discard() {
    source = nil
    save = nil
  }
}
