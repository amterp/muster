import AppKit
import ImageIO

enum ClipboardImage: Sendable {
  case png(Data)
  case tiff(Data)

  @MainActor static func read(from pasteboard: NSPasteboard) -> ClipboardImage? {
    if let data = pasteboard.data(forType: .png) { return .png(data) }
    if let data = pasteboard.data(forType: .tiff) { return .tiff(data) }
    return nil
  }

  @MainActor static func containsImage(_ pasteboard: NSPasteboard) -> Bool {
    pasteboard.availableType(from: [.png, .tiff]) != nil
  }

  func encoded() throws -> Data {
    switch self {
    case .png(let data):
      return data
    case .tiff(let data):
      guard let source = CGImageSourceCreateWithData(data as CFData, nil) else {
        throw ImagePasteFailure("the clipboard image could not be read")
      }
      let output = NSMutableData()
      guard
        let destination = CGImageDestinationCreateWithData(output, "public.png" as CFString, 1, nil)
      else { throw ImagePasteFailure("the clipboard image could not be converted to PNG") }
      CGImageDestinationAddImageFromSource(destination, source, 0, nil)
      guard CGImageDestinationFinalize(destination) else {
        throw ImagePasteFailure("the clipboard image could not be converted to PNG")
      }
      return output as Data
    }
  }
}

struct ImagePasteFailure: Error, Sendable {
  let reason: String
  init(_ reason: String) { self.reason = reason }

  @MainActor static func show(_ reason: String, in window: NSWindow?) {
    guard let window else {
      Core.warn("input.image.refused", ["reason": reason])
      return
    }
    let alert = NSAlert()
    alert.messageText = "Image paste failed"
    alert.informativeText = reason
    alert.addButton(withTitle: "OK")
    alert.beginSheetModal(for: window)
  }
}
