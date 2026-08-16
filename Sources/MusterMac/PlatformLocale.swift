import Foundation

// What locale this machine is set to, which is a question only the platform can answer.
//
// A window launched the way Muster is meant to be launched - Dock, Finder, Spotlight - gets no
// LANG and no LC_* at all, because launchd hands a GUI process almost nothing. Building the
// renderer currently leaves one in the process anyway, which is why nothing looks broken and
// why this is worth having: `daemon::supplied` in the core has the measurement and the
// argument.
//
// Reported here and decided in the core, the same split the log file and the herdr binary
// already draw: naming a POSIX locale from the user's macOS settings is an OS question, and
// whether a daemon with none of its own gets one is Muster's.

/// This machine's locale as a POSIX name, or nil if the platform will not name one.
///
/// Takes the two halves rather than reading them, so the answer is assertable without
/// depending on what the developer running the suite has their Mac set to - the same shape as
/// `herdrPath`.
///
/// `.UTF-8` unconditionally, which is Ghostty's answer too (`src/os/locale.zig`,
/// `setLangFromCocoa`). macOS has no other text encoding worth naming here, and a locale
/// without one is a locale a pane treats as ASCII.
public func posixLocale(language: String?, region: String?) -> String? {
  guard let language, let region, !language.isEmpty, !region.isEmpty else { return nil }
  return "\(language)_\(region).UTF-8"
}

/// The locale macOS says this user picked.
public func platformLocale(_ locale: Locale = .current) -> String? {
  posixLocale(
    language: locale.language.languageCode?.identifier, region: locale.region?.identifier)
}
