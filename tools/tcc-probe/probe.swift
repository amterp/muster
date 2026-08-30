import Foundation

// Muster's process arrangement in miniature, so macOS can be asked who it holds responsible
// for a pane's program - and, since the two are different questions, whether that pane's own
// protected request then succeeds.
//
// One binary playing every part, because the shape is what is being measured and not any of
// Muster's code: an app started by Launch Services, a daemon it spawns that outlives it, and a
// pane's program under the daemon. Nothing here talks to Muster, herdr or libghostty - a probe
// that did would be measuring three dependencies rather than the operating system.
//
// Four arrangements of the same shape, differing only in what the daemon is and who starts it.
// `child` is what Muster does today. The other three are the candidates on kan a_29i4bxafd:
//
//   child        a bare binary, spawned by the app                     (today)
//   bundled      an app bundle's executable, spawned by the app        (posix_spawn, new path)
//   opened       the same, started through Launch Services             (`open -n -a`)
//   launchagent  a bare binary, started by a per-user launchd job      (the card's option)
//
// What it reads is `responsibility_get_pid_responsible_for_pid`, which is the call TCC itself
// uses to decide which app a permission prompt names. It is private, and asked about *itself*
// rather than about another process on purpose: unentitled, it answers other pids by handing
// the pid back, which reads exactly like "this process is its own responsible process" and is
// the trap this probe was nearly caught by. Every line below is a process asking about itself,
// where the answer is always the true one.
//
// Findings live in docs/observations/macos-26.4.1.md and the transcript in corpus/macos-26.4.1/.

typealias ResponsibleFor = @convention(c) (pid_t) -> pid_t
// RTLD_DEFAULT. The symbol lives in libsystem and is resolved at runtime rather than linked,
// so a macOS that drops it produces a line saying so instead of a binary that will not launch.
let symbol = dlsym(
  UnsafeMutableRawPointer(bitPattern: -2), "responsibility_get_pid_responsible_for_pid")
let responsible = symbol.map { unsafeBitCast($0, to: ResponsibleFor.self) }

let role = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "app"
// The scratch directory every part is told about rather than works out. Launch Services and
// launchd both start a process with an environment of their own, so nothing set out here
// reaches the arms that go through them; an argument reaches all four.
let root = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "."
let arrangement = CommandLine.arguments.count > 3 ? CommandLine.arguments[3] : "-"
// This machine's default gateway, worked out by the driver. It is the control for the
// multicast line below, and only a shell can ask for it without a route-table parser here.
let gateway = CommandLine.arguments.count > 4 ? CommandLine.arguments[4] : ""

/// One line of the transcript, appended to a file all four arrangements share.
///
/// A file rather than stderr, which is what an earlier version used: `open` hands its child
/// launchd's descriptors, so the two arms that go through Launch Services and launchd would
/// have written their answers nowhere. Opened per line with `O_APPEND`, so concurrent arms
/// interleave rather than overwrite - the lines are far below `PIPE_BUF`, which is what makes
/// an append atomic.
///
/// Wall clock rather than seconds since this process started, because four arrangements start
/// at four different moments and the transcript is read as one timeline. The driver sorts.
func write(_ moment: String, _ answer: String) {
  let now = DateFormatter()
  now.dateFormat = "HH:mm:ss.SSS"
  let padded = arrangement.padding(toLength: 12, withPad: " ", startingAt: 0)
  let where_ = moment.padding(toLength: 30, withPad: " ", startingAt: 0)
  let line = "\(now.string(from: Date()))  \(padded) \(where_) \(answer)\n"
  let handle = Darwin.open("\(root)/transcript.raw", O_WRONLY | O_APPEND | O_CREAT, 0o644)
  guard handle >= 0 else { return }
  _ = line.withCString { Darwin.write(handle, $0, strlen($0)) }
  close(handle)
}

/// Who macOS holds responsible for this process, asked about `getpid()` and nothing else.
///
/// When the answer is this process, it also says what a prompt charged to it would be headed:
/// macOS names the responsible process's bundle, so a process that is its own responsible
/// process and has no bundle is one nothing can put a name to. That is the difference between
/// the two arrangements that both make attribution consistent.
func report(_ moment: String) {
  let charged = responsible.map { $0(getpid()) }
  let answer = charged.map(String.init) ?? "unavailable"
  var line = "pid=\(getpid()) ppid=\(getppid()) responsible=\(answer)"
  if charged == getpid() {
    let named = Bundle.main.bundleIdentifier.map { identifier in
      let name = Bundle.main.infoDictionary?["CFBundleName"] as? String ?? "unnamed"
      return "\(name) (\(identifier))"
    }
    line += " prompts-say=\(named ?? "nothing - no bundle")"
  }
  write(moment, line)
}

