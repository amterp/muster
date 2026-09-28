import Foundation
import Testing

@testable import MusterMac

// Where a run's log goes. An isolated run that writes beside the person's own mixes two runs'
// records in one directory, and two isolated runs at once fight over `latest.jsonl`.

@Test func aRunLogsBesideThePersonsOtherRuns() {
  #expect(
    logDirectory(environment: ["HOME": "/home/a"])
      == FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Logs/muster", isDirectory: true))
}

@Test func musterHomeMovesTheRunLog() {
  #expect(
    logDirectory(environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"])
      == URL(fileURLWithPath: "/scratch/logs", isDirectory: true))
}

@Test func anEmptyMusterHomeMovesNothing() {
  // The same reading `musterHome(environment:)` gives it: set and empty is unset.
  #expect(
    logDirectory(environment: ["MUSTER_HOME": "", "HOME": "/home/a"])
      == FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Logs/muster", isDirectory: true))
}
