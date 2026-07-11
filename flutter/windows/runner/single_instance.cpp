#include "single_instance.h"

#include <string>
#include <vector>

namespace wisp_single_instance {

namespace {

// Must match win32_window.cpp's kWindowClassName and main.cpp's window title so
// FindWindowW locates the running Wisp host window.
constexpr const wchar_t* kWindowClassName = L"FLUTTER_RUNNER_WIN32_WINDOW";
constexpr const wchar_t* kWindowTitle = L"Wisp";
constexpr const wchar_t* kMutexName = L"Local\\WispSingleInstance";

// On cold-start multi-select, several processes race: the host wins the mutex
// but its window may not exist yet when the followers try to forward. Retry
// FindWindow briefly before giving up.
constexpr int kFindWindowRetries = 100;     // attempts
constexpr DWORD kFindWindowRetryDelayMs = 50;  // ~5s total

// Holds the single-instance mutex for the process lifetime. Released by the OS
// on process exit; never explicitly closed for the host.
HANDLE g_mutex = nullptr;

HWND FindHostWindow() {
  for (int i = 0; i < kFindWindowRetries; ++i) {
    HWND hwnd = FindWindowW(kWindowClassName, kWindowTitle);
    if (hwnd != nullptr) {
      return hwnd;
    }
    Sleep(kFindWindowRetryDelayMs);
  }
  return nullptr;
}

void BringToForeground(HWND hwnd) {
  if (IsIconic(hwnd)) {
    ShowWindow(hwnd, SW_RESTORE);
  }
  SetForegroundWindow(hwnd);
}

// Sends one path to the host window as a UTF-8 WM_COPYDATA payload.
void ForwardPath(HWND hwnd, const std::string& path) {
  COPYDATASTRUCT cds{};
  cds.dwData = kCopyDataMagic;
  // Include the terminating null so the receiver can treat lpData as a C string.
  cds.cbData = static_cast<DWORD>(path.size() + 1);
  cds.lpData = const_cast<char*>(path.c_str());
  SendMessageW(hwnd, WM_COPYDATA, 0, reinterpret_cast<LPARAM>(&cds));
}

}  // namespace

bool AcquireOrForward(const std::vector<std::string>& args) {
  g_mutex = CreateMutexW(nullptr, FALSE, kMutexName);
  const bool already_running =
      g_mutex != nullptr && GetLastError() == ERROR_ALREADY_EXISTS;

  if (!already_running) {
    // We are the host (or the mutex could not be created — fail open and run
    // normally rather than refusing to start).
    return false;
  }

  // Another instance owns the window. Separate real file/folder paths from our
  // own launch flags (e.g. --autostart): a flag must never be forwarded as a
  // "path", nor pull the window to the foreground.
  std::vector<std::string> paths;
  bool autostart = false;
  for (const std::string& arg : args) {
    if (arg == "--autostart") {
      autostart = true;
      continue;
    }
    if (arg.rfind("--", 0) == 0) {
      continue;  // skip any other flags
    }
    if (!arg.empty()) {
      paths.push_back(arg);
    }
  }

  // A login (auto-start) relaunch while Wisp is already running: do nothing and
  // exit quietly. Bringing the window to the foreground here would defeat the
  // point of starting minimized/hidden.
  if (autostart && paths.empty()) {
    return true;
  }

  // Locate the running window to forward "Send via Wisp" paths to it.
  HWND host = FindHostWindow();
  if (host == nullptr) {
    // Could not locate the host window. Fail open: let this instance run so the
    // user's click is not silently dropped.
    return false;
  }

  BringToForeground(host);
  for (const std::string& path : paths) {
    ForwardPath(host, path);
  }
  return true;
}

std::string DecodeForwardedPath(const COPYDATASTRUCT* data) {
  if (data == nullptr || data->dwData != kCopyDataMagic ||
      data->lpData == nullptr || data->cbData == 0) {
    return std::string();
  }
  // Payload is a null-terminated UTF-8 string; cbData includes the null.
  return std::string(static_cast<const char*>(data->lpData));
}

}  // namespace wisp_single_instance
