import Foundation
import Testing

@testable import MusterMac

// A save that does not reach the watcher is a config that silently stops taking effect, and the
// person's only way to find out is noticing that a change did nothing. Real files, because what
// is under test is which writes the OS reports, and nothing short of the OS answers that.

/// A directory of its own holding a config file, removed when the case ends.
private struct Scratch {
  let directory: URL
  var config: URL { directory.appendingPathComponent("config.toml") }

  init() throws {
    directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("muster-watcher-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try Data("scrollback = 1\n".utf8).write(to: config)
  }

  func remove() {
    try? FileManager.default.removeItem(at: directory)
  }

  /// Writes over the file's own bytes, as `printf > config.toml` and an editor's write-in-place
  /// mode do: the file changes and the directory holding it does not.
  func writeInPlace(_ text: String) throws {
    let handle = try FileHandle(forWritingTo: config)
    try handle.truncate(atOffset: 0)
    try handle.write(contentsOf: Data(text.utf8))
    try handle.close()
  }

  /// Saves the way most editors do, by writing a new file and renaming it over the old one.
  func replace(_ text: String) throws {
    let written = directory.appendingPathComponent("config.toml.tmp")
    try Data(text.utf8).write(to: written)
    _ = try FileManager.default.replaceItemAt(config, withItemAt: written)
  }
}

/// Counts the watcher's calls.
@MainActor
private final class Calls {
  var count = 0
}

/// Waits up to two seconds for `calls` to reach `count`, letting the main queue run meanwhile.
@MainActor
private func reached(_ calls: Calls, _ count: Int) async -> Bool {
  for _ in 0..<200 where calls.count < count {
    try? await Task.sleep(for: .milliseconds(10))
  }
  return calls.count >= count
}

@MainActor
@Test func aConfigWrittenInPlaceIsReadAgain() async throws {
  let scratch = try Scratch()
  defer { scratch.remove() }
  let calls = Calls()
  let watcher = ConfigWatcher(path: scratch.config.path) { calls.count += 1 }
  #expect(watcher.start())

  try scratch.writeInPlace("scrollback = 2\n")

  #expect(await reached(calls, 1), "a write in place was never noticed")
  watcher.stop()
}

@MainActor
@Test func aWriteInPlaceAfterAReplacingSaveIsReadAgain() async throws {
  // The replacing save leaves a new file where the watched one was, and a watch still on the old
  // one would miss every write in place after it.
  let scratch = try Scratch()
  defer { scratch.remove() }
  let calls = Calls()
  let watcher = ConfigWatcher(path: scratch.config.path) { calls.count += 1 }
  #expect(watcher.start())

  try scratch.replace("scrollback = 2\n")
  #expect(await reached(calls, 1), "a replacing save was never noticed")
  try scratch.writeInPlace("scrollback = 3\n")

  #expect(await reached(calls, 2), "a write in place after a replacing save was never noticed")
  watcher.stop()
}
