allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

val newBuildDir: Directory =
    rootProject.layout.buildDirectory
        .dir("../../build")
        .get()
rootProject.layout.buildDirectory.value(newBuildDir)

subprojects {
    val newSubprojectBuildDir: Directory = newBuildDir.dir(project.name)
    project.layout.buildDirectory.value(newSubprojectBuildDir)
}
subprojects {
    project.evaluationDependsOn(":app")
}

// Pin every Android module to the one NDK the app resolves (flutter.ndkVersion).
//
// A module that compiles native code but declares no ndkVersion of its own
// silently falls back to the Android Gradle Plugin's bundled default, so the
// build ends up wanting two NDKs. Measured here with `app` on 28.2.13676358:
// wisp_bridge (the Rust core, via Cargokit), flutter_zxing (zxing-cpp via
// CMake) and camera_android_camerax all resolved to 27.0.12077973 instead.
//
// That costs a second multi-GB NDK download locally, and breaks the F-Droid
// build outright: its builder only has the NDK named in the recipe's `ndk:`
// field, and there is no second one to fall back to.
//
// :app is evaluated first (see evaluationDependsOn above), so its ndkVersion is
// already resolved by the time this runs. To re-measure, comment this block out
// and diff:
//   ./gradlew -q --init-script <script-printing-android.ndkVersion> help
subprojects {
    if (name == "app") return@subprojects
    afterEvaluate {
        val appNdk = rootProject.project(":app").extensions
            .findByType(com.android.build.gradle.BaseExtension::class.java)
            ?.ndkVersion
        val android = extensions.findByType(com.android.build.gradle.BaseExtension::class.java)
        if (appNdk != null && android != null && android.ndkVersion != appNdk) {
            logger.lifecycle("Pinning :$name NDK ${android.ndkVersion} -> $appNdk")
            android.ndkVersion = appNdk
        }
    }
}

tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
