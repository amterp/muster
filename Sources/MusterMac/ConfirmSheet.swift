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
/// **Whether Return confirms is each question's own choice** (`confirmOnReturn`). Return stays
/// unbound for the questions that exist to interrupt a reflexive keystroke over something nothing
/// brings back - Quit and Close Sessions, which ends every agent on every machine - where a sheet
/// that took the keystroke would be no sheet at all, and they confirm by click only. It is bound
/// for two questions, each the owner's decision. Closing a pane or a tab is there so that a
/// misclick or a reflexive ⌘W does not end an agent unseen, and answering it should be as quick as
/// asking: a closed pane comes back with its agent in its session (Reopen Pane), so a yes given
/// too fast is cheap to take back. Deleting a group takes Return too, though it is not cheap to
/// take back - its log is gone for every member - because the owner wanted the question answered
/// from the keyboard. The default is click only, so a new question has to say it wants Return.
/// Escape cancels either way, because a way out is not the same as a way through; the rename
/// sheet commits on Return because its whole gesture is type-and-enter.
///
/// `tell` is the same sheet with nothing to agree to: one button, on Return, for an act that was
/// refused and has to say why somewhere a person will see it.
@MainActor
public enum ConfirmSheet {
  /// Runs the sheet, and calls back only if it was confirmed.
  ///
  /// `confirm` is the button's words and should name the act - "Quit and Close Sessions", not
  /// "OK". Somebody reading only the buttons should still know what they picked.
  ///
  /// `preview` is the thing being agreed to, shown as it is: in a monospaced font, scrollable,
  /// and not editable, because what is confirmed is exactly what was shown. `otherwise` is
  /// called on cancel, for a caller that has something waiting on the answer either way.
  ///
  /// `confirmOnReturn` is whether Return presses the confirming button; see the type's comment
  /// for which questions say yes.
  public static func ask(
    on host: NSWindow, question: String, body: String, confirm: String,
    preview: String? = nil, confirmOnReturn: Bool = false, then act: @escaping () -> Void,
    otherwise cancelled: @escaping () -> Void = {}
  ) {
    present(
      on: host, heading: question, body: body, confirm: confirm, cancel: "Cancel",
      preview: preview, confirmOnReturn: confirmOnReturn, then: act, otherwise: cancelled)
  }

  /// Says why something did not happen, with one button to put the sheet away.
  public static func tell(on host: NSWindow, heading: String, body: String) {
    present(
      on: host, heading: heading, body: body, confirm: "OK", cancel: nil, preview: nil,
      confirmOnReturn: false, then: {}, otherwise: {})
  }

  /// The key that presses the confirming button: Return when the question asked for it, and
  /// always when there is no cancel, because the one button is then the way out.
  static func confirmKey(confirmOnReturn: Bool, cancellable: Bool) -> String {
    confirmOnReturn || !cancellable ? "\r" : ""
  }

  /// The sheet both of the above run.
  private static func present(
    on host: NSWindow, heading question: String, body: String, confirm: String, cancel: String?,
    preview: String?, confirmOnReturn: Bool, then act: @escaping () -> Void,
    otherwise cancelled: @escaping () -> Void
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

    let cancelling = cancel.map { NSButton(title: $0, target: nil, action: nil) }
    cancelling?.bezelStyle = .rounded
    cancelling?.keyEquivalent = "\u{1b}"

    let confirming = NSButton(title: confirm, target: nil, action: nil)
    confirming.bezelStyle = .rounded
    confirming.keyEquivalent = confirmKey(
      confirmOnReturn: confirmOnReturn, cancellable: cancelling != nil)

    let buttons = NSStackView(views: [cancelling, confirming].compactMap { $0 })
    buttons.orientation = .horizontal
    buttons.spacing = 12

    let previewed = preview.map { [shown($0)] } ?? []
    let stack = NSStackView(views: [heading, detail] + previewed + [buttons])
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

    let finish = Finisher(host: host, sheet: sheet, act: act, cancelled: cancelled)
    cancelling?.target = finish
    cancelling?.action = #selector(Finisher.cancel)
    confirming.target = finish
    confirming.action = #selector(Finisher.confirm)
    // Held by the sheet, because a button's target is weak and nothing else refers to this -
    // the same reason `RenameSheet` holds its own.
    held[ObjectIdentifier(sheet)] = finish

    host.beginSheet(sheet) { _ in held[ObjectIdentifier(sheet)] = nil }
  }

  private static var held: [ObjectIdentifier: Finisher] = [:]

  /// What is being agreed to, as text nobody can change on the way.
  private static func shown(_ text: String) -> NSView {
    let scroll = NSTextView.scrollableTextView()
    scroll.hasVerticalScroller = true
    scroll.borderType = .bezelBorder
    if let view = scroll.documentView as? NSTextView {
      view.string = text
      view.isEditable = false
      view.font = .monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
    }
    scroll.translatesAutoresizingMaskIntoConstraints = false
    NSLayoutConstraint.activate([
      scroll.widthAnchor.constraint(equalToConstant: 380),
      scroll.heightAnchor.constraint(equalToConstant: 160),
    ])
    return scroll
  }

  @MainActor
  private final class Finisher: NSObject {
    private let host: NSWindow
    private let sheet: NSWindow
    private let act: () -> Void
    private let cancelled: () -> Void

    init(
      host: NSWindow, sheet: NSWindow, act: @escaping () -> Void,
      cancelled: @escaping () -> Void
    ) {
      self.host = host
      self.sheet = sheet
      self.act = act
      self.cancelled = cancelled
    }

    @objc func cancel() {
      host.endSheet(sheet)
      cancelled()
    }

    @objc func confirm() {
      host.endSheet(sheet)
      act()
    }
  }
}
