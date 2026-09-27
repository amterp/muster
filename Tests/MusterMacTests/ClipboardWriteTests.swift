import AppKit
import Testing

@testable import MusterMac

// A program in a pane set the clipboard (OSC 52), and the config allowed it - the core sends the
// event only then. What the shell owes it is the pasteboard, and nothing about it in the log
// beyond its size.

@Suite("a program's clipboard write", .ownsTheSeam)
@MainActor
struct ClipboardWriteTests {
  @Test("lands on the pasteboard, replacing what was there")
  func theTextLands() {
    _ = recorder()
    // A pasteboard of the test's own, so running the suite never overwrites what the developer
    // last copied.
    let board = NSPasteboard(name: NSPasteboard.Name("muster.tests.clipboard.\(UUID())"))
    defer { board.releaseGlobally() }
    board.clearContents()
    board.setString("what was there", forType: .string)

    var write = Muster_ClipboardWrite()
    write.daemonID = "local"
    write.paneID = "p1w3r07bsd"
    write.text = "copied by vim"
    var event = Muster_Event()
    event.clipboardWrite = write

    Core.deliver(event, pasteboard: board)

    #expect(board.string(forType: .string) == "copied by vim")
  }

  @Test("is logged by its size and never its text")
  func theTextStaysOutOfTheLog() {
    let recorder = recorder()
    let board = NSPasteboard(name: NSPasteboard.Name("muster.tests.clipboard.\(UUID())"))
    defer { board.releaseGlobally() }
    var write = Muster_ClipboardWrite()
    write.paneID = "p1w3r07bsd"
    write.text = "hunter2"
    var event = Muster_Event()
    event.clipboardWrite = write

    Core.deliver(event, pasteboard: board)

    let logged = recorder.requests.compactMap {
      if case .logRecord(let record) = $0.payload { record } else { nil }
    }
    #expect(logged.contains { $0.fields["bytes"] == "7" }, "the write was not logged by size")
    #expect(!logged.contains { $0.fields.values.contains { $0.contains("hunter2") } })
  }
}
