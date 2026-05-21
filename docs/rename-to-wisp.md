# Rename plan — Drift → Wisp

Tracking the hard-fork rename from upstream `vsamarth/drift`. **Do not start
the code changes until the self-hosted rendezvous deploy is verified**
(`docs/deploy-rendezvous-server.md`), otherwise the renamed app has nowhere
to register.

## Goals

- New identity (`Wisp`) so users + stores don't confuse with upstream Drift.
- Stay fair to original author (Samarth Verma) per MIT license + courtesy.
- Resolve the `INSTALL_FAILED_UPDATE_INCOMPATIBLE` upgrade conflict once and
  for all (forced by `applicationId` change + new release keystore).
- Stop depending on `drift.samarthv.com` (upstream's rendezvous).

## License compliance (MIT)

MIT requires keeping the original copyright notice + the full license text.
**Append, don't replace.**

Edit `LICENSE`:

```
MIT License

Copyright (c) 2026 Samarth Verma
Copyright (c) 2026 vigov5 (Wisp fork)

Permission is hereby granted, free of charge, to any person obtaining a copy
[rest of the text unchanged]
```

## README acknowledgment

Replace the top banner of `README.md` so the fork heritage is visible at a
glance instead of buried in commit history:

```markdown
> Wisp is a friendly fork of [Drift](https://github.com/vsamarth/drift) by
> Samarth Verma. Heavy ❤️ to the upstream project — Wisp diverges in UX
> direction (Android-first polish, self-diagnose tooling) and runs its own
> rendezvous infrastructure so we don't lean on Drift's resources.
```

Then global-replace "Drift" → "Wisp" everywhere else in `README.md` **except**
the acknowledgment block above. The screenshots/gifs in `flutter/assets/`
can stay until new branded assets exist.

## Rename scope

### Must change (technical — required to avoid conflict + ship cleanly)

| Location | Change |
|---|---|
| `flutter/android/app/build.gradle.kts` | `applicationId = "com.example.drift"` → `dev.vigov5.wisp` |
| `flutter/android/app/src/main/AndroidManifest.xml` | `android:label="Drift"` → `"Wisp"` |
| `flutter/android/app/src/main/kotlin/com/example/drift/MainActivity.kt` | Move to `dev/vigov5/wisp/MainActivity.kt`, update `package` line |
| `flutter/android/app/src/main/kotlin/com/example/drift/TransferKeepaliveService.kt` | Same package move |
| `flutter/ios/Runner.xcodeproj/project.pbxproj` | `PRODUCT_BUNDLE_IDENTIFIER` → `dev.vigov5.wisp` (all 3 configurations) |
| `flutter/ios/Runner/Info.plist` | `CFBundleName`, `CFBundleDisplayName` → "Wisp" |
| `flutter/macos/Runner.xcodeproj/project.pbxproj` | Same bundle ID change |
| `flutter/macos/Runner/Configs/AppInfo.xcconfig` | `PRODUCT_BUNDLE_IDENTIFIER` + `PRODUCT_NAME` → Wisp |
| `flutter/windows/runner/Runner.rc` | Product name, file description, copyright → Wisp |
| `flutter/windows/runner/main.cpp` | Window class name → Wisp (if hardcoded) |
| Inno Setup script (look in `flutter/windows/` or `scripts/`) | App name + AppId GUID (regen with `[uuidgen]`) |
| `flutter/linux/CMakeLists.txt` | `BINARY_NAME` + `APPLICATION_ID` → Wisp |
| `flutter/linux/my_application.cc` | Window title (if hardcoded) |
| Method channels — `com.example.drift/transfer_keepalive` + `com.example.drift/file_picker` | Rename to match new `applicationId` on both Dart and Kotlin sides |
| Release keystore | Generate fresh — see `docs/deploy-rendezvous-server.md` is not the right place, will need a separate signing-setup doc |

### Should change (user-facing branding)

| Location | Change |
|---|---|
| `flutter/pubspec.yaml` | `description: "Send files..."` → mention Wisp |
| `flutter/assets/logo*.png` | Replace with Wisp logo when ready (defer if no design yet) |
| `flutter/lib/app/app_bootstrap.dart` | `/Download/Drift` → `/Download/Wisp` (new installs get a new folder; existing user data stays where it was) |
| `flutter/lib/features/settings/presentation/widgets/settings_page_body.dart` | Helper text mentioning "Drift" |
| `crates/core/src/rendezvous.rs::DEFAULT_RENDEZVOUS_URL` | `https://drift.samarthv.com` → your self-hosted URL (e.g. `https://wisp.<your-domain>`) |
| Anywhere in user-visible strings | `grep -ri "Drift" flutter/lib/ crates/app/src/ \| grep -v "//"` and judge case by case — internal logs are fine, status messages aren't |

### Optional / leave alone

| Location | Reason to keep |
|---|---|
| Rust crate names `drift_core`, `drift_app`, `drift_bridge` | Internal — not user-visible, renaming touches every Cargo.toml + every `use` + the FRB bridge. High churn, zero user benefit. |
| Iroh ALPN `b"drift/transfer/v1"` (`crates/core/src/protocol/mod.rs`) | **Keep.** This is the wire protocol identifier. Same value = a Drift user can still pair with a Wisp user. Different = isolated forks, harder for users who use both. |
| mDNS service type `_drift._udp.local.` | Same — keep for cross-fork LAN discovery. |
| Internal log prefixes `[receiver]`, `drift_app::receiver`, etc. | Internal observability only. |

**Stance: keep wire protocol + LAN discovery on the "drift" identifier so
Wisp ↔ Drift transfers still work cross-fork. The user-facing identity
diverges, the protocol does not.**

## Open decisions

Resolved so far:

| Question | Decision |
|---|---|
| Domain | `rendezvous.wisp.mooo.com` (FreeDNS) for the server. App display name = "Wisp". |
| Host | Oracle Cloud free tier — Ampere A1 ARM64 via Docker compose. |
| Logo source | `Wisp.svg` at repo root — solid `#06B6D4` (Tailwind cyan-500) circle + white arc + dot endpoints. |
| Theme palette | Migrated `drift_theme.dart` cyan family → Tailwind cyan-500/600/200 to match the icon. Hardcoded `0xFF4A8E9E`/`0xFF5FA7B7` in 4 widgets swapped to `kAccentCyanStrong`. Already committed-ready (no rename yet, just colour). |

Still open:

1. **`applicationId`**: `dev.vigov5.wisp` (GitHub username) is the default suggestion — confirm or supply a real reverse-DNS you own.
2. **Drift `Download/Drift` folder on existing users**: leave as is (old files stay accessible) or migrate to `Download/Wisp` (cleaner but requires copy on first launch — adds code).
3. **Wisp.svg → PNG asset conversion** (must do before `flutter pub run flutter_launcher_icons`):
   - Need a 1024×1024 PNG at `flutter/assets/logo.png` (and a rounded variant at `logo_rounded.png` for the README).
   - The repo doesn't ship `rsvg-convert` / Inkscape / ImageMagick, so this is a user-side step.
   - Easiest paths: open `Wisp.svg` in Figma → export PNG 1024px, or use an online SVG→PNG converter, or `inkscape Wisp.svg --export-png=flutter/assets/logo.png --export-width=1024` if Inkscape is installed.
   - Once the PNG exists: `flutter pub run flutter_launcher_icons` regenerates all platform launcher icons from the `flutter_launcher_icons:` block already configured in `pubspec.yaml`.

## Order of operations

1. ✅ Pick name (Wisp).
2. ✅ Pick domain (`rendezvous.wisp.mooo.com` for server).
3. ✅ Rebrand theme colours to match `Wisp.svg` (`kAccentCyan*` family + 4 hardcoded shades migrated to Tailwind cyan-500/600).
4. ✅ Deploy self-hosted rendezvous server + verified `/healthz`.
5. ✅ Convert `Wisp.svg` → PNGs (`wisp_square_logo.png`, `wisp_rounded_logo.png`) + regenerate launcher icons across all platforms.
6. ✅ Flip `DEFAULT_RENDEZVOUS_URL` in 4 spots (drift-core Rust, FRB sender, Dart default, Makefile). Existing per-install overrides preserved.
7. ✅ LICENSE — added Wisp-fork copyright alongside Samarth's, MIT obligations satisfied.
8. ✅ README — top acknowledgment block + global Drift→Wisp rename in the rest of the file.
9. ✅ `.deb` package Maintainer line updated (placeholder GitHub noreply — swap for a real email when comfortable).
10. **Generate release keystore for Wisp** (separate doc — defer until rename).
11. **Code rename touching all "Must change" rows above** (~15 files, 1–2 hours).
12. Rebuild Android APK with new keystore + new `applicationId`.
13. Release v1.0.0 — fresh major bump signals the identity break. Release notes:
    "Wisp was previously released as Drift fork v0.4.x. Please uninstall the
    old `com.example.drift` build before installing Wisp."
14. Notify upstream (GitHub issue on `vsamarth/drift`, courtesy ping).

## Files mentioning Drift today (rough census, for grep convenience)

```
grep -rin "drift" --include="*.dart" --include="*.kt" --include="*.swift" \
    --include="*.gradle*" --include="*.plist" --include="*.cmake" \
    --include="*.cc" --include="*.cpp" --include="*.rc" \
    flutter/
```

Plus `crates/app/src/diagnostics/`, `crates/core/src/rendezvous.rs`, and
the workflows under `.github/workflows/`.
