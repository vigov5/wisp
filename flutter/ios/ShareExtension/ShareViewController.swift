import UIKit

/// The iOS Share Extension: takes whatever the share sheet hands over, drops it
/// into the App Group container, and asks Wisp to come forward.
///
/// A share extension runs in its own process and cannot reach the app's Flutter
/// engine, so the App Group container is the hand-off point. `ShareIntentHandler`
/// on the app side drains it on every activation.
///
/// UTIs are spelled out as raw strings rather than using `UTType`, which is
/// iOS 14+; the deployment target here is 13.0.
class ShareViewController: UIViewController {
  private enum UTI {
    static let item = "public.item"
    static let plainText = "public.plain-text"
    static let url = "public.url"
    static let fileURL = "public.file-url"
  }

  /// Must match `ShareIntentHandler.appGroupIdentifier`.
  private let appGroupIdentifier = "group.dev.vigov5.wisp"
  private let dropBoxName = "SharedInbox"
  private let textMarkerName = "_wisp_shared_text.txt"

  override func viewDidLoad() {
    super.viewDidLoad()
    processShare()
  }

  private func processShare() {
    let attachments = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
      .flatMap { $0.attachments ?? [] }
    guard !attachments.isEmpty, let batchDir = makeBatchDirectory() else {
      finish()
      return
    }

    // Each attachment resolves asynchronously; the group is what lets us wait
    // for all of them before telling the host we're done. Completing early
    // would tear the extension down mid-copy and lose the share.
    let group = DispatchGroup()
    var collectedText: String?

    for attachment in attachments {
      // File-backed items first: a file URL also conforms to public.url, so
      // checking the URL case first would misread a shared file as a link.
      if attachment.hasItemConformingToTypeIdentifier(UTI.fileURL)
        || attachment.hasItemConformingToTypeIdentifier(UTI.item)
      {
        group.enter()
        attachment.loadFileRepresentation(forTypeIdentifier: UTI.item) { url, _ in
          defer { group.leave() }
          guard let url = url else { return }
          // The callback's URL is only valid until it returns, so copy now.
          let destination = batchDir.appendingPathComponent(url.lastPathComponent)
          try? FileManager.default.copyItem(at: url, to: destination)
        }
      } else if attachment.hasItemConformingToTypeIdentifier(UTI.plainText) {
        group.enter()
        attachment.loadItem(forTypeIdentifier: UTI.plainText, options: nil) { value, _ in
          defer { group.leave() }
          if let text = value as? String { collectedText = text }
        }
      } else if attachment.hasItemConformingToTypeIdentifier(UTI.url) {
        group.enter()
        attachment.loadItem(forTypeIdentifier: UTI.url, options: nil) { value, _ in
          defer { group.leave() }
          if let url = value as? URL { collectedText = url.absoluteString }
        }
      }
    }

    group.notify(queue: .main) { [weak self] in
      guard let self = self else { return }
      if let text = collectedText, !text.isEmpty {
        try? text.write(
          to: batchDir.appendingPathComponent(self.textMarkerName),
          atomically: true,
          encoding: .utf8
        )
      }
      // Nothing landed — remove the empty batch so the app doesn't wake for it.
      if (try? FileManager.default.contentsOfDirectory(atPath: batchDir.path))?.isEmpty ?? true {
        try? FileManager.default.removeItem(at: batchDir)
      }
      self.openHostApp()
      self.finish()
    }
  }

  /// Creates this share's batch directory in the App Group container.
  ///
  /// Named by timestamp so the app can drain multiple queued shares in the
  /// order they were made, with a UUID suffix to keep two shares in the same
  /// millisecond apart.
  private func makeBatchDirectory() -> URL? {
    let fileManager = FileManager.default
    guard
      let container = fileManager.containerURL(
        forSecurityApplicationGroupIdentifier: appGroupIdentifier)
    else {
      NSLog("[wisp] share extension has no access to App Group \(appGroupIdentifier)")
      return nil
    }
    let name = String(format: "%013.0f-%@", Date().timeIntervalSince1970 * 1000,
                      UUID().uuidString)
    let batchDir = container
      .appendingPathComponent(dropBoxName, isDirectory: true)
      .appendingPathComponent(name, isDirectory: true)
    do {
      try fileManager.createDirectory(at: batchDir, withIntermediateDirectories: true)
    } catch {
      NSLog("[wisp] could not create share batch directory: \(error)")
      return nil
    }
    return batchDir
  }

  /// Best-effort nudge to bring Wisp forward.
  ///
  /// iOS gives share extensions no supported way to launch their host app, so
  /// this may simply be refused — that's fine. The app drains the App Group on
  /// every activation, so the share still arrives the next time Wisp opens.
  private func openHostApp() {
    guard let url = URL(string: "wisp://share") else { return }
    // `UIApplication.shared` is unavailable in an extension, so reach it by
    // walking the responder chain. The single-argument `openURL:` is used
    // deliberately: `perform(_:with:)` passes exactly one argument, and
    // selecting the 3-argument `open(_:options:completionHandler:)` here would
    // leave the remaining parameters as garbage.
    let openURL = NSSelectorFromString("openURL:")
    var responder: UIResponder? = self
    while let current = responder {
      if current.responds(to: openURL) {
        current.perform(openURL, with: url)
        return
      }
      responder = current.next
    }
  }

  private func finish() {
    extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
  }
}
