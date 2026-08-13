import Foundation
import Testing

/// A corpus of cases, and the Swift driver that runs them.
///
/// The core's behavior is defined by data rather than by this language's tests, so a core
/// rewritten in another language is verified by cases a working implementation already
/// passed (MIP-1, `docs/testing.md`). This file is the thin part: load, validate, compare,
/// and say something useful when a case fails.
///
/// Everything it refuses to load is deliberate. A corpus that silently accepts a case with
/// no stated reason, or a file that does not say where its expectations came from, is a
/// corpus that decays into a record of whatever the implementation happened to do.
public struct Conformance: Sendable {
  public let file: String
  public let concept: String
  public let source: Source
  public let cases: [Case]

  /// How much a file's expectations are worth trusting.
  public enum Source: String, Sendable {
    /// Captured from a real herdr, a real libghostty-vt, a real terminal. Re-derivable.
    case recorded
    /// Lifted from an existing suite: trusted exactly as far as that implementation was.
    case ported
    /// Our own policy, with a citation. No oracle beyond the citation.
    case authored
  }

  public struct Case: Sendable {
    public let name: String
    public let why: String
    public let given: JSONValue
    public let expect: JSONValue
  }

  public enum Invalid: Error, CustomStringConvertible {
    case unreadable(file: String, detail: String)
    case malformed(file: String, detail: String)

    public var description: String {
      switch self {
      case .unreadable(let file, let detail):
        "conformance corpus \(file) could not be read: \(detail)"
      case .malformed(let file, let detail):
        "conformance corpus \(file) is malformed: \(detail)"
      }
    }
  }

  /// Loads `corpus/conformance/<file>`.
  public static func load(_ file: String, driverFile: String = #filePath) throws -> Conformance {
    let path = repositoryRoot(from: driverFile)
      .appendingPathComponent("corpus/conformance")
      .appendingPathComponent(file)

    guard let data = try? Data(contentsOf: path) else {
      throw Invalid.unreadable(file: file, detail: "no file at \(path.path)")
    }
    guard let raw = try? JSONSerialization.jsonObject(with: data) else {
      throw Invalid.malformed(file: file, detail: "not JSON")
    }
    let document = JSONValue(raw)

    guard let concept = document["concept"]?.stringValue, !concept.isEmpty else {
      throw Invalid.malformed(file: file, detail: "no `concept`")
    }
    guard let sourceName = document["source"]?.stringValue,
      let source = Source(rawValue: sourceName)
    else {
      throw Invalid.malformed(
        file: file,
        detail: "`source` must be one of recorded, ported, authored - it says how far these "
          + "expectations can be trusted, and a file without one cannot be judged")
    }
    // A recorded file that cannot be re-derived is indistinguishable from an authored one
    // claiming provenance it does not have.
    if source == .recorded, (document["regenerate"]?.stringValue ?? "").isEmpty {
      throw Invalid.malformed(
        file: file,
        detail: "a `recorded` corpus must carry the `regenerate` command that "
          + "produced it, or its provenance is a claim rather than a fact")
    }
    guard (document["why"]?.stringValue ?? "").isEmpty == false else {
      throw Invalid.malformed(file: file, detail: "no file-level `why`")
    }
    guard let rawCases = document["cases"]?.arrayValue, !rawCases.isEmpty else {
      throw Invalid.malformed(
        file: file,
        detail: "no cases. An empty corpus passes every driver, which reads as "
          + "coverage and is not")
    }

    var cases: [Case] = []
    var names = Set<String>()
    for (index, raw) in rawCases.enumerated() {
      guard let name = raw["name"]?.stringValue, !name.isEmpty else {
        throw Invalid.malformed(file: file, detail: "case \(index) has no `name`")
      }
      guard names.insert(name).inserted else {
        throw Invalid.malformed(
          file: file, detail: "two cases named `\(name)`, so a failure could not say which")
      }
      // The load-bearing rule. The comments in the tests these came from are the best
      // documentation in the repo, and a table row does not carry them by default.
      guard let why = raw["why"]?.stringValue, !why.isEmpty else {
        throw Invalid.malformed(
          file: file,
          detail: "case `\(name)` has no `why`. A case that does not say what it protects "
            + "cannot be judged when it fails, and gets deleted by whoever it inconveniences")
      }
      guard let given = raw["given"], let expect = raw["expect"] else {
        throw Invalid.malformed(file: file, detail: "case `\(name)` has no `given`/`expect`")
      }
      cases.append(Case(name: name, why: why, given: given, expect: expect))
    }

    return Conformance(file: file, concept: concept, source: source, cases: cases)
  }

  /// Runs every case through `subject` and compares what comes back to `expect`.
  ///
  /// - Returns: how many cases ran, so a driver can assert it rather than assume it.
  @discardableResult
  public func run(
    sourceLocation: SourceLocation = #_sourceLocation,
    _ subject: (JSONValue) throws -> JSONValue
  ) -> Int {
    for testCase in cases {
      let actual: JSONValue
      do {
        actual = try subject(testCase.given)
      } catch {
        Issue.record(
          Comment(rawValue: report(testCase, actual: nil, error: error)),
          sourceLocation: sourceLocation)
        continue
      }
      guard actual != testCase.expect else { continue }
      Issue.record(
        Comment(rawValue: report(testCase, actual: actual, error: nil)),
        sourceLocation: sourceLocation)
    }
    return cases.count
  }

  /// What a reader sees when a case fails.
  ///
  /// The `why` is in here on purpose: a failure is the one moment someone needs to know
  /// what the case was protecting, and it is the moment they are least inclined to go
  /// looking for it.
  private func report(_ testCase: Case, actual: JSONValue?, error: Error?) -> String {
    var lines = [
      "\(file) · \(testCase.name)",
      "  why:      \(testCase.why)",
      "  given:    \(testCase.given.rendered)",
      "  expected: \(testCase.expect.rendered)",
    ]
    if let actual {
      lines.append("  actual:   \(actual.rendered)")
    }
    if let error {
      lines.append("  the driver could not run this case: \(error)")
      lines.append(
        "  That is a corpus or driver problem rather than a failing behavior - the case "
          + "was never evaluated.")
    }
    if source == .ported {
      lines.append(
        "  This corpus is `ported`, so it is trusted only as far as the implementation it "
          + "came from. If this expectation is the thing that is wrong, fix it from a "
          + "recording or from the dependency's source - never by matching whichever "
          + "implementation is in front of you.")
    }
    return lines.joined(separator: "\n")
  }

  /// Walks up from a driver's own path to the checkout root.
  ///
  /// By path rather than by bundle, so that reading a case and running it are the same
  /// file - the same reason the snapshot helper does it.
  private static func repositoryRoot(from driverFile: String) -> URL {
    var directory = URL(fileURLWithPath: driverFile).deletingLastPathComponent()
    while directory.path != "/" {
      if FileManager.default.fileExists(
        atPath: directory.appendingPathComponent("Package.swift").path)
      {
        return directory
      }
      directory = directory.deletingLastPathComponent()
    }
    return URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
  }
}
