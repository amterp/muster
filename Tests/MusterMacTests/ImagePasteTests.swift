import AppKit
import ImageIO
import Testing

@testable import MusterMac

private let imagePNG: Data = {
  let provider = CGDataProvider(data: Data([255, 64, 32, 255]) as CFData)!
  let image = CGImage(
    width: 1, height: 1, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: 4,
    space: CGColorSpaceCreateDeviceRGB(),
    bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
    provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
  let output = NSMutableData()
  let destination = CGImageDestinationCreateWithData(output, "public.png" as CFString, 1, nil)!
  CGImageDestinationAddImage(destination, image, nil)
  precondition(CGImageDestinationFinalize(destination))
  return output as Data
}()

private func imageRecorder(eligible: Bool = true) -> RecordingDispatcher {
  RecordingDispatcher { request in
    switch request.payload {
    case .readImagePasteTarget:
      var target = Muster_ImagePasteTarget()
      if eligible {
        target.daemonID = "remote"
        target.paneID = "p1"
      }
      return .imagePasteTarget(target)
    case .prepareImagePaste:
      var prepared = Muster_ImagePastePrepared()
      prepared.uploadID = 7
      return .imagePastePrepared(prepared)
    default: return nil
    }
  }
}

@MainActor private func imageView(_ recorder: RecordingDispatcher) -> SurfaceView {
  seam(recorder)
  let view = SurfaceView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
  view.attach(typeable: true)
  view.pasteboard = NSPasteboard(name: NSPasteboard.Name("image-paste-\(UUID().uuidString)"))
  view.pasteboard.setData(imagePNG, forType: .png)
  return view
}

private func imageKey(_ type: NSEvent.EventType = .keyDown, repeated: Bool = false) -> NSEvent {
  NSEvent.keyEvent(
    with: type, location: .zero, modifierFlags: .control, timestamp: 0, windowNumber: 0,
    context: nil, characters: "v", charactersIgnoringModifiers: "v", isARepeat: repeated,
    keyCode: 0x09)!
}

@Test(.ownsTheSeam) @MainActor func commandPastePrefersTextThenAnUnchangedPNG() {
  let recorder = imageRecorder()
  let view = imageView(recorder)
  #expect(view.canPaste)
  view.pasteboard.setString("caption", forType: .string)
  view.paste(nil)
  #expect(recorder.requests.contains { $0.paste.text == "caption" })
  #expect(
    !recorder.requests.contains { if case .prepareImagePaste = $0.payload { true } else { false } })

  view.pasteboard.clearContents()
  view.pasteboard.setData(imagePNG, forType: .png)
  view.paste(nil)
  let completed = recorder.requests.compactMap { request -> Muster_PasteImage? in
    if case .pasteImage(let image) = request.payload { return image }
    return nil
  }
  #expect(completed.count == 1)
  #expect(completed.first?.png == imagePNG)
  #expect(completed.first?.uploadID == 7)
}

@Test(.ownsTheSeam) @MainActor func controlPasteCapturesThePaneAndSwallowsRepeatAndRelease() {
  let recorder = imageRecorder()
  let view = imageView(recorder)
  view.pasteboard.setString("caption", forType: .string)
  view.keyDown(with: imageKey())
  view.keyDown(with: imageKey(repeated: true))
  // A focus change can deliver the release to a different surface.
  SurfaceView(frame: .zero).keyUp(with: imageKey(.keyUp))

  let prepared = recorder.requests.compactMap { request -> Muster_PrepareImagePaste? in
    if case .prepareImagePaste(let target) = request.payload { return target }
    return nil
  }
  #expect(prepared.count == 1)
  #expect(prepared.first?.daemonID == "remote")
  #expect(prepared.first?.paneID == "p1")
  #expect(!recorder.requests.contains { if case .keyDown = $0.payload { true } else { false } })
  #expect(!recorder.requests.contains { if case .keyUp = $0.payload { true } else { false } })
}

@Test(.ownsTheSeam) @MainActor func controlPastePassesThroughWhenTheCoreDeclines() {
  let recorder = imageRecorder(eligible: false)
  let view = imageView(recorder)
  view.keyDown(with: imageKey())
  view.keyUp(with: imageKey(.keyUp))
  #expect(recorder.requests.contains { if case .keyDown = $0.payload { true } else { false } })
  #expect(recorder.requests.contains { if case .keyUp = $0.payload { true } else { false } })
  #expect(
    !recorder.requests.contains { if case .prepareImagePaste = $0.payload { true } else { false } })
}

@Test(.ownsTheSeam) @MainActor func aFreshPressDoesNotInheritAnImageKeysMissingRelease() {
  let first = imageRecorder()
  imageView(first).keyDown(with: imageKey())
  let next = imageRecorder(eligible: false)
  let view = imageView(next)
  view.keyDown(with: imageKey())
  view.keyUp(with: imageKey(.keyUp))
  #expect(next.requests.contains { if case .keyDown = $0.payload { true } else { false } })
  #expect(next.requests.contains { if case .keyUp = $0.payload { true } else { false } })
}

@Test func aTIFFClipboardImageBecomesAPNGWithTheSameDimensions() throws {
  let source = try #require(CGImageSourceCreateWithData(imagePNG as CFData, nil))
  let tiff = NSMutableData()
  let destination = try #require(
    CGImageDestinationCreateWithData(tiff, "public.tiff" as CFString, 1, nil))
  CGImageDestinationAddImageFromSource(destination, source, 0, nil)
  #expect(CGImageDestinationFinalize(destination))
  let encoded = try ClipboardImage.tiff(tiff as Data).encoded()
  let png = try #require(CGImageSourceCreateWithData(encoded as CFData, nil))
  #expect(CGImageSourceGetType(png) as String? == "public.png")
  let image = try #require(CGImageSourceCreateImageAtIndex(png, 0, nil))
  #expect(image.width == 1 && image.height == 1)
}
