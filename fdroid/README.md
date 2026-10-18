# F-Droid

Everything needed to publish Wisp on [F-Droid](https://f-droid.org), which
builds every app from source on its own infrastructure and signs the result
with its own key.

- `ndkcheck.gradle` — Gradle init script that prints each Android module's
  resolved `ndkVersion` (see "One NDK, not two" below).
- `dev.vigov5.wisp.yml` — the build recipe, byte-for-byte what belongs in
  [fdroiddata](https://gitlab.com/fdroid/fdroiddata) at
  `metadata/dev.vigov5.wisp.yml`. Copy it there unchanged; keep the two in
  sync. (It carries no header comment precisely so that stays true.)
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

## Why the recipe does what it does

`fdroid rewritemeta` is a canonical formatter and **strips comments inside list
items**, and CI fails the MR if the file is not already in that exact form. So
the recipe cannot carry its own explanation — it lives here instead. Keep the
YAML comment-free.

- **`rm: docs`** — `docs/` is the published web-receiver build and contains a
  prebuilt `docs/pkg/wisp_web_receiver_bg.wasm`. A `.wasm` is an unconditional
  rejection in F-Droid's scanner. Neither it nor the other removed directories
  (`web/`, and the non-Android Flutter targets) take part in an APK build.
- **The `rustup` symlink dance in `prebuild`** — the Rust core is compiled by
  Cargokit (flutter_rust_bridge), which always invokes cargo as `cargo +stable`
  and ignores `rust-toolchain.toml` and `rustup default`. Pointing the `stable`
  toolchain directory at the pinned 1.95.0 is what actually stops the build
  following whatever stable happens to be released that day.
- **`--dart-define=WISP_FDROID=true` in `build`** — compiles out the in-app
  updater: no startup release check against api.github.com and no Play Store
  deep link, so F-Droid owns updates. See "What the F-Droid build does
  differently" above.
- **`srclibs: flutter@3.41.9` is pinned, not `flutter@stable`** — the recipe
  runs `flutter pub get --enforce-lockfile`, and a handful of packages in
  `pubspec.lock` (`meta`, `matcher`, `test`, `test_api`, `test_core`,
  `vector_math`) are pinned by the Flutter SDK itself rather than by our
  pubspec. A different Flutter ships different versions of those, so
  `flutter@stable` makes the lockfile unsatisfiable the moment upstream
  Flutter moves. Pinning the SDK is what makes `--enforce-lockfile` work, and
  it is what makes the build reproducible. **This must match the Flutter that
  generated `flutter/pubspec.lock`.**
- **`scandelete: flutter/.pub-cache`** — `flutter pub get` populates a local
  pub cache; without this the scanner walks every downloaded package and dies
  on test fixtures inside them (`archive` alone ships ~4000 `.bin` files).
  Note the `flutter/` prefix: **`rm:` and `scandelete:` resolve against the
  repo root, while `prebuild:`/`build:` commands run in `subdir` (`flutter/`)**.
  So `PUB_CACHE=$(pwd)/.pub-cache` creates `flutter/.pub-cache`, and a bare
  `.pub-cache` here silently matches nothing — fdroid reports it as
  "Non-exist scandelete path" and then fails on thousands of scanner errors.

## Releasing a new version

1. Tag the release as usual (`vX.Y.Z`), with `flutter/pubspec.yaml` carrying
   the new `version: X.Y.Z+<code>`.
2. Add `fastlane/metadata/android/en-US/changelogs/<versionCode>.txt`
   (≤ 500 characters — F-Droid truncates past that).
3. If the Flutter SDK moved since the last release (`flutter --version` vs the
   `srclibs: flutter@X.Y.Z` in the recipe), update that pin to match and
   re-run `flutter pub get` so `pubspec.lock` and the pin agree. They must
   move together or the F-Droid build fails on `--enforce-lockfile`.
4. In fdroiddata, append a new entry to `Builds:` copied from the previous one
   with `versionName`, `versionCode` and `commit` updated, and bump
   `CurrentVersion` / `CurrentVersionCode`.

`AutoUpdateMode: Version` means F-Droid can usually write step 4 itself once
the tag appears; the manual edit is the fallback when the recipe itself has to
change.

## Testing the recipe before sending it

With [fdroidserver](https://f-droid.org/docs/Build_Server_Setup/) installed and
a checkout of fdroiddata:

```sh
cp fdroid/dev.vigov5.wisp.yml <fdroiddata>/metadata/
cd <fdroiddata>
fdroid readmeta
fdroid rewritemeta --check dev.vigov5.wisp   # CI fails on this one first
fdroid lint dev.vigov5.wisp
fdroid build dev.vigov5.wisp:34      # add --server for the clean VM build
```

`rewritemeta` is the one that bites: it reformats the file to a canonical form
and the MR pipeline rejects anything that differs, including comments inside
list items. If you have no fdroidserver, the pipeline prints the exact diff it
wants — apply that verbatim.

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
