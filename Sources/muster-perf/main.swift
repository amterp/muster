import Foundation
import MusterCore
import MusterHerdr
import MusterPerf
import MusterVT

// muster-perf: what one byte, one keystroke and one event cost.
//
// "Fast is a feature" budgets work by frequency times cardinality, which only means
// something if the per-unit costs are known and stay known. So this measures per unit, not
// per run, and compares against a recorded baseline rather than against a feeling.
//
// Out of the default gate on purpose (docs/testing.md: a functional green is never a
// performance claim). A timing assertion inside the gate makes the gate flaky, and a flaky
// gate gets ignored.

let usage = """
  usage: muster-perf [--record] [--baseline <path>] [--corpus <path>] [--tolerance <x>]

  Measures the per-unit costs on Muster's hot paths and compares them to a baseline.

    --record       write this run's numbers as the new baseline instead of judging
    --baseline     where the baseline lives (default perf/baseline.json)
    --corpus       recorded frames to replay (default corpus)
    --tolerance    how many times the recorded cost still passes (default 2.0)
  """

var recording = false
var baselinePath = "perf/baseline.json"
var corpusPath = "corpus"
var tolerance = 2.0

var arguments = Array(CommandLine.arguments.dropFirst())
while let argument = arguments.first {
  arguments.removeFirst()
  func value(_ name: String) -> String {
    guard let next = arguments.first else {
      FileHandle.standardError.write(Data("muster-perf: \(name) needs a value\n".utf8))
      exit(2)
    }
    arguments.removeFirst()
    return next
  }
  switch argument {
  case "--record": recording = true
  case "--baseline": baselinePath = value("--baseline")
  case "--corpus": corpusPath = value("--corpus")
  case "--tolerance": tolerance = Double(value("--tolerance")) ?? tolerance
  case "-h", "--help":
    print(usage)
    exit(0)
  default:
    FileHandle.standardError.write(
      Data("muster-perf: unknown argument \(argument)\n\(usage)\n".utf8))
    exit(2)
  }
}

// MARK: - The corpus these replay

/// Every recorded frame stream, concatenated.
///
/// Recorded from a real daemon rather than generated, so the shape being timed is the
/// shape that actually arrives: one 35 KB attach repaint followed by small diffs. A
/// synthetic stream of uniform frames would measure a workload Muster never sees.
func recordedFrameStreams(in corpus: String) -> [Data] {
  let root = URL(fileURLWithPath: corpus)
  guard
    let versions = try? FileManager.default.contentsOfDirectory(
      at: root, includingPropertiesForKeys: nil)
  else { return [] }

  return
    versions
    .flatMap { version -> [URL] in
      (try? FileManager.default.contentsOfDirectory(at: version, includingPropertiesForKeys: nil))?
        .map { $0.appendingPathComponent("frames.ndjson") } ?? []
    }
    .filter { FileManager.default.fileExists(atPath: $0.path) }
    .sorted { $0.path < $1.path }
    .compactMap { try? Data(contentsOf: $0) }
}

let streams = recordedFrameStreams(in: corpusPath)
guard !streams.isEmpty else {
  FileHandle.standardError.write(
    Data(
      """
      muster-perf: no recorded frames under \(corpusPath).
      The per-byte budgets replay real daemon output, so without the corpus this run \
      would report nothing while exiting 0 - which reads as a pass. Run from the repo \
      root, or pass --corpus.

      """.utf8))
  exit(2)
}

let wireBytes = streams.reduce(0) { $0 + $1.count }
let decodedFrames = streams.flatMap { stream -> [PaneFrame] in
  var decoder = FrameDecoder()
  return decoder.consume(stream).compactMap { event in
    if case .frame(let frame) = event { return frame }
    return nil
  }
}
let decodedBytes = decodedFrames.reduce(0) { $0 + $1.bytes.count }

var costs: [Cost] = []

// MARK: - Per byte: the data plane

// Two separate budgets that a single "frame cost" would hide. Decoding scales with the
// wire bytes the bridge reads; VT parsing scales with the ANSI those unwrap to, which is
// smaller and far more expensive per byte.

costs.append(
  Benchmark.measure(name: "frame.decode", unit: "ns/byte", unitsPerIteration: wireBytes) {
    for stream in streams {
      var decoder = FrameDecoder()
      let events = decoder.consume(stream)
      blackHole(events.count)
    }
  })

costs.append(
  Benchmark.measure(
    name: "frame.vt_parse", unit: "ns/byte", unitsPerIteration: max(1, decodedBytes),
    iterations: 20
  ) {
    // A fresh terminal per iteration: replaying a repaint into a terminal that already
    // holds it measures a different, cheaper thing.
    guard let terminal = try? Terminal(columns: 80, rows: 24) else { return }
    for frame in decodedFrames { terminal.write(frame.bytes) }
  })

