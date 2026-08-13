import Foundation
import Testing

/// Compares a value against a checked-in file, or writes the file when asked.
///
/// The point of snapshots here is that coverage grows by adding data rather than test
/// code (`docs/testing.md`), so the file is the case and this is only the plumbing.
///
/// Regenerate with `MUSTER_UPDATE_SNAPSHOTS=1 swift test`. Read the diff before
/// committing it: a snapshot tool that makes accepting a change effortless is a tool
/// that eventually records a bug as the expectation.
public enum Snapshot {
  /// Whether this run rewrites snapshots instead of asserting against them.
  public static var isRecording: Bool {
    ProcessInfo.processInfo.environment["MUSTER_UPDATE_SNAPSHOTS"] == "1"
  }

  /// Asserts `actual` matches `<directory of testFile>/snapshots/<name>`.
  public static func expect(
    _ actual: String,
    named name: String,
    testFile: String = #filePath,
    sourceLocation: SourceLocation = #_sourceLocation
  ) throws {
    let path = directory(for: testFile).appendingPathComponent(name)

    guard !isRecording else {
      try FileManager.default.createDirectory(
        at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
      try actual.write(to: path, atomically: true, encoding: .utf8)
      // A recording run asserts nothing, so it must not be able to pass for a real one.
      Issue.record(
        """
        Recorded snapshot \(name). This run proves nothing: MUSTER_UPDATE_SNAPSHOTS was \
        set, so every case wrote its own expectation. Review the diff, then re-run \
        without it.
        """, sourceLocation: sourceLocation)
      return
    }

    guard let expected = try? String(contentsOf: path, encoding: .utf8) else {
      Issue.record(
        """
        No snapshot at \(path.path).
        Nothing was verified for this case. Create it with:
          MUSTER_UPDATE_SNAPSHOTS=1 swift test
        and read the file it writes before committing it.
        """, sourceLocation: sourceLocation)
      return
    }

    guard actual != expected else { return }

    Issue.record(
      """
      Snapshot \(name) does not match.
      \(firstDifference(expected: expected, actual: actual))
      If the new output is right, re-record with MUSTER_UPDATE_SNAPSHOTS=1 and review \
      the diff. If it is not, this is the bug the snapshot was there to catch.
      """, sourceLocation: sourceLocation)
  }

  private static func directory(for testFile: String) -> URL {
    URL(fileURLWithPath: testFile)
      .deletingLastPathComponent()
      .appendingPathComponent("snapshots")
  }

  /// The first differing line, with its neighbours.
  ///
  /// A whole-file dump of an 80x24 grid buries the one row that changed, which is the
  /// only thing the reader needs.
  private static func firstDifference(expected: String, actual: String) -> String {
    let expectedLines = expected.components(separatedBy: "\n")
    let actualLines = actual.components(separatedBy: "\n")

    for index in 0..<max(expectedLines.count, actualLines.count) {
      let want = index < expectedLines.count ? expectedLines[index] : nil
      let got = index < actualLines.count ? actualLines[index] : nil
      guard want != got else { continue }
      return """
        First difference at line \(index + 1):
          expected: \(want.map { "\"\($0)\"" } ?? "<end of file>")
          actual:   \(got.map { "\"\($0)\"" } ?? "<end of file>")
        """
    }
    return "Files differ only in trailing content."
  }
}
