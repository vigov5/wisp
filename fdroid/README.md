# F-Droid

Everything needed to publish Wisp on [F-Droid](https://f-droid.org), which
builds every app from source on its own infrastructure and signs the result
with its own key.

- `ndkcheck.gradle` — Gradle init script that prints each Android module's
  resolved `ndkVersion` (see "One NDK, not two" below).
- `dev.vigov5.wisp.yml` — the build recipe. The authoritative copy lives in
  [fdroiddata](https://gitlab.com/fdroid/fdroiddata) as
  `metadata/dev.vigov5.wisp.yml`; this one is kept beside the source so the
  two can be diffed. Update both together.
- `../fastlane/metadata/android/en-US/` — the store listing (title, summary,
  description, changelogs, icon, feature graphic, screenshots). F-Droid reads
  this straight out of the repo at the tagged commit, so it needs no
  fdroiddata change to update.

## What the F-Droid build does differently

Two things, both forced by F-Droid's inclusion policy:

1. **No Google ML Kit.** QR pairing decodes with
   [zxing-cpp](https://github.com/zxing-cpp/zxing-cpp) through `flutter_zxing`
   rather than `mobile_scanner`, whose Android backend is the closed-source
   `com.google.mlkit:barcode-scanning`. This applies to *every* build, not just
   F-Droid's — there is one code path.
2. **No in-app updater.** `--dart-define=WISP_FDROID=true` sets
   `kIsFdroidBuild` (see `flutter/lib/platform/platform_features.dart`), which
   compiles out the startup release check against `api.github.com`, the Play
   Store deep link, and the whole Update group in Settings. F-Droid ships the
   APK, so F-Droid updates it.

Everything else — the Rust transfer core, USB/AOA, the rendezvous client — is
identical to the Play and GitHub builds.

## Releasing a new version

1. Tag the release as usual (`vX.Y.Z`), with `flutter/pubspec.yaml` carrying
   the new `version: X.Y.Z+<code>`.
2. Add `fastlane/metadata/android/en-US/changelogs/<versionCode>.txt`
   (≤ 500 characters — F-Droid truncates past that).
3. In fdroiddata, append a new entry to `Builds:` copied from the previous one
   with `versionName`, `versionCode` and `commit` updated, and bump
   `CurrentVersion` / `CurrentVersionCode`.

`AutoUpdateMode: Version` means F-Droid can usually write step 3 itself once
the tag appears; the manual edit is the fallback when the recipe itself has to
change.

## Testing the recipe before sending it

With [fdroidserver](https://f-droid.org/docs/Build_Server_Setup/) installed and
a checkout of fdroiddata:

```sh
cp fdroid/dev.vigov5.wisp.yml <fdroiddata>/metadata/
cd <fdroiddata>
fdroid readmeta
fdroid lint dev.vigov5.wisp
fdroid build dev.vigov5.wisp:32      # add --server for the clean VM build
```

To reproduce just the app side of it locally, without fdroidserver:

```sh
cd flutter
mv android/key.properties android/key.properties.bak   # F-Droid signs, we don't
flutter build apk --release --dart-define=WISP_FDROID=true
```

Then confirm no ML Kit survived in the APK:

```sh
unzip -l build/app/outputs/flutter-apk/app-release.apk | grep -ci mlkit   # want 0
```

## One NDK, not two

The recipe pins `ndk: r28c` (28.2.13676358), and `flutter/android/build.gradle.kts`
pins every Android module to whatever `:app` resolves so that one entry is
enough. Without that pin the native modules — `wisp_bridge`, `flutter_zxing`
and `camera_android_camerax` — fall back to the Android Gradle Plugin's own
default (27.0.12077973 as measured) while the app is on 28.2.13676358. Locally
that just means a second NDK download; on F-Droid's builder the second NDK does
not exist and the build fails.

If a Flutter upgrade moves `flutter.ndkVersion`, update `ndk:` in the recipe to
match. To check what each module resolves to:

```sh
cd flutter/android
./gradlew -q --init-script ../../fdroid/ndkcheck.gradle help
```

## Things that will trip the scanner

`fdroid build` refuses prebuilt binaries in the source tree. The recipe's `rm:`
list exists for this: `docs/` holds the published web receiver including
`docs/pkg/wisp_web_receiver_bg.wasm`, and a `.wasm` is a hard rejection. Before
tagging, check nothing new has landed:

```sh
git ls-files | grep -iE '\.(apk|aar|jar|class|dex|so|a|zip|gz|tgz|wasm)$'
```

Anything that shows up either gets removed from the repo or added to `rm:`.