/// One UDP datagram, and what happened to it.
func datagram(to address: String, port: UInt16) -> String {
  let descriptor = socket(AF_INET, SOCK_DGRAM, 0)
  guard descriptor >= 0 else { return "socket() failed errno=\(errno)" }
  defer { close(descriptor) }

  var destination = sockaddr_in()
  destination.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
  destination.sin_family = sa_family_t(AF_INET)
  destination.sin_port = port.bigEndian
  destination.sin_addr.s_addr = inet_addr(address)

  let payload = [UInt8](repeating: 0, count: 12)
  let size = socklen_t(MemoryLayout<sockaddr_in>.size)
  let sent = withUnsafePointer(to: &destination) { pointer in
    pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { target in
      sendto(descriptor, payload, payload.count, 0, target, size)
    }
  }
  guard sent < 0 else { return "sent" }
  let code = errno
  return "REFUSED errno=\(code) \(String(cString: strerror(code)))"
}

/// Whether this process may reach the local network, asked by doing it - and beside it, the
/// control that says what the answer means.
///
/// Local Network is the protected request whose denial is unrecognisable as one. It arrives as
/// `EHOSTUNREACH` on the multicast send rather than as a prompt or a permission error, which is
/// what sent the report on kan a_29i4bxafd chasing interfaces and `IP_MULTICAST_IF` for an
/// afternoon. `dns-sd` keeps working throughout and confirms the wrong thing, because it asks
/// mDNSResponder rather than sending multicast itself.
///
/// So the unicast line is not decoration. Same process, same instant, same LAN: if the datagram
/// to the gateway goes and the one to the mDNS group does not, then routing is not what refused
/// it. Without that line a reader has only an error that names routing.
///
/// It is also the one thing in this probe that leaves the machine.
func network() {
  let multicast = datagram(to: "224.0.0.251", port: 5353)
  let unicast = gateway.isEmpty ? "not asked" : datagram(to: gateway, port: 5353)
  write("local network", "multicast=\(multicast)  unicast-to-gateway=\(unicast)")
}

/// A part of the arrangement, run as a child of this one.
func spawn(_ executable: String, _ role: String, _ arrangement: String) {
  let child = Process()
  child.executableURL = URL(fileURLWithPath: executable)
  child.arguments = [role, root, arrangement, gateway]
  try? child.run()
}

/// The same, but through Launch Services rather than by forking.
///
/// `-n` because Launch Services would otherwise activate an instance that is already running
/// instead of starting the one being measured, and this arm exists precisely to watch a fresh
/// process come up.
func launchServices(_ bundle: String, _ role: String, _ arrangement: String) {
  let child = Process()
  child.executableURL = URL(fileURLWithPath: "/usr/bin/open")
  child.arguments = ["-n", "-a", bundle, "--args", role, root, arrangement, gateway]
  try? child.run()
}

// Where each part of each arrangement lives. The driver put them here.
let bare = "\(root)/probe-bare"
let spawnedBundle = "\(root)/ProbeSpawned.app"
let openedBundle = "\(root)/ProbeOpened.app"

// The waits are what the measurement is about rather than a convenience: the question is what
// changes when the app quits, so each part has to still be alive on the far side of that.
switch role {
case "daemon":
  report("daemon, app alive")
  // A bare binary, always. A pane runs whatever agent somebody runs - `claude`, `python`, a
  // shell - and never an app bundle, so spawning this arm's own executable would quietly give
  // every pane a bundled identity that no real pane has.
  spawn(bare, "pane-early", arrangement)
  Thread.sleep(forTimeInterval: 12)
  report("daemon, app gone")
  // A pane made by a later Muster is still made by this daemon, so this is that case.
  spawn(bare, "pane-late", arrangement)
  Thread.sleep(forTimeInterval: 12)
case "pane-early":
  report("pane made while app alive")
  network()
  Thread.sleep(forTimeInterval: 12)
  report("same pane, app now gone")
  network()
  Thread.sleep(forTimeInterval: 10)
case "pane-late":
  report("pane made after app gone")
  network()
  Thread.sleep(forTimeInterval: 8)
case "here":
  // No arrangement around it: this is whatever shell ran `probe --here`, asked the same two
  // questions the modelled panes are asked.
  report("this shell")
  network()
default:
  report("app")
  spawn(bare, "daemon", "child")
  spawn("\(spawnedBundle)/Contents/MacOS/Probe", "daemon", "bundled")
  launchServices(openedBundle, "daemon", "opened")
  Thread.sleep(forTimeInterval: 5)
}
