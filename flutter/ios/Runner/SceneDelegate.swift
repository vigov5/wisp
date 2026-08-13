import Flutter
import UIKit

class SceneDelegate: FlutterSceneDelegate {

  /// Cold start: the share launched the app, so the URLs ride in on the
  /// connection options. `super` first — it builds the Flutter window.
  override func scene(
    _ scene: UIScene,
    willConnectTo session: UISceneSession,
    options connectionOptions: UIScene.ConnectionOptions
  ) {
    super.scene(scene, willConnectTo: session, options: connectionOptions)
    ShareIntentHandler.handle(
      urls: connectionOptions.urlContexts.map { $0.url },
      isColdStart: true
    )
  }

  /// Warm start: Wisp was already running when the file was shared.
  override func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
    super.scene(scene, openURLContexts: URLContexts)
    ShareIntentHandler.handle(urls: URLContexts.map { $0.url }, isColdStart: false)
  }
}
