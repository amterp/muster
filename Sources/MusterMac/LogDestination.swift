import Foundation

// Where the logs go, which is the one part of logging that is an OS question. The core
// writes the records; this file decides they belong in ~/Library/Logs, and hands the path
// over at startup. Ports pick their own answer here and nothing below changes.

/// Opens this run's log file and points every process Muster spawns at it.
///
/// Returns the path, so startup can say where to look. Nil means logging is off, which is
/// what a release build does unless `MUSTER_LOG=1` asks otherwise: a log that records
/// what a person types is not something to switch on for them.
@discardableResult
public func startLogging() -> String? {
  let environment = ProcessInfo.processInfo.environment
  // An explicit file wins, so a bug report can be captured to a path of its own.
  if let path = environment["MUSTER_LOG_FILE"], !path.isEmpty {
    return path
  }

  #if DEBUG
    let wanted = environment["MUSTER_LOG"] != "0"
  #else
    let wanted = environment["MUSTER_LOG"] == "1"
  #endif
  guard wanted else { return nil }

  let directory = FileManager.default.homeDirectoryForCurrentUser
    .appendingPathComponent("Library/Logs/muster", isDirectory: true)
  guard
    (try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true))
      != nil
  else { return nil }

  let path =
    directory
    .appendingPathComponent("muster-\(fileTimestamp())-\(getpid()).jsonl").path

  // Inherited by every bridge this app spawns, which is what puts both sides of a
  // keystroke in one timeline.
  setenv("MUSTER_LOG_FILE", path, 1)

  pointLatestAt(path, in: directory)
  pruneSessions(in: directory)
  return path
}

/// A stable name for the newest run.
///
/// The whole point of this file is answering "I just hit a bug" without first working out
/// which of forty files was that run.
private func pointLatestAt(_ path: String, in directory: URL) {
  let latest = directory.appendingPathComponent("latest.jsonl")
  try? FileManager.default.removeItem(at: latest)
  try? FileManager.default.createSymbolicLink(
    at: latest, withDestinationURL: URL(fileURLWithPath: path))
}

/// Keeps the newest runs and drops the rest.
///
/// Every launch writes a file and nothing else ever deletes one, so without this the
/// directory grows for as long as Muster is used.
private func pruneSessions(in directory: URL, keeping keep: Int = 20) {
  guard
    let names = try? FileManager.default.contentsOfDirectory(atPath: directory.path)
  else { return }
  let sessions = names.filter { $0.hasPrefix("muster-") && $0.hasSuffix(".jsonl") }.sorted()
  for name in sessions.dropLast(keep) {
    try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
  }
}

/// ISO order, no colons: sorts chronologically as text, and survives a filesystem.
private func fileTimestamp() -> String {
  let formatter = DateFormatter()
  formatter.dateFormat = "yyyy-MM-dd_HH-mm-ss"
  return formatter.string(from: Date())
}
