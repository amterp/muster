import Foundation
import GhosttyKit
import Testing

// What libghostty makes of a config file Muster wrote, asserted against the real library rather
// than believed. This is the mechanism the whole appearance vocabulary rests on: there is no
// setter in the C API, so handing over a file is how Muster says what a pane should look like
// (docs/observations/libghostty-9f9b8d1d.md section 9). A pin bump that changes any of this
// should fail here rather than in a window.
//
// Serialized, and one ghostty_init for the whole suite: ghostty_init assigns process-global state
// (src/global.zig, `var state: ?GlobalState`), so two of them racing is two tests fighting over
// one global.

@Suite(.serialized) struct GhosttyConfigTests {
  static let ready: Bool = {
    var argv: [UnsafeMutablePointer<CChar>?] = [strdup("muster")]
    return argv.withUnsafeMutableBufferPointer { buffer in
      ghostty_init(UInt(buffer.count), buffer.baseAddress!) == GHOSTTY_SUCCESS
    }
  }()

  /// A config built from lines Muster wrote, the way the shell would build one.
  func loaded(_ lines: [String]) throws -> ghostty_config_t {
    #expect(Self.ready)
    let path = FileManager.default.temporaryDirectory
      .appendingPathComponent("muster-probe-\(UUID().uuidString).conf")
    try lines.joined(separator: "\n").write(to: path, atomically: true, encoding: .utf8)
    defer { try? FileManager.default.removeItem(at: path) }

    let config = ghostty_config_new()!
    path.path.withCString { ghostty_config_load_file(config, $0) }
    ghostty_config_finalize(config)
    return config
  }

  @Test func aFileMusterWroteDecidesWhatASurfaceLooksLike() throws {
    let config = try loaded([
      "background = #123456",
      "foreground = #abcdef",
      "cursor-style = bar",
      "cursor-style-blink = false",
      "font-size = 17",
      "font-family = Menlo",
      "selection-background = #414868",
      "window-padding-x = 4",
      "palette = 1=#ff0000",
      "palette = 9=#00ff00",
    ])
    defer { ghostty_config_free(config) }

    #expect(color(config, "background") == "#123456")
    #expect(color(config, "foreground") == "#abcdef")
    #expect(name(config, "cursor-style") == "bar")
    #expect(flag(config, "cursor-style-blink") == false)
    #expect(float(config, "font-size") == 17)
    #expect(palette(config, 1) == "#ff0000")
    #expect(palette(config, 9) == "#00ff00")
    #expect(ghostty_config_diagnostics_count(config) == 0)

    // Not every key can be read back: `selection-background` is a union and `window-padding-x`
    // a plain struct, and ghostty_config_get answers only for types with a C representation
    // (src/config/c_get.zig). They applied - the diagnostics count above is what says so.
    #expect(color(config, "selection-background") == nil)
    #expect(count(config, "window-padding-x") == nil)
  }

  @Test func aValueLibghosttyCannotParseIsADiagnosticToo() throws {
    // This is what makes a zero diagnostics count an oracle for the keys that cannot be read
    // back. If only unknown *keys* were reported, `cursor-style = wobble` would pass silently
    // and the count would prove nothing about the values.
    let config = try loaded(["cursor-style = wobble"])
    defer { ghostty_config_free(config) }

    #expect(ghostty_config_diagnostics_count(config) >= 1)
    if ghostty_config_diagnostics_count(config) >= 1 {
      let first = ghostty_config_get_diagnostic(config, 0)
      print("BAD VALUE: \(first.message.map { String(cString: $0) } ?? "(none)")")
    }
  }

  @Test func nothingOnDiskIsNotAFailure() throws {
    // The case where a person named no appearance at all, which has to leave libghostty's own
    // defaults in place rather than producing an unusable config.
    let config = try loaded([])
    defer { ghostty_config_free(config) }
    #expect(color(config, "background") != nil)
    #expect(ghostty_config_diagnostics_count(config) == 0)
  }

  @Test func aKeyLibghosttyDoesNotKnowIsADiagnosticRatherThanAFailure() throws {
    let config = try loaded(["background = #123456", "not-a-ghostty-key = 1"])
    defer { ghostty_config_free(config) }

    #expect(ghostty_config_diagnostics_count(config) >= 1)
    if ghostty_config_diagnostics_count(config) >= 1 {
      let first = ghostty_config_get_diagnostic(config, 0)
      print("DIAGNOSTIC: \(first.message.map { String(cString: $0) } ?? "(none)")")
    }
    // Everything else still applied, which is what makes a diagnostic worth logging rather than
    // treating as fatal.
    #expect(color(config, "background") == "#123456")
  }

  @Test func aSecondFileReplacesTheFirst() throws {
    // What a reload is: a fresh handle read from a fresh file, handed to ghostty_app_update_config.
    let first = try loaded(["background = #111111"])
    defer { ghostty_config_free(first) }
    let second = try loaded(["background = #222222"])
    defer { ghostty_config_free(second) }

    #expect(color(first, "background") == "#111111")
    #expect(color(second, "background") == "#222222")
  }

}

// Readers for the C API's several shapes. `ghostty_config_get` writes through a void pointer whose
// type is decided by the key, so a mismatch reads garbage rather than failing (an f32 read as an
// f64 comes back as -1.0000002441229299, which is how this list got written).

private func float(_ config: ghostty_config_t, _ key: String) -> Float? {
  var out: Float = -1
  let got = key.withCString { ghostty_config_get(config, &out, $0, UInt(strlen($0))) }
  return got ? out : nil
}

private func count(_ config: ghostty_config_t, _ key: String) -> UInt32? {
  var out: UInt32 = 0
  let got = key.withCString { ghostty_config_get(config, &out, $0, UInt(strlen($0))) }
  return got ? out : nil
}

private func flag(_ config: ghostty_config_t, _ key: String) -> Bool? {
  var out = false
  let got = key.withCString { ghostty_config_get(config, &out, $0, UInt(strlen($0))) }
  return got ? out : nil
}

private func name(_ config: ghostty_config_t, _ key: String) -> String? {
  var out: UnsafePointer<CChar>?
  let got = key.withCString { ghostty_config_get(config, &out, $0, UInt(strlen($0))) }
  guard got, let out else { return nil }
  return String(cString: out)
}

private func color(_ config: ghostty_config_t, _ key: String) -> String? {
  var out = ghostty_config_color_s()
  let got = key.withCString { ghostty_config_get(config, &out, $0, UInt(strlen($0))) }
  return got ? String(format: "#%02x%02x%02x", out.r, out.g, out.b) : nil
}

private func palette(_ config: ghostty_config_t, _ index: Int) -> String? {
  var out = ghostty_config_palette_s()
  let got = "palette".withCString { ghostty_config_get(config, &out, $0, UInt(strlen($0))) }
  guard got else { return nil }
  let entry = withUnsafeBytes(of: out.colors) { raw in
    raw.bindMemory(to: ghostty_config_color_s.self)[index]
  }
  return String(format: "#%02x%02x%02x", entry.r, entry.g, entry.b)
}
