import Flutter
import UIKit

/// Bridges iOS "share to Wisp" / "Open in Wisp" file hand-offs onto the same
/// `dev.vigov5.wisp/share_intent` channel Android's ACTION_SEND uses, so the
/// Dart side (`platform/share_intent.dart`) needs no per-platform branch.
///
/// The cold/warm split mirrors Android exactly:
///   - Cold start (the share launched the app) stashes the paths; Dart drains
///     them once via `getInitialSharedFiles`. Invoking the channel here would
///     race the Dart handler, which isn't installed until the first frame.
///   - Warm start pushes straight through `onSharedFiles`.
enum ShareIntentHandler {
  private static let channelName = "dev.vigov5.wisp/share_intent"

  /// Cache subdirectory holding our copies of shared items.
  private static let inboxDirName = "wisp_shared"

  private static var channel: FlutterMethodChannel?

  /// Cold-start stash, handed to Dart once. Text has no iOS producer yet (the
  /// document-types path only carries files) but the channel contract matches
  /// Android's, so the Share Extension can fill it in later.
  private static var pendingFiles: [String] = []
  private static var pendingText: String?

  static func register(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: channelName, binaryMessenger: messenger)
    self.channel = channel
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "getInitialSharedFiles":
        let files = pendingFiles
        pendingFiles = []
        result(files)
      case "getInitialSharedText":
        let text = pendingText
        pendingText = nil
        result(text)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  /// Handles URLs delivered by the scene. `isColdStart` must be true when they
  /// arrived in `scene(_:willConnectTo:options:)`.
  static func handle(urls: [URL], isColdStart: Bool) {
    let paths = urls.compactMap { copyIntoCache($0) }
    guard !paths.isEmpty else { return }

    // On a warm start the Dart handler is already installed, so deliver now.
    // Falling back to the stash keeps a share from being dropped if the engine
    // somehow isn't wired yet.
    if !isColdStart, let channel = channel {
      channel.invokeMethod("onSharedFiles", arguments: paths)
    } else {
      pendingFiles.append(contentsOf: paths)
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
      .appendingPathComponent(inboxDirName, isDirectory: true)
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
