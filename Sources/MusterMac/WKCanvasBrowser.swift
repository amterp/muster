import AppKit
import CryptoKit
import WebKit

@MainActor
final class WKCanvasBrowser: NSObject, CanvasBrowser, WKNavigationDelegate, WKUIDelegate {
  private final class CanvasWebView: WKWebView {
    // WebKit handles key equivalents before AppKit tries the main menu. Decline them here so
    // the menu gets its shortcuts and any remaining key reaches KeyboardWindow.sendEvent.
    override func performKeyEquivalent(with event: NSEvent) -> Bool { false }
  }

  private enum Failure: Error {
    case invalidScheme
    case rulesUnavailable
    case navigationFailed
  }

  private struct Document {
    let revision: UInt64
    let token = UUID().uuidString
  }

  private struct Load {
    let document: Document
    let navigation: WKNavigation
    let completion: CheckedContinuation<Void, any Error>
  }

  private final class LinkHandler: NSObject, WKScriptMessageHandler {
    weak var browser: WKCanvasBrowser?

    init(browser: WKCanvasBrowser) {
      self.browser = browser
    }

    func userContentController(
      _ controller: WKUserContentController, didReceive message: WKScriptMessage
    ) {
      browser?.receive(message)
    }
  }

  let view = NSView(frame: .zero)
  private let world = WKContentWorld.world(name: "Muster Canvas")
  private var webView: WKWebView?
  private var policy: CanvasPolicy?
  private var prepared: (policy: CanvasPolicy, rules: WKContentRuleList)?
  private var pending: Load?
  private var displayed: Document?
  private var allowsStringLoad = false
  private var zoom: CGFloat = 1

  var focusTarget: NSView? { webView }
  var onLink: ((UInt64, URL) -> Void)?
  var onBlockedNavigation: ((URL?) -> Void)?
  var onContentLost: (() -> Void)?

  private static let linkMessage = "musterCanvasLink"

  func prepare(_ policy: CanvasPolicy) async throws {
    if self.policy == policy || prepared?.policy == policy { return }
    let json = try Self.ruleJSON(allowing: policy.loadSchemes)
    let digest = SHA256.hash(data: Data(json.utf8))
      .map { String(format: "%02x", $0) }.joined()
    guard
      let rules = try await WKContentRuleListStore.default().compileContentRuleList(
        forIdentifier: "muster-canvas-\(digest)", encodedContentRuleList: json)
    else { throw Failure.rulesUnavailable }
    try Task.checkCancellation()
    prepared = (policy, rules)
  }

  private func install(_ policy: CanvasPolicy, rules: WKContentRuleList) {
    let focusWindow: NSWindow? = webView.flatMap { old in
      guard let window = old.window, let responder = window.firstResponder as? NSView,
        responder === old || responder.isDescendant(of: old)
      else { return nil }
      return window
    }
    // A page continues to run under its original rules until its web view is discarded.
    discardWebView()
    let configuration = WKWebViewConfiguration()
    configuration.websiteDataStore = .nonPersistent()
    configuration.defaultWebpagePreferences.allowsContentJavaScript = policy.scriptsEnabled
    configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
    let controller = configuration.userContentController
    controller.add(rules)
    controller.add(LinkHandler(browser: self), contentWorld: world, name: Self.linkMessage)

    let webView = CanvasWebView(frame: view.bounds, configuration: configuration)
    webView.autoresizingMask = [.width, .height]
    webView.allowsLinkPreview = false
    webView.navigationDelegate = self
    webView.uiDelegate = self
    webView.pageZoom = zoom
    self.webView = webView
    self.policy = policy
    view.addSubview(webView)
    focusWindow?.makeFirstResponder(webView)
  }

