import Foundation

@MainActor
final class CoreCanvasPageSource: CanvasPageSource {
  private struct Observer {
    let id: UUID
    let receive: (UInt64, Bool) -> Void
  }

  private let window: String
  private let dispatcher: Dispatcher
  private var observers: [PaneKey: Observer] = [:]

  init(window: String, dispatcher: Dispatcher = Core.dispatcher) {
    self.window = window
    self.dispatcher = dispatcher
  }

  func observeRevisions(
    for pane: PaneKey, receive: @escaping (UInt64, Bool) -> Void
  ) -> @MainActor () -> Void {
    let id = UUID()
    observers[pane] = Observer(id: id, receive: receive)
    return { [weak self] in
      guard let self, observers[pane]?.id == id else { return }
      observers.removeValue(forKey: pane)
    }
  }

  func publish(pane: PaneKey, revision: UInt64, reset: Bool) {
    observers[pane]?.receive(revision, reset)
  }

  nonisolated func page(for pane: PaneKey, revision: UInt64) async throws -> CanvasPage {
    try Task.checkCancellation()
    return try Core.readCanvasPage(pane, window: window, through: dispatcher)
  }
}

extension CanvasInfo.Kind {
  init?(_ kind: Muster_CanvasKind) {
    switch kind {
    case .markdown: self = .markdown
    case .html: self = .html
    case .image: self = .image
    default: return nil
    }
  }
}

extension CanvasInfo {
  init(_ info: Muster_CanvasInfo) {
    self.init(revision: info.revision, kind: Kind(info.kind), title: info.title)
  }
}
