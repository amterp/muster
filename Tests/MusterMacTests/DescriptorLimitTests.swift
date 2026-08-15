import Testing

@testable import MusterMac

// The arithmetic, not the syscall. Getting this wrong costs a pane that renders nothing at
// the fifteenth split - long after launch, and with no error attached to it.

/// `RLIMIT_NOFILE`'s hard limit as launchd sets it, spelled out because Darwin's
/// `RLIM_INFINITY` is a macro rather than a constant Swift can see.
private let unlimited: UInt64 = (1 << 63) - 1

@Test("a shell's limit is already enough, so nothing is asked for")
func aGenerousLimitIsLeftAlone() {
  // What a developer's terminal reports. A setrlimit here would be a syscall that changes
  // nothing, and a log line claiming a limit was raised when it was not.
  #expect(DescriptorLimit.request(soft: 1_048_575, hard: 1_048_575) == nil)
}

@Test("launchd's default is raised to what a full window needs")
func theDockLimitIsRaised() {
  // 256 soft against an unlimited hard is what a double-clicked app inherits, and it is the
  // only case this whole file exists for.
  #expect(DescriptorLimit.request(soft: 256, hard: unlimited) == DescriptorLimit.wanted)
}

@Test("a hard ceiling below what we want is what gets asked for")
func aLowCeilingIsNotExceeded() {
  // Asking past the hard limit fails outright, which would leave the process on 256 rather
  // than on the 1024 it could have had.
  #expect(DescriptorLimit.request(soft: 256, hard: 1024) == 1024)
}

@Test("a soft limit already at the ceiling asks for nothing")
func nothingIsAskedForTwice() {
  // Distinct from having enough: this process cannot have more, and a refused setrlimit
  // would be reported as a problem when there is nothing anyone could do about it.
  #expect(DescriptorLimit.request(soft: 300, hard: 300) == nil)
}

@Test("a window that ends up short of a full window says so")
func aShortLimitIsNotSufficient() {
  // The distinction the log warns on. 300 is more than a launchd default and still less than
  // two daemons of fifteen panes cost.
  let short = DescriptorLimit.Outcome(
    before: 256, after: 300, hard: 300, asked: 300, refusal: nil)
  let ample = DescriptorLimit.Outcome(
    before: 256, after: DescriptorLimit.wanted, hard: unlimited,
    asked: DescriptorLimit.wanted, refusal: nil)

  #expect(!short.sufficient)
  #expect(ample.sufficient)
}
