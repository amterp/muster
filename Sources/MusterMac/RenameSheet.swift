import AppKit

/// Asks for a name, and hands back what was typed.
///
/// The first text input in this app, which is most of the cost of being able to name a pane.
/// Everything else about naming is a request the core already understands - the hard part is
/// that until now nothing here had a field somebody could type into.
///
/// **A sheet rather than an editable row in the list.** The list rebuilds every row whenever a
/// roster arrives or an agent blinks state, and an editor living inside one of those rows would
/// be destroyed mid-word by an agent going from working to idle. A sheet is outside that, so
/// nothing a daemon says while somebody is typing can take the field away.
///
/// It also keeps the one action path honest: the sheet only collects text, and what it collects
/// goes out as the same rename request a CLI would send. Nothing here decides anything.
@MainActor
public enum RenameSheet {
  /// Runs the sheet on a window and calls back with the name, or not at all if it was dropped.
  ///
  /// `current` is what somebody typed before, not what the row draws: offering `muster · claude`
  /// as the starting text would ask them to delete a name they never wrote. Empty is a new name
  /// and is the ordinary case.
  ///
  /// An empty answer is a real answer and is passed on - it is how a name is taken away.
  public static func ask(
    on host: NSWindow, subject: String, current: String, then act: @escaping (String) -> Void
  ) {
    let sheet = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 360, height: 108),
      styleMask: [.titled], backing: .buffered, defer: false)

    let prompt = NSTextField(labelWithString: "Name this \(subject)")
    prompt.font = .systemFont(ofSize: 13, weight: .semibold)

    let field = NSTextField(string: current)
    // A real editable field, which is what makes ⌃⌘Space work: the emoji picker is the
    // platform's and needs no picker of Muster's own, and a name like "🔥 payments spike" is
    // exactly what this is for.
    field.isEditable = true
    field.isBezeled = true
    field.font = .systemFont(ofSize: 13)
    field.placeholderString = "Leave empty to remove the name"

    let cancel = NSButton(title: "Cancel", target: nil, action: nil)
    cancel.bezelStyle = .rounded
    // Escape drops the sheet, which is what the platform's cancel key means everywhere else.
    cancel.keyEquivalent = "\u{1b}"

    let confirm = NSButton(title: "Name", target: nil, action: nil)
    confirm.bezelStyle = .rounded
    // Return commits, so naming something is type-and-enter rather than type-and-reach.
    confirm.keyEquivalent = "\r"

    let content = NSView(frame: sheet.contentLayoutRect)
    for view in [prompt, field, cancel, confirm] as [NSView] {
      content.addSubview(view)
    }
    sheet.contentView = content

    let inset: CGFloat = 20
    let width = 360 - inset * 2
    prompt.frame = NSRect(x: inset, y: 72, width: width, height: 18)
    field.frame = NSRect(x: inset, y: 44, width: width, height: 22)
    confirm.frame = NSRect(x: 360 - inset - 80, y: 10, width: 80, height: 24)
    cancel.frame = NSRect(x: 360 - inset - 80 - 88, y: 10, width: 80, height: 24)

    let finish = Finisher(host: host, sheet: sheet, field: field, act: act)
    cancel.target = finish
    cancel.action = #selector(Finisher.cancel)
    confirm.target = finish
    confirm.action = #selector(Finisher.confirm)
    // Held by the sheet, because nothing else refers to it and a button's target is weak -
    // without this the object is gone before anybody presses anything.
    held[ObjectIdentifier(sheet)] = finish

    host.beginSheet(sheet) { _ in held[ObjectIdentifier(sheet)] = nil }
    sheet.makeFirstResponder(field)
  }

  /// The sheets currently up, keeping each one's button target alive until it answers.
  ///
  /// Isolated like everything else here rather than `nonisolated(unsafe)`. It was the latter,
  /// which bought nothing - every reader is already on the main actor - while turning off the
  /// one check that would refuse a future call from a background thread. A dictionary written
  /// from two threads corrupts silently; a compiler error does not.
  private static var held: [ObjectIdentifier: Finisher] = [:]

  /// What the two buttons are wired to. A class because a button's target has to be an object.
  @MainActor
  private final class Finisher: NSObject {
    private let host: NSWindow
    private let sheet: NSWindow
    private let field: NSTextField
    private let act: (String) -> Void

    init(host: NSWindow, sheet: NSWindow, field: NSTextField, act: @escaping (String) -> Void) {
      self.host = host
      self.sheet = sheet
      self.field = field
      self.act = act
    }

    @objc func cancel() {
      host.endSheet(sheet)
    }

    @objc func confirm() {
      let typed = field.stringValue
      host.endSheet(sheet)
      act(typed)
    }
  }
}