  static func ruleJSON(allowing schemes: [String]) throws -> String {
    let schemes = Set(schemes.map { $0.lowercased() }).sorted()
    guard
      schemes.allSatisfy({
        $0.range(of: #"\A[a-z][a-z0-9+.-]*\z"#, options: .regularExpression) != nil
      })
    else { throw Failure.invalidScheme }

    let block = ["trigger": ["url-filter": ".*"], "action": ["type": "block"]]
    let exceptions = schemes.map { scheme in
      let escaped = NSRegularExpression.escapedPattern(for: scheme)
      return [
        "trigger": ["url-filter": "^\(escaped):"],
        "action": ["type": "ignore-previous-rules"],
      ]
    }
    let json = try JSONSerialization.data(
      withJSONObject: [block] + exceptions, options: .sortedKeys)
    return String(decoding: json, as: UTF8.self)
  }

  func load(_ page: CanvasPage) async throws {
    guard pending == nil else { throw Failure.navigationFailed }
    try Task.checkCancellation()
    if policy != page.policy {
      guard let prepared, prepared.policy == page.policy else {
        throw Failure.rulesUnavailable
      }
      install(prepared.policy, rules: prepared.rules)
      self.prepared = nil
    }
    guard let webView else { throw Failure.rulesUnavailable }
    let document = Document(revision: page.revision)
    let controller = webView.configuration.userContentController
    controller.removeAllUserScripts()
    controller.addUserScript(
      WKUserScript(
        source: Self.documentScript(token: document.token), injectionTime: .atDocumentStart,
        forMainFrameOnly: false, in: world))
    displayed = nil
    try await withCheckedThrowingContinuation {
      (continuation: CheckedContinuation<Void, any Error>) in
      allowsStringLoad = true
      guard let navigation = webView.loadHTMLString(page.html, baseURL: nil) else {
        allowsStringLoad = false
        continuation.resume(throwing: Failure.navigationFailed)
        return
      }
      pending = Load(document: document, navigation: navigation, completion: continuation)
    }
  }

  private static func documentScript(token: String) -> String {
    """
    globalThis.musterCanvasDocument = "\(token)";

    function interceptLink(event) {
      const anchor = event.composedPath().find(element =>
        element instanceof Element && ["a", "area"].includes(element.localName));
      if (!anchor) return;
      const href = anchor.getAttribute("href") ??
        anchor.getAttributeNS("http://www.w3.org/1999/xlink", "href");
      if (href === null) return;

      event.preventDefault();
      if (!event.isTrusted || event.type !== "click" || event.button !== 0) return;
      try {
        window.webkit.messageHandlers.\(linkMessage).postMessage({
          token: globalThis.musterCanvasDocument,
          url: new URL(href, anchor.baseURI).href
        });
      } catch {}
    }

    window.addEventListener("click", interceptLink, true);
    window.addEventListener("auxclick", interceptLink, true);
    for (const type of ["contextmenu", "beforeinput", "drop", "dragover", "submit"]) {
      window.addEventListener(type, event => event.preventDefault(), true);
    }
    window.addEventListener("DOMContentLoaded", () => {
      for (const region of document.querySelectorAll("[contenteditable]")) {
        if (region.isContentEditable) region.contentEditable = "false";
      }
      for (const control of document.querySelectorAll("input, textarea")) {
        if (control.type === "file") control.disabled = true;
        if (!["checkbox", "radio", "range", "color", "file"].includes(control.type)) {
          control.readOnly = true;
        }
      }
    }, {once: true});
    """
  }

  private func receive(_ message: WKScriptMessage) {
    guard message.name == Self.linkMessage, message.frameInfo.isMainFrame,
      let webView, message.webView === webView, let displayed,
      let body = message.body as? [String: String], body["token"] == displayed.token,
      let raw = body["url"], let url = URL(string: raw)
    else { return }
    onLink?(displayed.revision, url)
  }

  func webView(
    _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
    preferences: WKWebpagePreferences,
    decisionHandler:
      @escaping @MainActor @Sendable (WKNavigationActionPolicy, WKWebpagePreferences)
      -> Void
  ) {
    let url = action.request.url
    if webView === self.webView, allowsStringLoad, action.targetFrame?.isMainFrame == true,
      action.navigationType == .other, url?.absoluteString == "about:blank",
      !action.shouldPerformDownload
    {
      allowsStringLoad = false
      decisionHandler(.allow, preferences)
    } else {
      onBlockedNavigation?(url)
      decisionHandler(.cancel, preferences)
    }
  }

  func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
    guard let load = takeLoad(navigation) else { return }
    displayed = load.document
    load.completion.resume()
  }

  func webView(
    _ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
    withError error: any Error
  ) {
    takeLoad(navigation)?.completion.resume(throwing: Failure.navigationFailed)
  }

  func webView(
    _ webView: WKWebView, didFail navigation: WKNavigation!, withError error: any Error
  ) {
    takeLoad(navigation)?.completion.resume(throwing: Failure.navigationFailed)
  }

