# fdroiddata MR — New app: Wisp (dev.vigov5.wisp)

## Branch
    dev.vigov5.wisp

## Commit message
    New app: Wisp

## MR title
    New app: Wisp (dev.vigov5.wisp)

## MR description

Wisp is a peer-to-peer file transfer app built on iroh. Devices find each
other on the LAN, or pair with a 6-character code or a QR code when
discovery fails; transfers are end-to-end encrypted and fall back to an
encrypted relay only when a direct connection cannot be established.
It can also transfer over a USB cable between two phones.

- Source: https://github.com/vigov5/wisp (MIT)
- Upstream is a fork of Drift by Samarth Verma.

Notes for reviewers:

- **No ML Kit.** Upstream replaced `mobile_scanner` with `flutter_zxing`
  (zxing-cpp over dart:ffi) in 2.8.1 specifically so the app could be built
  here; `com.google.mlkit` and `com.google.android.gms` no longer appear
  anywhere in the dependency graph. 2.8.1 is the first buildable version,
  which is why there is no earlier build entry.
- **In-app updater is compiled out.** The build passes
  `--dart-define=WISP_FDROID=true`, which removes the startup release check
  against api.github.com, the Play Store deep link, and the Update section
  in Settings, so F-Droid owns updates.
- **`rm: docs`** drops the project's published web-receiver build, which
  contains a prebuilt `.wasm` the scanner rejects. It takes no part in the
  Android build.
- **Rust toolchain is pinned.** The native core is compiled by Cargokit
  (flutter_rust_bridge), which always invokes `cargo +stable`. The prebuild
  symlinks `stable` onto the pinned 1.95.0 so the build does not drift with
  whatever stable happens to be that day.
- **Single NDK.** Upstream pins every Gradle module to the app's
  `flutter.ndkVersion`; without that, the Rust core and the zxing-cpp module
  fall back to AGP's bundled default and the build would want a second NDK
  that is not on the builder.

Metadata (summary, description, changelog, icon, screenshots) is supplied
from `fastlane/metadata/android/en-US/` in the upstream repo at the tag.
