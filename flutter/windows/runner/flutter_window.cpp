#include "flutter_window.h"

#include <optional>
#include <string>

#include "flutter/generated_plugin_registrant.h"
#include "single_instance.h"
#include "windows_integration.h"

namespace {

// Channel name shared with the Dart side (lib/platform/windows_context_menu.dart).
constexpr const char* kWindowsIntegrationChannel =
    "dev.vigov5.wisp/windows_integration";

}  // namespace

FlutterWindow::FlutterWindow(const flutter::DartProject& project,
                             bool start_hidden)
    : project_(project), start_hidden_(start_hidden) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  // Bridge the Windows shell integration to Dart. Register/unregister/status
  // calls are handled synchronously here; forwarded "Send via Wisp" paths are
  // pushed to Dart via InvokeMethod("onSendViaWisp", [path]) on WM_COPYDATA.
  windows_integration_channel_ =
      std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
          flutter_controller_->engine()->messenger(),
          kWindowsIntegrationChannel,
          &flutter::StandardMethodCodec::GetInstance());
  windows_integration_channel_->SetMethodCallHandler(
      [](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
             result) {
        const std::string& method = call.method_name();
        if (method == "registerContextMenu") {
          result->Success(
              flutter::EncodableValue(wisp_integration::RegisterContextMenu()));
        } else if (method == "unregisterContextMenu") {
          result->Success(flutter::EncodableValue(
              wisp_integration::UnregisterContextMenu()));
        } else if (method == "isContextMenuRegistered") {
          result->Success(flutter::EncodableValue(
              wisp_integration::IsContextMenuRegistered()));
        } else {
          result->NotImplemented();
        }
      });

  // On a normal launch, show the window once the first frame is ready. On an
  // auto-start (login) launch, keep it hidden here and let the Dart side
  // (window_manager) settle the final state — stay in the tray, or minimize —
  // so Wisp doesn't flash a window in the user's face at login.
  if (!start_hidden_) {
    flutter_controller_->engine()->SetNextFrameCallback([&]() {
      this->Show();
    });
  }

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  windows_integration_channel_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

void FlutterWindow::SurfaceWindow() {
  // The forwarding process grants us foreground rights
  // (AllowSetForegroundWindow) so SetForegroundWindow below actually takes.
  // Handle both minimize paths: iconic (minimized to the taskbar) and fully
  // hidden (minimized to the tray).
  HWND hwnd = GetHandle();
  if (hwnd == nullptr) {
    return;
  }
  if (!IsWindowVisible(hwnd)) {
    ShowWindow(hwnd, SW_SHOW);
  }
  if (IsIconic(hwnd)) {
    ShowWindow(hwnd, SW_RESTORE);
  }
  SetForegroundWindow(hwnd);
}

void FlutterWindow::HandleForwardedPath(const COPYDATASTRUCT* copy_data) {
  const std::string path =
      wisp_single_instance::DecodeForwardedPath(copy_data);
  if (path.empty() || !windows_integration_channel_) {
    return;
  }
  // Bring the window forward so the user sees the draft. The Dart side also
  // restores via window_manager to keep its tracked state in sync.
  SurfaceWindow();
  windows_integration_channel_->InvokeMethod(
      "onSendViaWisp",
      std::make_unique<flutter::EncodableValue>(flutter::EncodableList{
          flutter::EncodableValue(path),
      }));
}

void FlutterWindow::HandleSurfaceRequest() {
  // A plain relaunch. Surface natively, then let Dart restore via
  // window_manager — that un-hides a tray-hidden window (which a native
  // SetForegroundWindow alone cannot) and re-syncs window_manager's state.
  SurfaceWindow();
  if (windows_integration_channel_) {
    windows_integration_channel_->InvokeMethod("onSurfaceRequested", nullptr);
  }
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
    case WM_COPYDATA: {
      auto* copy_data = reinterpret_cast<const COPYDATASTRUCT*>(lparam);
      if (copy_data != nullptr &&
          copy_data->dwData == wisp_single_instance::kSurfaceMagic) {
        HandleSurfaceRequest();
      } else {
        HandleForwardedPath(copy_data);
      }
      return TRUE;
    }
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
