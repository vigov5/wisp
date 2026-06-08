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
  explicit FlutterWindow(const flutter::DartProject& project);
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

  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // MethodChannel bridging the Windows shell integration (context-menu
  // register/unregister/status + forwarded "Send via Wisp" paths) to Dart.
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>>
      windows_integration_channel_;
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
