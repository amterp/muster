import Testing

@testable import MusterMac

@Suite("what somebody is told before the one thing Muster cannot undo")
struct QuitSummaryTests {
  static func machine(
    _ daemon: String, host: String = "", panes: Int, directories: [String] = [],
    started: Bool = true
  ) -> Core.Machine {
    Core.Machine(
      daemon: daemon, host: host, socket: "/tmp/\(daemon).sock", startedByMuster: started,
      panes: panes, directories: directories)
  }

  @MainActor
  @Test("the heading counts what would end, across every machine")
  func headingCounts() {
    let machines = [
      QuitSummaryTests.machine("local", panes: 2),
      QuitSummaryTests.machine("devenv", host: "dev", panes: 3),
    ]
    #expect(QuitSummary.question(machines: machines) == "Quit and close 5 panes?")
    #expect(
      QuitSummary.question(machines: [QuitSummaryTests.machine("local", panes: 1)])
        == "Quit and close 1 pane?")
  }

  @MainActor
  @Test("it names the directories, because that is what somebody recognises")
  func bodyNamesDirectories() {
    // A count is a number people agree to. `~/src/nook` is the thing that makes somebody stop,
    // and stopping is the entire purpose of this sheet.
    let body = QuitSummary.body(machines: [
      QuitSummaryTests.machine("local", panes: 2, directories: ["~/src/muster", "~/src/nook"])
    ])
    #expect(body.contains("~/src/muster, ~/src/nook"))
    #expect(body.contains("local on this machine: 2 panes"))
  }

  @MainActor
  @Test("a session Muster walked into is called out as one")
  func adoptedIsNamed() {
    // The hazard this whole card came from: a Muster launched today adopted a daemon started
    // eighteen hours earlier holding somebody's working agent, because `ensure_running`
    // attaches to whatever answers. Ending that is a different decision from ending a session
    // this window made, and the sheet has to say which it is.
    let adopted = QuitSummary.body(machines: [
      QuitSummaryTests.machine("local", panes: 1, started: false)
    ])
    #expect(adopted.contains("already running when the window opened"))

    let started = QuitSummary.body(machines: [QuitSummaryTests.machine("local", panes: 1)])
    #expect(!started.contains("already running when the window opened"))
  }

  @MainActor
  @Test("it says what a pane's process is actually asked, and what the other way out does")
  func bodySaysWhatHappens() {
    // Measured rather than guessed: `server.stop` gives a pane's process a catchable SIGHUP
    // and a window to act in, and Claude Code exits within about 300ms and puts the terminal
    // back on its way out. So "asked to stop" is true and "killed" would not be - but neither
    // is "finishes what it was doing", and the sentence has to carry both halves.
    let body = QuitSummary.body(machines: [QuitSummaryTests.machine("local", panes: 1)])
    #expect(body.contains("asked to stop"))
    #expect(body.contains("does not finish"))
    #expect(body.contains("Quitting normally leaves all of this running."))
  }

  @MainActor
  @Test("a window attached to nothing says so rather than asking about nothing")
  func nothingToClose() {
    let body = QuitSummary.body(machines: [])
    #expect(body.contains("no machines"))
    #expect(QuitSummary.question(machines: []) == "Quit and close sessions?")
  }
}
