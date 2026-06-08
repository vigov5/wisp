#ifndef RUNNER_SINGLE_INSTANCE_H_
#define RUNNER_SINGLE_INSTANCE_H_

#include <windows.h>

#include <string>
#include <vector>

// Single-instance guard + path forwarding for the "Send via Wisp" context menu.
//
// Windows launches the app once per selected item ("<exe>" "%1"), so a
// multi-select fires several processes. We want exactly one Wisp window with all
// paths aggregated into one Send draft. The first process to start owns a named
// mutex and becomes the host; every later process finds the host window and
// forwards its path(s) via WM_COPYDATA, then exits.
namespace wisp_single_instance {

// Sentinel placed in COPYDATASTRUCT.dwData so the host can distinguish our
// forwarded paths from any other WM_COPYDATA traffic.
constexpr ULONG_PTR kCopyDataMagic = 0x57495350;  // 'WISP'

// Called early in wWinMain. Acquires the single-instance mutex.
//   - Returns true  => another instance already owns the mutex; `args` (file
//                      paths) were forwarded to its window. The caller should
//                      exit immediately without creating a window.
//   - Returns false => this process is the host; it keeps the mutex for its
//                      lifetime and should proceed to create the window. `args`
//                      still flow to Dart via set_dart_entrypoint_arguments.
bool AcquireOrForward(const std::vector<std::string>& args);

// Decodes a forwarded WM_COPYDATA payload (UTF-8) back into a path string.
// Returns empty when the payload is not one of ours.
std::string DecodeForwardedPath(const COPYDATASTRUCT* data);

}  // namespace wisp_single_instance

#endif  // RUNNER_SINGLE_INSTANCE_H_
