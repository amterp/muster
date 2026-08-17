import Foundation
import Testing

@testable import MusterMac

// Where this window listens for requests from outside its own process. Worth pinning because
// every wrong answer is a CLI that reaches the wrong window or no window at all - and the
// symptom of reaching the wrong one is a pane appearing on somebody else's screen.

@Test func theEndpointCarriesThePidSoTwoWindowsAreTwoEndpoints() {
  #expect(
    commandSocketPath(environment: ["HOME": "/home/a"], pid: 4321)
      == "/home/a/.muster/state/command-4321.sock")
}

@Test func musterHomeMovesTheEndpoint() {
  #expect(
    commandSocketPath(environment: ["MUSTER_HOME": "/scratch", "HOME": "/home/a"], pid: 7)
      == "/scratch/state/command-7.sock")
}

@Test func anEmptyExplicitEndpointMeansListenNowhere() {
  #expect(commandSocketPath(environment: ["MUSTER_COMMAND_SOCKET": "", "HOME": "/home/a"]) == nil)
}

@Test func thePaneVariableDoesNotDecideWhereToListen() {
  // The variable a pane is given says where to dial. If it also said where to listen, a Muster
  // launched from inside a Muster pane would inherit the outer window's path, bind it, and take
  // over its endpoint - leaving the outer window undriveable with nothing to show why.
  #expect(
    commandSocketPath(
      environment: ["MUSTER_SOCKET": "/somebody/elses.sock", "HOME": "/home/a"], pid: 9)
      == "/home/a/.muster/state/command-9.sock")
}

// Sweeping sockets left by Musters that are gone. A killed window never unlinks its own, and
// finding the endpoint means trying the ones that are there.

@Test func aSocketWhosePidIsGoneIsSweptAndOneStillRunningIsLeft() throws {
  let home = URL(fileURLWithPath: NSTemporaryDirectory())
    .appendingPathComponent("muster-sweep-\(ProcessInfo.processInfo.processIdentifier)")
  let state = home.appendingPathComponent("state", isDirectory: true)
  try FileManager.default.createDirectory(at: state, withIntermediateDirectories: true)
  defer { try? FileManager.default.removeItem(at: home) }

  // This process is alive by definition, which is what makes it the one that must survive.
  let alive = state.appendingPathComponent(
    "command-\(ProcessInfo.processInfo.processIdentifier).sock")
  // Above the pid ceiling, so no process can hold it. A number that could be live would make
  // this test fail on whichever machine happened to have that pid.
  let dead = state.appendingPathComponent("command-999999999.sock")
  // Not an endpoint at all, and the sweep has no business touching it.
  let arrangement = state.appendingPathComponent("window.toml")
  for file in [alive, dead, arrangement] {
    try Data().write(to: file)
  }

  sweepDeadCommandSockets(environment: ["MUSTER_HOME": home.path])

  #expect(FileManager.default.fileExists(atPath: alive.path))
  #expect(!FileManager.default.fileExists(atPath: dead.path))
  #expect(FileManager.default.fileExists(atPath: arrangement.path))
}
