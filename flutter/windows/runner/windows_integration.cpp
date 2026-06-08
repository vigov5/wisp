#include "windows_integration.h"

#include <windows.h>

#include <string>

namespace wisp_integration {

namespace {

// Verb id (registry subkey name) and the label shown in the context menu.
constexpr const wchar_t* kVerbKey = L"WispSend";
constexpr const wchar_t* kVerbLabel = L"Send via Wisp";

// The two parent classes we attach the verb to: every file, and folders.
constexpr const wchar_t* kFileShellPath = L"Software\\Classes\\*\\shell\\WispSend";
constexpr const wchar_t* kDirShellPath =
    L"Software\\Classes\\Directory\\shell\\WispSend";

// Full path of the running executable.
std::wstring ExecutablePath() {
  wchar_t buffer[MAX_PATH];
  DWORD len = GetModuleFileNameW(nullptr, buffer, MAX_PATH);
  if (len == 0 || len == MAX_PATH) {
    return std::wstring();
  }
  return std::wstring(buffer, len);
}

// The command string written under <verb>\command: "<exe>" "%1"
std::wstring CommandValue(const std::wstring& exe) {
  return L"\"" + exe + L"\" \"%1\"";
}

bool SetStringValue(HKEY key, const wchar_t* name, const std::wstring& value) {
  // +1 to include the terminating null in the byte count, as REG_SZ expects.
  const DWORD bytes = static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t));
  return RegSetValueExW(key, name, 0, REG_SZ,
                        reinterpret_cast<const BYTE*>(value.c_str()),
                        bytes) == ERROR_SUCCESS;
}

// Writes <shellPath> with the verb label + icon, and <shellPath>\command with
// the launch command. Returns true on success.
bool WriteVerb(const wchar_t* shellPath, const std::wstring& exe) {
  HKEY verb = nullptr;
  if (RegCreateKeyExW(HKEY_CURRENT_USER, shellPath, 0, nullptr,
                      REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr, &verb,
                      nullptr) != ERROR_SUCCESS) {
    return false;
  }

  bool ok = SetStringValue(verb, nullptr, kVerbLabel);
  ok = ok && SetStringValue(verb, L"Icon", L"\"" + exe + L"\",0");

  HKEY command = nullptr;
  if (ok && RegCreateKeyExW(verb, L"command", 0, nullptr,
                            REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr,
                            &command, nullptr) == ERROR_SUCCESS) {
    ok = SetStringValue(command, nullptr, CommandValue(exe));
    RegCloseKey(command);
  } else {
    ok = false;
  }

  RegCloseKey(verb);
  return ok;
}

// Reads <shellPath>\command default value; empty string if absent.
std::wstring ReadCommand(const wchar_t* shellPath) {
  std::wstring commandPath = std::wstring(shellPath) + L"\\command";
  wchar_t buffer[1024];
  DWORD bytes = sizeof(buffer);
  if (RegGetValueW(HKEY_CURRENT_USER, commandPath.c_str(), nullptr,
                   RRF_RT_REG_SZ, nullptr, buffer, &bytes) != ERROR_SUCCESS) {
    return std::wstring();
  }
  return std::wstring(buffer);
}

bool VerbMatchesExe(const wchar_t* shellPath, const std::wstring& exe) {
  return ReadCommand(shellPath) == CommandValue(exe);
}

}  // namespace

bool RegisterContextMenu() {
  const std::wstring exe = ExecutablePath();
  if (exe.empty()) {
    return false;
  }
  bool files = WriteVerb(kFileShellPath, exe);
  bool dirs = WriteVerb(kDirShellPath, exe);
  return files && dirs;
}

bool UnregisterContextMenu() {
  // RegDeleteTreeW removes the verb and its `command` subkey. Treat a missing
  // key (ERROR_FILE_NOT_FOUND) as already-removed.
  LSTATUS f = RegDeleteTreeW(HKEY_CURRENT_USER, kFileShellPath);
  LSTATUS d = RegDeleteTreeW(HKEY_CURRENT_USER, kDirShellPath);
  bool fileOk = (f == ERROR_SUCCESS || f == ERROR_FILE_NOT_FOUND);
  bool dirOk = (d == ERROR_SUCCESS || d == ERROR_FILE_NOT_FOUND);
  return fileOk && dirOk;
}

bool IsContextMenuRegistered() {
  const std::wstring exe = ExecutablePath();
  if (exe.empty()) {
    return false;
  }
  return VerbMatchesExe(kFileShellPath, exe) &&
         VerbMatchesExe(kDirShellPath, exe);
}

}  // namespace wisp_integration
