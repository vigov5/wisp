import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    // Registered against the same registry as the plugins so shares handed to
    // the scene have a channel to arrive on. Cold-start shares are stashed
    // natively until Dart asks for them, so registration order doesn't matter.
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "WispShareIntent") {
      ShareIntentHandler.register(messenger: registrar.messenger())
    }
  }
}
