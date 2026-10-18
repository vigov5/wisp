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

# Flutter embedding + plugin entry points.
#
# There is deliberately no blanket `-keep class io.flutter.embedding.** { *; }`
# here. Keeping every member of every embedding class also keeps the method
# that constructs PlayStoreDeferredComponentManager, which drags
# com.google.android.play.core.* references into classes.dex — and F-Droid's
# APK scanner rejects those as a non-free dependency. Negating the class out
# of the keep rule is not enough, because the reference lives in the body of
# another kept method.
#
# Flutter itself ships no such rule: its own flutter_proguard_rules.pro only
# keeps FlutterPlugin implementations, and everything else the engine needs is
# reachable (MainActivity via the manifest, plugins via GeneratedPluginRegistrant,
# io.flutter.plugin.common.* from our Kotlin channels). Wisp uses no deferred
# components, so R8 can prove that path dead and drop it.
#
# If a future engine version strips something it should not, add a targeted
# keep for that class — not the package.
-keep class io.flutter.plugin.** { *; }
-dontwarn io.flutter.**
-dontwarn com.google.android.play.core.**

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
