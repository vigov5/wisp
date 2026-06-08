#ifndef RUNNER_WINDOWS_INTEGRATION_H_
#define RUNNER_WINDOWS_INTEGRATION_H_

// Per-user Windows shell integration: registers a "Send via Wisp" verb on the
// right-click context menu for files (the `*` class) and folders (the
// `Directory` class) under HKCU\Software\Classes. Per-user means no admin/UAC
// is required. The verb launches the app with the selected path as its first
// argument ("%1"); single-instance forwarding (see single_instance.h) collapses
// multiple launches into one window.
namespace wisp_integration {

// Writes the "Send via Wisp" verb for both files and folders, pointing the
// command at the current executable. Overwrites any existing entry so a moved
// install self-heals. Returns true on success.
bool RegisterContextMenu();

// Removes the "Send via Wisp" verb from both files and folders. Returns true if
// the entries are gone afterwards (already-absent counts as success).
bool UnregisterContextMenu();

// Returns true only when both verbs exist AND their command points at the
// current executable, so a stale entry from an older install reads as "needs
// re-register" rather than "enabled".
bool IsContextMenuRegistered();

}  // namespace wisp_integration

#endif  // RUNNER_WINDOWS_INTEGRATION_H_