  private func takeLoad(_ navigation: WKNavigation?) -> Load? {
    guard let pending, pending.navigation === navigation else { return nil }
    self.pending = nil
    allowsStringLoad = false
    return pending
  }

  func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
    guard webView === self.webView else { return }
    let hadPendingLoad = pending != nil
    if let pending {
      takeLoad(pending.navigation)?.completion.resume(throwing: Failure.navigationFailed)
    }
    displayed = nil
    if !hadPendingLoad { onContentLost?() }
  }

  func webView(
    _ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
    for action: WKNavigationAction, windowFeatures: WKWindowFeatures
  ) -> WKWebView? {
    onBlockedNavigation?(action.request.url)
    return nil
  }

  func webView(
    _ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
    initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping @MainActor @Sendable () -> Void
  ) {
    completionHandler()
  }

  func webView(
    _ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
    initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping @MainActor @Sendable (Bool) -> Void
  ) {
    completionHandler(false)
  }

  func webView(
    _ webView: WKWebView, runJavaScriptTextInputPanelWithPrompt prompt: String,
    defaultText: String?, initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping @MainActor @Sendable (String?) -> Void
  ) {
    completionHandler(nil)
  }

  func webView(
    _ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters,
    initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping @MainActor @Sendable ([URL]?) -> Void
  ) {
    completionHandler(nil)
  }

  func scrollPosition() async -> NSPoint? {
    guard let webView, let displayed else { return nil }
    let result = try? await webView.callAsyncJavaScript(
      """
      if (globalThis.musterCanvasDocument === token) return [window.scrollX, window.scrollY];
      """,
      arguments: ["token": displayed.token], in: nil, contentWorld: world)
    guard let point = result as? [Double], point.count == 2 else { return nil }
    return NSPoint(x: point[0], y: point[1])
  }

  func update(appearance: CanvasAppearance, scrollTo: NSPoint?) {
    guard let webView, let displayed else { return }
    webView.underPageBackgroundColor = appearance.background.color
    var arguments: [String: Any] = ["token": displayed.token, "css": appearance.css]
    if let scrollTo {
      arguments["scroll"] = [scrollTo.x, scrollTo.y]
    } else {
      arguments["scroll"] = NSNull()
    }
    webView.callAsyncJavaScript(
      """
      if (globalThis.musterCanvasDocument !== token) return;
      for (const [name, value] of Object.entries(css)) {
        document.documentElement.style.setProperty(name, value);
      }
      if (scroll !== null) window.scrollTo({left: scroll[0], top: scroll[1], behavior: "instant"});
      """,
      arguments: arguments, in: nil, in: world)
  }

  func setZoom(_ zoom: CGFloat) {
    self.zoom = zoom
    if let webView, webView.pageZoom != zoom { webView.pageZoom = zoom }
  }

  func scroll(_ movement: CanvasScroll) {
    guard let webView, let displayed else { return }
    webView.callAsyncJavaScript(
      """
      if (globalThis.musterCanvasDocument !== token) return;
      switch (movement) {
        case "top": window.scrollTo(0, 0); break;
        case "bottom": window.scrollTo(0, document.scrollingElement.scrollHeight); break;
        case "pageUp": window.scrollBy(0, -window.innerHeight); break;
        case "pageDown": window.scrollBy(0, window.innerHeight); break;
      }
      """,
      arguments: ["token": displayed.token, "movement": movement.rawValue],
      in: nil, in: world)
  }

  func selectAll() {
    guard let webView, let responder = webView.window?.firstResponder as? NSView,
      responder === webView || responder.isDescendant(of: webView)
    else { return }
    responder.tryToPerform(#selector(NSText.selectAll(_:)), with: nil)
  }

  func tearDown() {
    discardWebView()
    prepared = nil
    onLink = nil
    onBlockedNavigation = nil
    onContentLost = nil
  }

  private func discardWebView() {
    if let pending {
      takeLoad(pending.navigation)?.completion.resume(throwing: CancellationError())
    }
    displayed = nil
    policy = nil
    guard let webView else { return }
    self.webView = nil
    webView.configuration.userContentController.removeScriptMessageHandler(
      forName: Self.linkMessage, contentWorld: world)
    webView.navigationDelegate = nil
    webView.uiDelegate = nil
    webView.stopLoading()
    webView.removeFromSuperview()
  }
}
