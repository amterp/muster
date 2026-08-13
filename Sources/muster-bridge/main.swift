import Darwin
import Foundation
import MusterCore
import MusterHerdr

// muster-bridge: one pane's frame stream, unwrapped into one surface.
//
// libghostty gives a surface no way to be fed bytes, so the only channel into it is the
// command it spawns (docs/observations/libghostty-9f9b8d1d.md section 2). This is that
// command. Its stdout is the surface's PTY, and its job is to turn herdr's JSON frame
// envelopes back into the ANSI they wrap.
//
// Output only, deliberately. The frames have already consumed the pane's terminal modes,
// so nothing here may encode input - that belongs where the modes live, in the daemon.
// The one thing this does write back is geometry.

let usage = """
  usage: muster-bridge <pane-id> [--control-socket <path>]

  Runs `herdr terminal session control <pane-id>` and unwraps its frames onto stdout.
  Sized from the PTY on stdout, which is the surface's own geometry.

  With --control-socket, dials that socket and relays whatever the app sends onto
  herdr's control stream verbatim - input and scroll. Without it, the pane renders but
  cannot be typed into.
  """

guard CommandLine.arguments.count == 2 || CommandLine.arguments.count == 4,
  CommandLine.arguments.count == 2 || CommandLine.arguments[2] == "--control-socket"
else {
  FileHandle.standardError.write(Data((usage + "\n").utf8))
  exit(2)
}
let paneID = CommandLine.arguments[1]
let controlSocketPath = CommandLine.arguments.count == 4 ? CommandLine.arguments[3] : nil

// The app names the file and every bridge it spawns inherits it, so one pane's whole
// story - keystroke leaves the app, arrives here, goes to herdr - reads in order.
Log.startFromEnvironment(process: "bridge:\(paneID)")

/// The surface's grid, read from the PTY libghostty gave us.
///
/// This is why resize needs no channel of its own: libghostty sizes the PTY from the
/// surface's pixels and font metrics, so asking the PTY is asking the surface.
func terminalSize() -> (cols: UInt16, rows: UInt16) {
  var ws = winsize()
  guard ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == 0, ws.ws_col > 0 else { return (80, 24) }
  return (ws.ws_col, ws.ws_row)
}

let initial = terminalSize()

let herdr = Process()
herdr.executableURL = URL(fileURLWithPath: "/usr/bin/env")
herdr.arguments = [
  "herdr", "terminal", "session", "control", paneID,
  "--cols", String(initial.cols), "--rows", String(initial.rows),
]
let toHerdr = Pipe()
let fromHerdr = Pipe()
herdr.standardInput = toHerdr
herdr.standardOutput = fromHerdr
herdr.standardError = FileHandle.standardError

Log.info(
  "bridge.start",
  [
    "pane": paneID, "cols": String(initial.cols), "rows": String(initial.rows),
    "control_socket": controlSocketPath ?? "(none)",
  ])

do {
  try herdr.run()
} catch {
  Log.error("bridge.herdr.failed", ["pane": paneID, "error": "\(error)"])
  FileHandle.standardError.write(
    Data(
      """
      muster-bridge: could not start herdr: \(error)
      This pane will render nothing. Check that herdr is on PATH and that the daemon \
      this process can see owns pane \(paneID).

      """.utf8))
  exit(1)
}

// herdr's control stream takes newline-delimited JSON on stdin. Resize is the only thing
// written here; input goes over the control plane, not through this pipe.
func send(_ message: [String: Any]) {
  guard let data = try? JSONSerialization.data(withJSONObject: message) else { return }
  toHerdr.fileHandleForWriting.write(data + Data("\n".utf8))
}

// SIGWINCH is delivered asynchronously, so it only sets a flag; a dispatch source does
// the work on a queue where allocating is safe.
let winchSource = DispatchSource.makeSignalSource(signal: SIGWINCH, queue: .main)
winchSource.setEventHandler {
  let size = terminalSize()
  send(["type": "terminal.resize", "cols": Int(size.cols), "rows": Int(size.rows)])
}
winchSource.resume()
signal(SIGWINCH, SIG_IGN)

