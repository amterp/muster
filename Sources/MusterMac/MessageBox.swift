import AppKit
import SwiftUI

// The message box's field: an AppKit text view rather than SwiftUI's `TextEditor`, which on
// macOS 14 cannot say where the cursor is. The core needs that for every edit (`SaveDraft`
// completes the @name at the cursor, and puts the cursor back where its rewrite leaves it), and
// the field needs to know about Backspace at the very start and about an input method's
// composition, neither of which a SwiftUI binding reports.

/// Where a cursor is, in the Unicode scalar values the core counts and the UTF-16 units AppKit
/// counts.
enum ScalarOffsets {
  /// The scalars before `utf16` units of `text`. An offset inside a scalar counts the scalar it
  /// is inside as before it.
  static func scalars(beforeUTF16 utf16: Int, in text: String) -> Int {
    var units = 0
    var count = 0
    for scalar in text.unicodeScalars {
      if units >= utf16 { break }
      units += scalar.utf16.count
      count += 1
    }
    return count
  }

  /// The UTF-16 units before `scalars` scalar values of `text`.
  static func utf16(beforeScalars scalars: Int, in text: String) -> Int {
    text.unicodeScalars.prefix(max(0, scalars)).reduce(0) { $0 + $1.utf16.count }
  }
}

struct MessageBoxField: NSViewRepresentable {
  @ObservedObject var box: MessageBox

  func makeCoordinator() -> Coordinator {
    Coordinator(box: box)
  }

  func makeNSView(context: Context) -> NSScrollView {
    let scroll = NSScrollView()
    scroll.hasVerticalScroller = true
    scroll.drawsBackground = false
    scroll.borderType = .noBorder
    let field = MessageBoxTextView(frame: .zero)
    field.box = box
    field.delegate = context.coordinator
    field.isRichText = false
    field.importsGraphics = false
    field.allowsUndo = true
    field.drawsBackground = false
    field.font = .systemFont(ofSize: NSFont.systemFontSize)
    field.isAutomaticQuoteSubstitutionEnabled = false
    field.isAutomaticDashSubstitutionEnabled = false
    field.isAutomaticTextReplacementEnabled = false
    field.isVerticallyResizable = true
    field.isHorizontallyResizable = false
    field.autoresizingMask = [.width]
    field.textContainer?.widthTracksTextView = true
    field.setAccessibilityLabel("Message box")
    scroll.documentView = field
    field.take(box.draft)
    context.coordinator.applied = box.revision
    return scroll
  }

  func updateNSView(_ scroll: NSScrollView, context: Context) {
    guard let field = scroll.documentView as? MessageBoxTextView else { return }
    field.box = box
    field.isEditable = !box.pending
    // What the core changed that the field did not: a post's outcome, Reply, a recipient taken
    // away. Never into a composition, which would break the input method's hold on its text.
    if context.coordinator.applied != box.revision, !field.hasMarkedText() {
      field.take(box.draft)
      context.coordinator.applied = box.revision
    }
  }

  @MainActor
  final class Coordinator: NSObject, NSTextViewDelegate {
    let box: MessageBox
    /// The draft revision the field last showed.
    var applied = 0

    init(box: MessageBox) {
      self.box = box
    }

    func textDidChange(_ notification: Notification) {
      guard let field = notification.object as? MessageBoxTextView, !field.hasMarkedText() else {
        return
      }
      // An undo is announced by the Edit menu, and may be by AppKit too: once is enough.
      let cursor = field.scalarCursor
      if field.string == box.draft.text, cursor == Int(box.draft.cursor) { return }
      if let answer = box.edited(text: field.string, cursor: cursor) {
        field.take(answer)
      }
      applied = box.revision
    }

    func textViewDidChangeSelection(_ notification: Notification) {
      guard let field = notification.object as? MessageBoxTextView, !field.taking,
        !field.hasMarkedText()
      else { return }
      if let answer = box.moved(text: field.string, cursor: field.scalarCursor) {
        field.take(answer)
      }
    }
  }
}

/// The text view the message box types into.
final class MessageBoxTextView: NSTextView {
  weak var box: MessageBox?

  /// What Undo takes back: the field's own, not the window's, so that it can be forgotten
  /// whenever the core rewrites the text.
  private let history = UndoManager()

  override var undoManager: UndoManager? { history }

  /// Whether the field is showing a draft of the core's, whose text and cursor it sets one after
  /// the other: the cursor AppKit leaves in between is nobody's move.
  private(set) var taking = false

  /// The cursor, in Unicode scalar values.
  var scalarCursor: Int {
    ScalarOffsets.scalars(beforeUTF16: selectedRange().location, in: string)
  }

  /// Shows a draft as the core keeps it, unless the field already does.
  ///
  /// A rewrite of the core's - a completion, an @name taken as a recipient, a post clearing the
  /// box - forgets what Undo could take back (mine). Setting the text is not an edit Undo
  /// records, so what it had recorded would replay against text that is no longer there.
  /// Undoing the rewrite itself was the alternative, but undoing an @name taken as a recipient
  /// would put the name back as text beside the recipient, for the core to take again.
  func take(_ draft: Muster_MessageBoxDraft) {
    taking = true
    defer { taking = false }
    let cursor = ScalarOffsets.utf16(beforeScalars: Int(draft.cursor), in: draft.text)
    if string != draft.text {
      string = draft.text
      history.removeAllActions()
    }
    if selectedRange() != NSRange(location: cursor, length: 0) {
      setSelectedRange(NSRange(location: cursor, length: 0))
    }
  }

  override func keyDown(with event: NSEvent) {
    guard let box, !hasMarkedText() else { return super.keyDown(with: event) }
    if box.sendChord?.matches(event) == true {
      box.send()
      return
    }
    if !box.draft.completions.isEmpty, completes(event, in: box) { return }
    // Tab reaches the newest question's answers (MIP-9, section 6); with none it is text.
    let plain = event.modifierFlags.intersection([.shift, .control, .option, .command]).isEmpty
    if event.keyCode == 0x30, plain, box.tabToAnswers() { return }
    super.keyDown(with: event)
  }

  /// Tab or Return takes the highlighted completion, and the arrows move between them, while
  /// any are offered (mine). Otherwise both are text, and the arrows move the cursor.
  private func completes(_ event: NSEvent, in box: MessageBox) -> Bool {
    let plain = event.modifierFlags.intersection([.shift, .control, .option, .command]).isEmpty
    guard plain else { return false }
    switch event.keyCode {
    case 0x30, 0x24:  // Tab, Return
      if let answer = box.completeHighlighted(text: string, cursor: scalarCursor) {
        take(answer)
      }
      return true
    case 0x7E:  // Up
      box.moveHighlight(by: -1)
      return true
    case 0x7D:  // Down
      box.moveHighlight(by: 1)
      return true
    default:
      return false
    }
  }

  /// Backspace straight after the last recipient turns it back into text (MIP-9, section 3).
  override func deleteBackward(_ sender: Any?) {
    if let box, !box.draft.to.isEmpty, !hasMarkedText(),
      selectedRange() == NSRange(location: 0, length: 0)
    {
      if let answer = box.unaddress(text: string) { take(answer) }
      return
    }
    super.deleteBackward(sender)
  }
}
