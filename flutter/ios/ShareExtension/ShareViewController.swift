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
    static let plainText = "public.plain-text"
    static let url = "public.url"
    static let fileURL = "public.file-url"
    static let data = "public.data"
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
    guard !attachments.isEmpty else {
      finish()
      return
    }
    // No App Group means there is nowhere to put the share. That happens on
    // installs where the entitlement wasn't granted (an unsigned sideload
    // without the group injected). Report it instead of completing normally,
    // so the user sees a failure rather than a share that silently vanishes.
    guard let batchDir = makeBatchDirectory() else {
      extensionContext?.cancelRequest(
        withError: NSError(
          domain: "dev.vigov5.wisp.ShareExtension",
          code: 1,
          userInfo: [
            NSLocalizedDescriptionKey:
              "Wisp can't access its shared storage on this install."
          ]
        )
      )
      return
    }

    // Each attachment resolves asynchronously; the group is what lets us wait
    // for all of them before telling the host we're done. Completing early
    // would tear the extension down mid-copy and lose the share.
    let group = DispatchGroup()
    let lock = NSLock()
    var collectedText: String?

    func recordText(_ text: String) {
      lock.lock()
      defer { lock.unlock() }
      collectedText = text
    }

    /// Copies a provider-supplied file into the batch.
    ///
    /// `preferredName` is used when the source name is unhelpful:
    /// `loadFileRepresentation` sometimes hands back a generic temp file (a
    /// shared link arrives as a file literally called "URL"), while the item's
    /// own URL carries the real name.
    ///
    /// A URL that came straight from the sending app points into *its*
    /// container, so it needs security-scoped access before we can read it —
    /// without this the copy fails silently and the share is dropped.
    func copyFile(at url: URL, preferredName: String? = nil) {
      let scoped = url.startAccessingSecurityScopedResource()
      defer {
        if scoped { url.stopAccessingSecurityScopedResource() }
      }
      var name = preferredName ?? url.lastPathComponent
      if name.isEmpty { name = url.lastPathComponent }
      let destination = batchDir.appendingPathComponent(name)
      lock.lock()
      defer { lock.unlock() }
      do {
        try FileManager.default.copyItem(at: url, to: destination)
      } catch {
        // Some providers hand over a URL that can't be copied but can be read.
        if let data = try? Data(contentsOf: url) {
          try? data.write(to: destination)
        } else {
          NSLog("[wisp] could not take shared file \(name): \(error)")
        }
      }
    }

    for attachment in attachments {
      // Order matters, and not in the obvious direction: public.url conforms to
      // public.item, so testing the file case first swallows a Safari link into
      // a file load that yields nothing. Resolve URLs first and decide from the
      // loaded value — a file URL still lands in the file branch below.
      if attachment.hasItemConformingToTypeIdentifier(UTI.url) {
        group.enter()
        attachment.loadItem(forTypeIdentifier: UTI.url, options: nil) { value, _ in
          guard let url = value as? URL else {
            group.leave()
            return
          }
          guard url.isFileURL else {
            recordText(url.absoluteString)
            group.leave()
            return
          }
          // A file arriving as public.file-url (this is what Filza's "Open in"
          // sends) gives us a path inside the *sending* app's container. Ask for
          // a file representation instead, which the system hands over somewhere
          // we're actually allowed to read; keep the original name, since the
          // representation's temp file is often named generically.
          let fileType =
            attachment.registeredTypeIdentifiers.first { $0 != UTI.url && $0 != UTI.fileURL }
            ?? UTI.data
          attachment.loadFileRepresentation(forTypeIdentifier: fileType) { fileURL, error in
            defer { group.leave() }
            if let fileURL = fileURL {
              copyFile(at: fileURL, preferredName: url.lastPathComponent)
            } else {
              NSLog("[wisp] no representation for \(fileType): \(String(describing: error))")
              // Last resort: the raw URL, which works when the sender granted
              // us a security scope we can open.
              copyFile(at: url)
            }
          }
        }
      } else if attachment.hasItemConformingToTypeIdentifier(UTI.plainText) {
        group.enter()
        attachment.loadItem(forTypeIdentifier: UTI.plainText, options: nil) { value, _ in
          defer { group.leave() }
          if let text = value as? String { recordText(text) }
        }
      } else if let type = attachment.registeredTypeIdentifiers.first {
        // Ask for the provider's own concrete type (public.jpeg, com.adobe.pdf,
        // …). Requesting the abstract public.item instead leaves photo shares
        // with no matching representation, and the completion never fires — the
        // extension then hangs and never returns to the app at all.
        group.enter()
        attachment.loadFileRepresentation(forTypeIdentifier: type) { url, error in
          defer { group.leave() }
          guard let url = url else {
            NSLog("[wisp] no file representation for \(type): \(String(describing: error))")
            return
          }
          copyFile(at: url)
        }
      }
    }

    // Watchdog: a provider that never calls back would otherwise leave the
    // share sheet spinning forever with no way out. Whatever arrived by then
    // still gets handed over.
    var finished = false
    let complete: () -> Void = { [weak self] in
      guard let self = self, !finished else { return }
      finished = true
      lock.lock()
      let text = collectedText
      lock.unlock()
      if let text = text, !text.isEmpty {
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

    group.notify(queue: .main, execute: complete)
    DispatchQueue.main.asyncAfter(deadline: .now() + 20, execute: complete)
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
