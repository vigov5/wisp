import java.util.Properties
import java.io.FileInputStream

plugins {
    id("com.android.application")
    id("kotlin-android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// Release signing config is loaded from android/key.properties (gitignored).
// CI writes that file from secrets before invoking `flutter build apk`.
// Local developers can either drop in their own key.properties or skip it
// and fall back to debug signing (the case is detected at runtime below).
val keystoreProperties = Properties().apply {
    val keystorePropertiesFile = rootProject.file("key.properties")
    if (keystorePropertiesFile.exists()) {
        load(FileInputStream(keystorePropertiesFile))
    }
}

android {
    namespace = "dev.vigov5.wisp"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        applicationId = "dev.vigov5.wisp"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        create("release") {
            val storeFilePath = keystoreProperties.getProperty("storeFile")
            if (storeFilePath != null) {
                // storeFile path is interpreted relative to flutter/android/
                // (the rootProject dir) so the keystore can live alongside
                // key.properties rather than inside the :app module.
                storeFile = rootProject.file(storeFilePath)
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            // Use the release signing config when key.properties is present
            // (CI + properly-set-up local dev); otherwise fall back to debug
            // signing so `flutter run --release` keeps working without a
            // keystore configured.
            signingConfig = if (keystoreProperties.getProperty("storeFile") != null) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
        }
        getByName("debug") {
            // Debug APKs build only the ABIs listed here, which also restricts
            // which Rust targets Cargokit compiles — a fat debug build (3 ABIs)
            // is slow and huge. Defaults to arm64-v8a (the physical test device).
            // To also build the x86_64 emulator ABI for a debug run, pass:
            //   flutter build apk --debug -PdebugAbis=arm64-v8a,x86_64
            // Release builds are unaffected and still ship every ABI.
            val debugAbis = (project.findProperty("debugAbis") as String?)
                ?.split(",")
                ?.map { it.trim() }
                ?.filter { it.isNotEmpty() }
                ?: listOf("arm64-v8a")
            ndk {
                abiFilters.clear()
                abiFilters.addAll(debugAbis)
            }
        }
    }
}

flutter {
    source = "../.."
}

dependencies {
    implementation("androidx.documentfile:documentfile:1.0.1")
    // lifecycleScope (auto-cancels on activity destroy) + Dispatchers.IO so
    // ACTION_SEND URI copies don't block the main thread on cold launch.
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
}
