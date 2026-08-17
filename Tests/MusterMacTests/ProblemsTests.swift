import AppKit
import Testing

@testable import MusterMac

// A config refused at 18:55 left a window running default keybindings all evening, and the
// only thing that ever mentioned it was a line in a log file. These are the decisions that
// stops that: what a dismissal hides, what it does not, and what happens when the same thing
// breaks twice.

@Suite("the roster says what is wrong with the window")
struct ProblemsTests {
  private func problem(
    _ key: String, _ severity: Problem.Severity = .error, _ detail: String = "something"
  ) -> Problem {
    Problem(key: key, severity: severity, detail: detail)
  }

  @Test("nothing wrong draws nothing, not an empty box")
  func nothingIsNothing() {
    // The common case by a wide margin, and the one worth protecting: before this existed the
    // roster was a list of agents, and it still has to look like one.
    #expect(ProblemsModel.display(problems: [], dismissed: []) == .nothing)
    #expect(ProblemsModel.display(problems: [], dismissed: ["config"]) == .nothing)
  }

  @Test("a problem nobody has waved away is shown, whole")
  func aProblemIsShown() {
    let refusal = problem("config", .error, "`resize_step` is 10, and it needs a unit now.")
    #expect(
      ProblemsModel.display(problems: [refusal], dismissed: []) == .raised([refusal]))
  }

  @Test("dismissing everything outstanding leaves a count, not silence")
  func dismissingLeavesACount() {
    // The whole point of the collapsed state. A dismissal is somebody saying "not now", and
    // reading it as "never mention this again" is how the original bug worked.
    let display = ProblemsModel.display(
      problems: [problem("config"), problem("daemon", .warning)],
      dismissed: ["config", "daemon"])
    #expect(display == .collapsed(count: 2, severity: .error))
  }

  @Test("the count is coloured by the worst thing behind it")
  func theCountTakesTheWorstSeverity() {
    // A red dot behind a yellow one would be a lie about what is waiting, and the collapsed
    // state is precisely the state where nobody can see which is which.
    let warningsOnly = ProblemsModel.display(
      problems: [problem("daemon", .warning), problem("renderer", .warning)],
      dismissed: ["daemon", "renderer"])
    #expect(warningsOnly == .collapsed(count: 2, severity: .warning))

    let mixed = ProblemsModel.display(
      problems: [problem("daemon", .warning), problem("config", .error)],
      dismissed: ["daemon", "config"])
    #expect(mixed == .collapsed(count: 2, severity: .error))
  }

  @Test("a problem arriving after a dismissal is shown, and only it")
  func aNewProblemInterrupts() {
    // Dismissing a stale daemon must not silence a config refusal that turns up afterwards.
    // Showing only the undismissed one is what keeps the box from re-announcing what somebody
    // already waved away in the same breath.
    let fresh = problem("config", .error, "resize_step needs a unit")
    let display = ProblemsModel.display(
      problems: [problem("daemon", .warning), fresh], dismissed: ["daemon"])
    #expect(display == .raised([fresh]))
  }

  @Test("a problem that cleared and came back is shown again")
  func aProblemComingBackIsShownAgain() {
    // Fixing a config and breaking it a second time should look like the second time it
    // happened. Without this, one dismissal buys silence for every future occurrence of the
    // same condition - which is the failure mode this whole surface exists to end.
    let dismissed: Set<String> = ["config"]
    let whileFixed = ProblemsModel.retained(dismissed: dismissed, outstanding: [])
    #expect(whileFixed.isEmpty)

    let again = problem("config", .error, "divider is not a colour")
    #expect(ProblemsModel.display(problems: [again], dismissed: whileFixed) == .raised([again]))
  }

  @Test("a dismissal survives while its problem does")
  func aDismissalSurvivesItsProblem() {
    // The other side of the rule above: somebody who waved away a config they are still editing
    // should not have it thrown back at them by an unrelated problem arriving.
    let retained = ProblemsModel.retained(
      dismissed: ["config"], outstanding: [problem("config"), problem("daemon", .warning)])
    #expect(retained == ["config"])
  }

  @Test("an unknown severity is read as the worse one")
  func anUnknownSeverityIsAnError() {
    // Under-reporting is the failure this exists to fix, so a spelling this shell does not know
    // is treated as the one that gets somebody's attention rather than the one that does not.
    #expect(Problem.Severity("error") == .error)
    #expect(Problem.Severity("warning") == .warning)
    #expect(Problem.Severity("catastrophe") == .error)
    #expect(Problem.Severity("") == .error)
  }

  @MainActor
  @Test("nothing wrong takes none of the roster's height")
  func nothingTakesNoRoom() {
    // The layout half of "draws nothing". A zero-height area is what lets the list fill the
    // sidebar exactly as it did before any of this existed.
    let view = ProblemsView(frame: NSRect(x: 0, y: 0, width: 220, height: 100))
    view.show(.nothing)
    #expect(view.height(forWidth: 220) == 0)
    #expect(view.isHidden)
  }

  @MainActor
  @Test("a raised problem takes room, and a dismissed one takes less")
  func raisedTakesMoreRoomThanCollapsed() {
    let view = ProblemsView(frame: NSRect(x: 0, y: 0, width: 220, height: 100))
    let long = String(repeating: "a config refusal that runs on. ", count: 6)
    view.show(.raised([problem("config", .error, long)]))
    let raised = view.height(forWidth: 220)
    view.show(.collapsed(count: 1, severity: .error))
    let collapsed = view.height(forWidth: 220)
    #expect(raised > collapsed)
    #expect(collapsed > 0)
    #expect(!view.isHidden)
  }

  @MainActor
  @Test("a pathological message cannot take the window")
  func aLongMessageIsCapped() {
    // The roster's job is listing agents. A problem carrying a thousand words has to be
    // readable without becoming the sidebar, which is why the box caps and the tooltip keeps
    // the rest.
    let view = ProblemsView(frame: NSRect(x: 0, y: 0, width: 220, height: 2000))
    view.show(.raised([problem("config", .error, String(repeating: "wordy. ", count: 400))]))
    #expect(view.height(forWidth: 220) < 400)
  }
}
