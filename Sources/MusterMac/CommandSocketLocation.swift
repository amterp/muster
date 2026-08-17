import Foundation

/// Where this window listens for requests from outside its own process.
///
/// Beside the arrangement and the pane names, because it is Muster's own state and nothing a
/// person should have to know about. An OS question, which is why it is answered here and handed
/// to the core at startup - the same division the log file and the state file already draw.
///
/// The name carries this process's pid, because two Musters are two windows: a caller has to be
/// able to reach the one it means, and a single fixed path would mean the second window to open
/// silently took the first one's callers.
///
/// The variable that overrides it is deliberately not the `MUSTER_SOCKET` a pane is given.
/// That one says where to dial and this one says where to listen, and one name for both would
/// mean a Muster launched from inside a Muster pane inherited the outer window's path and bound
/// it - taking over its endpoint and leaving it undriveable with nothing to show why.
public func commandSocketPath(
  environment: [String: String] = ProcessInfo.processInfo.environment,
  pid: Int32 = ProcessInfo.processInfo.processIdentifier
) -> String? {
  if let explicit = environment["MUSTER_COMMAND_SOCKET"] {
    // Empty is how a test or a script says "listen nowhere", the same spelling the arrangement
    // and the pane names use.
    return explicit.isEmpty ? nil : explicit
  }

  guard let home = musterHome(environment: environment) else { return nil }
  return home.appendingPathComponent("state/command-\(pid).sock").path
}

/// Removes endpoint sockets left behind by Musters that are gone.
///
/// A window that is killed - or crashes, or is stopped in a debugger - never unlinks its socket,
/// so without this the directory fills up with files that refuse every connection. That is not
/// merely untidy: finding the endpoint means trying the sockets that are there, and a caller
/// cannot tell "this Muster is dead" from "Muster is still starting up" without trying.
///
/// A pid that no longer exists is the test, and it is safe because the pid is in the name and a
/// live Muster is the process that named its own. `kill(pid, 0)` reports existence without
/// sending anything; EPERM counts as alive, since a process owned by somebody else is still a
/// process and not ours to clean up after.
public func sweepDeadCommandSockets(
  environment: [String: String] = ProcessInfo.processInfo.environment
) {
  guard let home = musterHome(environment: environment) else { return }
  let state = home.appendingPathComponent("state", isDirectory: true)
  let entries =
    (try? FileManager.default.contentsOfDirectory(at: state, includingPropertiesForKeys: nil))
    ?? []

  for entry in entries {
    let name = entry.lastPathComponent
    guard name.hasPrefix("command-"), name.hasSuffix(".sock") else { continue }
    let digits = name.dropFirst("command-".count).dropLast(".sock".count)
    guard let pid = pid_t(digits) else { continue }
    if kill(pid, 0) == 0 || errno == EPERM { continue }
    try? FileManager.default.removeItem(at: entry)
  }
}