/// Pumps decoded frames to the surface.
///
/// Its own type rather than top-level state because the read handler runs off the main
/// queue, and top-level code in a `main.swift` is main-actor isolated. `@unchecked` is
/// honest here: `FileHandle` serializes its readability callbacks, so the decoder has one
/// caller at a time.
final class FramePump: @unchecked Sendable {
  private var decoder = FrameDecoder()
  private let out = FileHandle.standardOutput
  /// Whether anything was ever painted, which is what separates a pane that ended from a
  /// pane that never began.
  private var rendered = false

  func consume(_ chunk: Data) {
    for event in decoder.consume(chunk) {
      switch event {
      case .frame(let frame):
        // An attach opens with a full repaint, so a surface never has to have seen the
        // start of the stream.
        if !rendered { Log.info("bridge.frame.first", ["bytes": String(frame.bytes.count)]) }
        rendered = true
        Log.trace("bridge.frame", ["bytes": String(frame.bytes.count)])
        out.write(frame.bytes)
      case .closed(let reason):
        finish(reason: reason)
      }
    }
  }

  /// Reports why the stream ended, and exits.
  ///
  /// herdr states its reason in the closing frame and this process is the only thing that
  /// ever sees it. Exiting silently made a mistyped pane id and a pane the user closed
  /// into the same event: an empty window and ghostty's own "failed to launch" box, which
  /// blames the command rather than naming the pane that does not exist.
  func finish(reason: String?) -> Never {
    let why = reason ?? "herdr gave no reason"
    Log.info("bridge.closed", ["pane": paneID, "reason": why, "rendered": String(rendered)])
    guard rendered else {
      Log.error(
        "bridge.attach.failed",
        [
          "pane": paneID, "reason": why,
          "impact": "this window stays empty for as long as it is open",
        ])
      // Nothing ever painted, so the attach itself failed. Non-zero because this is not
      // a session ending - it is a session that never started.
      FileHandle.standardError.write(
        Data(
          """
          muster-bridge: could not attach to pane \(paneID): \(why)
          This window will stay empty. Most often the pane id is wrong or its workspace \
          is gone; `herdr pane list` names the panes that exist right now.

          """.utf8))
      exit(1)
    }
    FileHandle.standardError.write(Data("muster-bridge: pane \(paneID) closed: \(why)\n".utf8))
    exit(0)
  }
}

let pump = FramePump()
fromHerdr.fileHandleForReading.readabilityHandler = { handle in
  let chunk = handle.availableData
  if chunk.isEmpty {
    // herdr hung up without a closing frame, which the protocol does not call for. Same
    // exit either way - it is what tells libghostty this pane's command is gone - but it
    // goes through the same reporting so the window never just stops.
    pump.finish(reason: "herdr's stream ended without a closing frame")
  }
  pump.consume(chunk)
}

// The app's end of the pane: whatever it sends is already herdr control-stream JSON, so
// relaying it is a copy. Keeping the bridge free of any vocabulary of its own is what
// lets the adapter stay in one place (architecture.md, the backend seam).
// Held for the life of the process: a released socket stops relaying.
let controlSocket = controlSocketPath.flatMap(ControlSocket.init(path:))
if let controlSocketPath {
  if let controlSocket {
    Log.info("bridge.control.dialed", ["path": controlSocketPath])
    controlSocket.relay { line in
      Log.debug(
        "bridge.relay",
        [
          "bytes": String(line.count),
          "line": Log.includesInput ? String(decoding: line, as: UTF8.self) : "",
        ])
      toHerdr.fileHandleForWriting.write(line)
    }
  } else {
    Log.error(
      "bridge.control.failed",
      [
        "path": controlSocketPath,
        "impact": "this pane renders but swallows every keystroke",
      ])
    FileHandle.standardError.write(
      Data(
        """
        muster-bridge: could not reach the app on \(controlSocketPath)
        This pane will render but swallow every keystroke, which otherwise looks like a \
        dead terminal rather than a broken channel. Usual cause: the app closed the \
        socket, or this bridge outlived the window that spawned it.

        """.utf8))
  }
}

// Stdin is the surface's PTY. Nothing reads it - input takes the control plane - but the
// line discipline would otherwise echo and buffer whatever libghostty writes there,
// painting over the frames we just rendered.
var raw = termios()
if tcgetattr(STDIN_FILENO, &raw) == 0 {
  cfmakeraw(&raw)
  tcsetattr(STDIN_FILENO, TCSANOW, &raw)
}

dispatchMain()
