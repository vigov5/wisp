# R8 keep rules for the release build (isMinifyEnabled + isShrinkResources).
#
# Most of what matters here is already covered automatically:
#  * Manifest-declared components (MainActivity, TransferKeepaliveService,
#    UsbTunnelVpnService) are kept by the AAPT-generated rules.
#  * AndroidX/plugin libraries ship their own consumer ProGuard rules.
#  * The Rust core is a native .so loaded through dart:ffi, so R8 (which only
#    processes Java/Kotlin bytecode) never touches it — no JNI keep rules needed.
# The rules below are belt-and-suspenders for the small surface R8 could still
# get wrong, and cost us almost nothing since the app's own Kotlin glue is tiny.

# Flutter embedding + plugin entry points. Flutter's own Gradle plugin adds
# these too; kept explicitly so a future engine tweak can't strip a callback.
-keep class io.flutter.embedding.** { *; }
-keep class io.flutter.plugin.** { *; }
-dontwarn io.flutter.**

# Our Kotlin glue: the Activity, the SAF fast-path, the USB AOA channel, and the
# keepalive / USB-tunnel Services. Android instantiates the Services by name and
# the platform channels are reached reflectively from the engine, so keep the
# whole (very small) package rather than chase individual entry points.
-keep class dev.vigov5.wisp.** { *; }

# Any class that carries native (JNI) methods — defensive; keep the class and
# its native method names so a native lookup can't miss.
-keepclasseswithmembernames class * {
    native <methods>;
}
