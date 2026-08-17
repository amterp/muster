import Foundation

// Muster's process arrangement in miniature, so macOS can be asked who it holds responsible
// for a pane's program.
//
// One binary playing three parts, because the shape is what is being measured and not any of
// Muster's code: an app started by Launch Services, a daemon it spawns that outlives it, and a
// pane's program under the daemon. Nothing here talks to Muster, herdr or libghostty - a probe
// that did would be measuring three dependencies rather than the operating system.
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

let started = Date()

/// One line of the transcript. On stderr because `open --stderr` is the only way to catch the
/// output of an app Launch Services starts, and all three parts inherit it.
func report(_ label: String) {
  let at = String(format: "%5.1fs", Date().timeIntervalSince(started))
  let answer = responsible.map { String($0(getpid())) } ?? "unavailable"
  let padded = label.padding(toLength: 34, withPad: " ", startingAt: 0)
  FileHandle.standardError.write(
    Data("\(at)  \(padded) pid=\(getpid()) ppid=\(getppid()) responsible=\(answer)\n".utf8))
}

func spawn(_ role: String) {
  let child = Process()
  child.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
  child.arguments = [role]
  try! child.run()
}

// The waits are what the measurement is about rather than a convenience: the question is what
// changes when the app quits, so each part has to still be alive on the far side of that.
switch CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "app" {
case "daemon":
  report("daemon, app alive")
  spawn("pane-early")
  Thread.sleep(forTimeInterval: 12)
  report("daemon, app gone")
  // A pane made by a later Muster is still made by this daemon, so this is that case.
  spawn("pane-late")
  Thread.sleep(forTimeInterval: 12)
case "pane-early":
  report("pane made while app alive")
  Thread.sleep(forTimeInterval: 12)
  report("same pane, app now gone")
  Thread.sleep(forTimeInterval: 10)
case "pane-late":
  report("pane made after app gone")
  Thread.sleep(forTimeInterval: 8)
default:
  report("app")
  spawn("daemon")
  Thread.sleep(forTimeInterval: 5)
}
