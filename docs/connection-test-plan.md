# Connection Test — Implementation Plan

Self-diagnose checklist exposed in Settings → "Test connection".

## Scope (MVP — Phase 1)

8 checks across 5 groups:

| # | Check | Group | Source |
|---|---|---|---|
| 1 | Internet reachable (HEAD `gstatic/204`, 2s timeout) | network | Rust |
| 2 | Rendezvous `/health` 200 OK | rendezvous | Rust |
| 3 | LAN: mDNS self-scan (advertise + scan 3s, find self) | lan | Rust |
| 4 | P2P: Loopback iroh dial (self → self handshake) | p2p | Rust |
| 5 | Local network permission | permissions | Dart (`permission_handler`) |
| 6 | Notification permission | permissions | Dart (`permission_handler`) |
| 7 | `downloadRoot` writable (create+delete temp file) | local | Rust |
| 8 | Disk space ≥ 500 MB free | local | Rust |
| 9 | Windows firewall rule for `Wisp.exe` (Windows only) | local | Rust (`netsh`) |

Out of scope for MVP (deferred to Phase 2+): DNS check, gateway latency, relay/pkarr, NAT type, UDP STUN probe, identity stability, pairing cache, "Copy report".

## Decisions

- **Layout**: Grouped checklist + summary banner. Each row ticks live via `StreamSink<CheckResult>`. Skip downstream groups when upstream fails (no rendezvous if no internet, etc.).
- **mDNS self-scan**: use a dedicated service type `_wisp-diag._udp.local.` so production discovery (`_wisp._udp.local.`) is not polluted.
- **Permission handling**: add `permission_handler` package to `flutter/pubspec.yaml`.
- **Windows firewall**: shell out to `netsh advfirewall firewall show rule name=Wisp` and parse output.
- **No "Copy report"** in MVP.

## Data contract

```dart
enum CheckStatus { running, pass, warn, fail, skipped }

class CheckResult {
  final String id;          // 'internet.gstatic', 'lan.self_scan' ...
  final String groupId;     // 'network' | 'rendezvous' | 'lan' | 'p2p' | 'permissions' | 'local'
  final CheckStatus status;
  final String label;
  final String detail;
  final String? hint;
  final ({String label, ActionKind kind})? action;
}

enum ActionKind { openAppSettings, openUrl, copyDetail, retry }
```

## File structure

**Rust** (`crates/app/src/diagnostics/` + `flutter/rust/src/api/diagnostics.rs`):

```
crates/app/src/diagnostics/
  mod.rs       # orchestrator, takes &ReceiverService + StreamSink<CheckResult>
  network.rs   # internet HEAD probe (reqwest)
  rendezvous.rs
  lan.rs       # temp ServiceDaemon advertise + scan
  iroh.rs      # loopback dial
  local.rs     # disk write, free space, netsh firewall
flutter/rust/src/api/diagnostics.rs   # FFI surface
```

**Flutter** (`flutter/lib/features/diagnostics/`):

```
domain/check_result.dart
application/diagnostics_controller.dart   # Riverpod AsyncNotifier
application/diagnostics_source.dart       # Rust stream bridge
application/permission_probe.dart         # platform perms
presentation/connection_test_page.dart
presentation/widgets/check_row.dart
presentation/widgets/group_section.dart
presentation/widgets/summary_banner.dart
```

**Settings entry**: insert `SettingsSectionField` after "Discovery Server", before `ReliabilitySettingsSection` in `flutter/lib/features/settings/presentation/widgets/settings_page_body.dart`.

## UI mockup

```
+----------------------------------------+
| <- Connection Test         [Re-run]    |
+----------------------------------------+
| +------------------------------------+ |
| | ! 2 warnings - pairing co the cham | |
| | 5 pass . 2 warn . 1 fail   12s ago | |
| +------------------------------------+ |
|                                        |
| Network                          v     |
|  v  Internet reachable      32 ms      |
|  v  Rendezvous /health      42 ms      |
|                                        |
| LAN                              v     |
|  x  mDNS self-scan failed              |
|     Wi-Fi co the dang isolation mode   |
|     [ Open Wi-Fi settings ]            |
|                                        |
| P2P                              v     |
|  v  Loopback iroh dial      18 ms      |
|                                        |
| Permissions                      v     |
|  v  Local network granted              |
|  !  POST_NOTIFICATIONS denied          |
|     [ Open app settings ]              |
|                                        |
| Local                            v     |
|  v  Download folder writable           |
|  v  Disk 12.4 GB free                  |
|  v  Firewall rule present (Win)        |
+----------------------------------------+
```

Tap fail/warn row expands hint + action button. Group header reflects worst status in group. Top banner updates in real-time as results arrive.

## Implementation order

1. Skeleton + data model + page with hardcoded results (verify UI render for all 3 statuses)
2. Permissions check (Dart-only — quick win, validates state plumbing)
3. Network + Rendezvous (Rust — sets the FRB pattern for subsequent checks)
4. Local config group (disk, writable, firewall)
5. LAN self-scan (spin up temporary `ServiceDaemon`)
6. Loopback iroh dial (handle endpoint-not-ready case)
7. Skip/dependency logic + summary banner + polish

## Phase 2 backlog

DNS resolve, gateway latency, relay reachability (`endpoint.online()`), pkarr lookup, NAT type detection, UDP STUN probe, identity stable, pairing cache valid, mDNS receive count, "Copy report" (Markdown format).
