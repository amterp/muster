import AppKit

/// Asks before something that cannot be taken back, and says what it would cost.
///
/// The second sheet in this app and built on the same terms as `RenameSheet`: a hand-made
/// `NSWindow` rather than an `NSAlert`, so the two modals in Muster look like each other rather
/// than one looking like the system and one not.
///
/// **What it is for is the cost, not the question.** "Are you sure?" is a dialog people learn to
/// dismiss without reading, and the one destructive action here ends processes holding somebody
/// else's work - three agents mid-task, on machines whose names are the only thing that would
/// make them recognisable. So the body is a list of what would end, and the button says what it
/// does rather than "OK".
///
/// Return is *not* bound to the confirming button, unlike the rename sheet. There, return
/// commits because the whole gesture is type-and-enter; here, the reflex this exists to
/// interrupt is exactly a keystroke, and a sheet that took one would be no sheet at all. Escape
/// still cancels, because a way out is not the same as a way through.
@MainActor
public enum ConfirmSheet {
  /// Runs the sheet, and calls back only if it was confirmed.
  ///
  /// `confirm` is the button's words and should name the act - "Quit and Close Sessions", not
  /// "OK". Somebody reading only the buttons should still know what they picked.
  public static func ask(
    on host: NSWindow, question: String, body: String, confirm: String,
    then act: @escaping () -> Void
  ) {
    let sheet = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 420, height: 10),
      styleMask: [.titled], backing: .buffered, defer: false)

    let heading = NSTextField(labelWithString: question)
    heading.font = .boldSystemFont(ofSize: NSFont.systemFontSize)

    let detail = NSTextField(wrappingLabelWithString: body)
    detail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
    detail.textColor = .secondaryLabelColor
    // A width to wrap against, since a wrapping label in a hand-built sheet has no layout to
    // take one from and would otherwise measure itself as one very long line.
    detail.preferredMaxLayoutWidth = 380

    let cancel = NSButton(title: "Cancel", target: nil, action: nil)
    cancel.bezelStyle = .rounded
    cancel.keyEquivalent = "\u{1b}"

    let confirming = NSButton(title: confirm, target: nil, action: nil)
    confirming.bezelStyle = .rounded

    let buttons = NSStackView(views: [cancel, confirming])
    buttons.orientation = .horizontal
    buttons.spacing = 12

    let stack = NSStackView(views: [heading, detail, buttons])
    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 12
    stack.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
    stack.setHuggingPriority(.defaultHigh, for: .vertical)

    let content = NSView(frame: .zero)
    content.addSubview(stack)
    stack.translatesAutoresizingMaskIntoConstraints = false
    NSLayoutConstraint.activate([
      stack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
      stack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
      stack.topAnchor.constraint(equalTo: content.topAnchor),
      stack.bottomAnchor.constraint(equalTo: content.bottomAnchor),
      content.widthAnchor.constraint(equalToConstant: 420),
    ])
    sheet.contentView = content
    // The buttons sit at the trailing edge, which is where the platform puts them.
    buttons.leadingAnchor.constraint(greaterThanOrEqualTo: stack.leadingAnchor).isActive = true
    buttons.trailingAnchor.constraint(equalTo: stack.trailingAnchor).isActive = true

    let finish = Finisher(host: host, sheet: sheet, act: act)
    cancel.target = finish
    cancel.action = #selector(Finisher.cancel)
    confirming.target = finish
    confirming.action = #selector(Finisher.confirm)
    // Held by the sheet, because a button's target is weak and nothing else refers to this -
    // the same reason `RenameSheet` holds its own.
    held[ObjectIdentifier(sheet)] = finish

    host.beginSheet(sheet) { _ in held[ObjectIdentifier(sheet)] = nil }
  }

  private static var held: [ObjectIdentifier: Finisher] = [:]

  @MainActor
  private final class Finisher: NSObject {
    private let host: NSWindow
    private let sheet: NSWindow
    private let act: () -> Void

    init(host: NSWindow, sheet: NSWindow, act: @escaping () -> Void) {
      self.host = host
      self.sheet = sheet
      self.act = act
    }

    @objc func cancel() {
      host.endSheet(sheet)
    }

    @objc func confirm() {
      host.endSheet(sheet)
      act()
    }
  }
}
