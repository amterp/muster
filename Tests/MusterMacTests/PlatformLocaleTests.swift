import Foundation
import Testing

@testable import MusterMac

// Naming a POSIX locale from what macOS says the user picked. The half the shell owns, and all
// of it: whether a daemon gets the answer is the core's, and pinned by the conformance corpus.

@Suite("the platform's locale, as a POSIX name")
struct PlatformLocaleTests {
  @Test("a language and a region become the name a pane's shell understands")
  func bothHalvesBecomeALocale() {
    #expect(posixLocale(language: "en", region: "AU") == "en_AU.UTF-8")
    #expect(posixLocale(language: "ja", region: "JP") == "ja_JP.UTF-8")
  }

  @Test("a half-answer is no answer")
  func aMissingHalfNamesNothing() {
    // Nothing plausible is invented from one half. A locale Muster made up goes into a daemon
    // that outlives the app and reaches every pane it ever spawns, so being wrong here is
    // worse than the C locale, which is at least honest about being a default.
    #expect(posixLocale(language: "en", region: nil) == nil)
    #expect(posixLocale(language: nil, region: "AU") == nil)
    #expect(posixLocale(language: "", region: "AU") == nil)
    #expect(posixLocale(language: "en", region: "") == nil)
  }

  @Test("a real locale is read off the platform rather than guessed")
  func theMachineAnswers() {
    // Whatever this Mac is set to - the assertion is the shape, because the value belongs to
    // whoever is running the suite.
    let named = platformLocale(Locale(identifier: "pt_BR"))
    #expect(named == "pt_BR.UTF-8")
  }
}
