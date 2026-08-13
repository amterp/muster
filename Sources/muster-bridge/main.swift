import Darwin
import Foundation
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

do {
  try herdr.run()
} catch {
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

  func consume(_ chunk: Data) {
    for event in decoder.consume(chunk) {
      switch event {
      case .frame(let frame):
        // An attach opens with a full repaint, so a surface never has to have seen the
        // start of the stream.
        out.write(frame.bytes)
      case .closed:
        exit(0)
      }
    }
  }
}

let pump = FramePump()
fromHerdr.fileHandleForReading.readabilityHandler = { handle in
  let chunk = handle.availableData
  if chunk.isEmpty {
    // herdr hung up. Exiting is what tells libghostty this pane's command is gone; the
    // surface keeps its last painted frame until then.
    exit(0)
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
    controlSocket.relay { line in
      toHerdr.fileHandleForWriting.write(line)
    }
  } else {
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
