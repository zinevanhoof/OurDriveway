import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// The release signing key: `storeFile`, `storePassword`, `keyAlias`, `keyPassword` in
// src-tauri/gen/android/keystore.properties, which is gitignored. The keystore itself lives
// outside the repo. Its SHA-256 is in public/.well-known/assetlinks.json, so App Links only
// verify for an APK signed with this exact key. See the README's Android section.
val keystoreProperties = Properties().apply {
    val propFile = rootProject.file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 36
    namespace = "com.gromit.our_driveway"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.gromit.our_driveway"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        create("release") {
            // Filled only when the key is there, so a debug build (`tauri android dev`)
            // still configures without it. A release build without it is stopped below.
            if (!keystoreProperties.isEmpty) {
                storeFile = file(keystoreProperties.getProperty("storeFile"))
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.getByName("release")
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    // 1.15.0 and not 1.17.0, which is current: from 1.16.0 webkit depends on
    // kotlin-stdlib 2.1.20, and this project's Kotlin Gradle plugin is 1.9.25, whose
    // compiler reads metadata up to 2.0.0 only. Gradle resolves the stdlib to the
    // highest requested version across the whole graph, so one such dependency fails
    // every Kotlin file in the module, `generated/` included. Raise this after the
    // Kotlin plugin goes to 2.x. core-splashscreen 1.2.0 is current and asks for
    // stdlib 2.0.21, which is inside what 1.9.25 can read.
    implementation("androidx.webkit:webkit:1.15.0")
    implementation("androidx.core:core-splashscreen:1.2.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")

// Without the key a release build still succeeds, as an unsigned APK no phone installs.
gradle.taskGraph.whenReady {
    if (keystoreProperties.isEmpty && allTasks.any { it.name.contains("Release") }) {
        throw GradleException(
            "src-tauri/gen/android/keystore.properties is missing, so a release build cannot " +
                "be signed. See the README's Android section."
        )
    }
}