import Flutter
import UIKit

/// Bridges iOS shares onto the same `dev.vigov5.wisp/share_intent` channel
/// Android's ACTION_SEND uses, so the Dart side (`platform/share_intent.dart`)
/// needs no per-platform branch.
///
/// Two sources feed it:
///   - Files opened straight into the app ("Open in Wisp", `CFBundleDocumentTypes`),
///     arriving as scene URL contexts.
///   - The Share Extension, which cannot talk to us directly and instead drops
///     items into the shared App Group container for us to drain.
///
/// Delivery keys off whether Dart has taken its cold-start batch yet rather
/// than off launch timing: anything arriving before that is stashed and handed
/// over by `getInitialSharedFiles`, anything after is pushed live. Guessing
/// from "is this a cold start" would race the Dart handler, which isn't
/// installed until the first frame.
enum ShareIntentHandler {
  private static let channelName = "dev.vigov5.wisp/share_intent"

  /// Must match the App Group on both the app and the extension.
  static let appGroupIdentifier = "group.dev.vigov5.wisp"

  /// Directory inside the App Group container the extension drops shares into.
  static let dropBoxName = "SharedInbox"

  /// Reserved filename the extension uses for a plain-text/URL share.
  static let textMarkerName = "_wisp_shared_text.txt"

  /// Cache subdirectory holding our copies of shared items.
  private static let cacheDirName = "wisp_shared"

  private static var channel: FlutterMethodChannel?
  private static var pendingFiles: [String] = []
  private static var pendingText: String?

  /// Set once Dart has drained the cold-start batch; from then on new shares
  /// are pushed over the channel instead of stashed.
  private static var initialDrained = false

  static func register(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: channelName, binaryMessenger: messenger)
    self.channel = channel
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "getInitialSharedFiles":
        initialDrained = true
        let files = pendingFiles
        pendingFiles = []
        result(files)
      case "getInitialSharedText":
        initialDrained = true
        let text = pendingText
        pendingText = nil
        result(text)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  /// Handles URLs delivered by the scene, cold or warm.
  ///
  /// A `wisp://` URL is the Share Extension nudging us awake rather than a
  /// document — it carries no payload, the items are waiting in the App Group
  /// drop box.
  static func handle(urls: [URL]) {
    var paths: [String] = []
    var wakeFromExtension = false
    for url in urls {
      if url.isFileURL {
        if let path = copyIntoCache(url) { paths.append(path) }
      } else if url.scheme == "wisp" {
        wakeFromExtension = true
      }
    }
    deliver(files: paths, text: nil)
    if wakeFromExtension { drainAppGroupDropBox() }
  }

  /// Moves anything the Share Extension left in the App Group container into
  /// our own cache and delivers it.
  ///
  /// Called on every scene activation, not just on the `wisp://` wake: the
  /// extension's attempt to foreground us is best-effort, so a share may well
  /// be sitting here from a session where the user never got bounced over.
  static func drainAppGroupDropBox() {
    let fileManager = FileManager.default
    guard
      let container = fileManager.containerURL(
        forSecurityApplicationGroupIdentifier: appGroupIdentifier)
    else { return }

    let dropBox = container.appendingPathComponent(dropBoxName, isDirectory: true)
    guard
      let batches = try? fileManager.contentsOfDirectory(
        at: dropBox, includingPropertiesForKeys: nil)
    else { return }

    var paths: [String] = []
    var text: String?
    // Oldest first, so multiple queued shares keep their order.
    for batch in batches.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
      guard
        let items = try? fileManager.contentsOfDirectory(
          at: batch, includingPropertiesForKeys: nil)
      else { continue }
      for item in items {
        if item.lastPathComponent == textMarkerName {
          text = (try? String(contentsOf: item, encoding: .utf8)) ?? text
        } else if let path = copyIntoCache(item) {
          paths.append(path)
        }
      }
      // Drained: drop the batch so it isn't delivered twice on the next
      // activation, and so the shared container doesn't grow without bound.
      try? fileManager.removeItem(at: batch)
    }

    deliver(files: paths, text: text)
  }

  private static func deliver(files: [String], text: String?) {
    if !files.isEmpty {
      if initialDrained, let channel = channel {
        channel.invokeMethod("onSharedFiles", arguments: files)
      } else {
        pendingFiles.append(contentsOf: files)
      }
    }
    guard let text = text, !text.isEmpty else { return }
    if initialDrained, let channel = channel {
      channel.invokeMethod("onSharedText", arguments: text)
    } else {
      pendingText = text
    }
  }

  /// Copies an incoming item into an app-owned cache directory and returns the
  /// copy's path.
  ///
  /// The original may live outside our sandbox (an in-place open, which needs
  /// security-scoped access) or in `Documents/Inbox` (a copy iOS made for us,
  /// which is ours to delete). Either way the sender only guarantees the URL
  /// for the duration of this call, so taking our own copy is what makes the
  /// path safe to hand to Dart.
  private static func copyIntoCache(_ url: URL) -> String? {
    let scoped = url.startAccessingSecurityScopedResource()
    defer {
      if scoped { url.stopAccessingSecurityScopedResource() }
    }

    let fileManager = FileManager.default
    // One directory per item: two shares can carry the same filename, and a
    // flat directory would silently overwrite the first.
    let destinationDir = fileManager.temporaryDirectory
      .appendingPathComponent(cacheDirName, isDirectory: true)
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let destination = destinationDir.appendingPathComponent(url.lastPathComponent)

    do {
      try fileManager.createDirectory(at: destinationDir, withIntermediateDirectories: true)
      try fileManager.copyItem(at: url, to: destination)
    } catch {
      NSLog("[wisp] could not copy shared item \(url.lastPathComponent): \(error)")
      return nil
    }

    // iOS hands "copy" style opens to us in Documents/Inbox and never cleans
    // that up itself, so drop the original now that we hold our own copy.
    if url.path.contains("/Documents/Inbox/") {
      try? fileManager.removeItem(at: url)
    }
    return destination.path
  }
}