// MARK: - Per intent: the input path

// What every keystroke pays before it reaches a socket. Small by construction - the
// encoder is built once and holds the pane's modes - and worth a standing number because
// this is the one path where a regression is felt rather than measured.

let encoder = try? KeyEncoder(profile: .unknownPane)
let keymap = Keymap()
let keystrokes: [KeyEvent] = [
  KeyEvent(
    action: .press, key: .keyH, modifiers: [], consumedModifiers: [], text: "h",
    unshiftedCodepoint: nil, isComposing: false),
  KeyEvent(
    action: .press, key: .keyC, modifiers: .control, consumedModifiers: [], text: "c",
    unshiftedCodepoint: nil, isComposing: false),
  KeyEvent(
    action: .press, key: .enter, modifiers: .shift, consumedModifiers: [], text: "\r",
    unshiftedCodepoint: nil, isComposing: false),
  KeyEvent(
    action: .press, key: .backspace, modifiers: .`super`, consumedModifiers: [], text: "\u{7f}",
    unshiftedCodepoint: nil, isComposing: false),
]

if let encoder {
  costs.append(
    Benchmark.measure(
      name: "input.encode", unit: "ns/key", unitsPerIteration: keystrokes.count * 100,
      iterations: 100
    ) {
      for _ in 0..<100 {
        for key in keystrokes {
          switch keymap.resolve(key) {
          case .text(let bytes): blackHole(bytes.count)
          case .unbound: blackHole(((try? encoder.encode(key)) ?? []).count)
          case .serverEncoded, .action: break
          }
        }
      }
    })
} else {
  FileHandle.standardError.write(
    Data("muster-perf: skipping input.encode - the key encoder would not build\n".utf8))
}

// MARK: - Per event: the control plane
//
// The mirror's application cost belongs here, and the benchmark lands with the mirror
// (kan a_26DAm1Zt0). Declared as an absence rather than left out, because a budget nobody
// wrote down reads the same as a budget nobody exceeded.

// MARK: - Report

print(Report.table(costs))
print("")

if recording {
  let baseline = Baseline(
    recorded: Date.ISO8601FormatStyle(includingFractionalSeconds: false).format(Date()),
    machine: machineDescription(), costs: costs)
  let encoderJSON = JSONEncoder()
  encoderJSON.outputFormatting = [.prettyPrinted, .sortedKeys]
  guard let data = try? encoderJSON.encode(baseline) else {
    FileHandle.standardError.write(Data("muster-perf: could not encode the baseline\n".utf8))
    exit(1)
  }
  try? FileManager.default.createDirectory(
    at: URL(fileURLWithPath: baselinePath).deletingLastPathComponent(),
    withIntermediateDirectories: true)
  do {
    try (data + Data("\n".utf8)).write(to: URL(fileURLWithPath: baselinePath))
  } catch {
    FileHandle.standardError.write(Data("muster-perf: \(baselinePath): \(error)\n".utf8))
    exit(1)
  }
  print("recorded \(costs.count) costs to \(baselinePath)")
  exit(0)
}

guard let data = try? Data(contentsOf: URL(fileURLWithPath: baselinePath)),
  let baseline = try? JSONDecoder().decode(Baseline.self, from: data)
else {
  FileHandle.standardError.write(
    Data(
      """
      muster-perf: no baseline at \(baselinePath), so nothing gates these numbers.
      Record one with `muster-perf --record` once the machine is quiet.

      """.utf8))
  exit(2)
}

if baseline.machine != machineDescription() {
  print(
    "note: baseline recorded on \(baseline.machine), running on \(machineDescription()). "
      + "A cross-machine comparison explains a failure that is not a regression.")
}

let comparison = compare(measured: costs, against: baseline, tolerance: tolerance)
print(Report.verdict(comparison, tolerance: tolerance))
exit(comparison.isClean ? 0 : 1)

// MARK: -

/// Keeps a result from being optimized away without measuring the keeping.
@inline(never)
func blackHole(_ value: Int) {
  if value == Int.min { fatalError("unreachable") }
}

func machineDescription() -> String {
  var info = utsname()
  uname(&info)
  let machine = withUnsafeBytes(of: &info.machine) { buffer in
    String(cString: buffer.baseAddress!.assumingMemoryBound(to: CChar.self))
  }
  return "\(machine)-\(ProcessInfo.processInfo.operatingSystemVersionString)"
}
