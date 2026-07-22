#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <flutter/method_channel.h>
#include <flutter/standard_method_codec.h>

#include <memory>

#include "win32_window.h"

// A window that does nothing but host a Flutter view.
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|.
  // When |start_hidden| is true the window is not shown on the first frame
  // (used for auto-start/login launches); the Dart side then decides whether to
  // keep it in the tray or minimize it.
  explicit FlutterWindow(const flutter::DartProject& project,
                         bool start_hidden = false);
  virtual ~FlutterWindow();

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // Handles a forwarded "Send via Wisp" path delivered via WM_COPYDATA from a
  // second launch, dispatching it to Dart over windows_integration_channel_.
  void HandleForwardedPath(const COPYDATASTRUCT* copy_data);

  // Handles a surface request from a plain relaunch (WM_COPYDATA carrying
  // kSurfaceMagic and no path): brings the window forward and notifies Dart so
  // window_manager un-hides it from the tray and re-syncs its state.
  void HandleSurfaceRequest();

  // Brings the native window to the foreground, handling both minimize paths:
  // iconic (taskbar) and fully hidden (tray). Shared by the path + surface
  // handlers.
  void SurfaceWindow();

  // The project to run.
  flutter::DartProject project_;

  // When true, the window is not shown on the first frame (auto-start launch).
  bool start_hidden_ = false;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // MethodChannel bridging the Windows shell integration (context-menu
  // register/unregister/status + forwarded "Send via Wisp" paths) to Dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      windows_integration_channel_;
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
