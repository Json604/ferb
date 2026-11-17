plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.roborazzi)
}

// :app (stage 3 slice S11, kb T-1211): FERB for Android itself, on :core (the phone library, the vault and keys) and
// :design (the glass lab's phone design). README.md says how to build, test and install it.

val engine: File = rootDir.resolve("../../engine").canonicalFile
val ferbAbis: List<String> = (findProperty("ferb.abis") as String? ?: "arm64-v8a").split(",").map { it.trim() }.filter { it.isNotEmpty() }

android {
    namespace = "com.ferb.app"
    compileSdk = 37
    defaultConfig {
        // kb PHN-500: the app's id on Android (the lab's was local.ferb.link); an update must keep it
        applicationId = "com.ferb.phone"
        minSdk = 30
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0-s11"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // arm64-v8a only by default (every phone FERB targets, the Apple-silicon emulator); kb PHN-495
        ndk { abiFilters += ferbAbis }
    }
    buildFeatures { compose = true; buildConfig = true }
    buildTypes {
        release {
            // R8 shrinks and obfuscates; :core's consumer rules keep JNA and the UniFFI bindings (found by name)
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
        // A build that starts as a release does (kb PHN-633): R8, not debuggable, the baseline profile
        // (src/main/baseline-prof.txt); signed with the debug key, so it installs over the debug app on the same phone
        // and keeps its pairing (`adb install -r`). For measuring and for the owner's own phone; never published.
        create("benchmark") {
            initWith(getByName("release"))
            signingConfig = signingConfigs.getByName("debug")
            isDebuggable = false
            matchingFallbacks += listOf("release")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    packaging {
        // the phone library, JNA's and zxing-cpp's stay uncompressed and page-aligned, loaded in place
        jniLibs.useLegacyPackaging = false
    }
    testOptions {
        unitTests {
            isIncludeAndroidResources = true
            all { test ->
                test.maxHeapSize = "2g"
                // the screenshots draw as a phone does (kb PHN-455)
                test.systemProperty("robolectric.graphicsMode", "NATIVE")
                test.systemProperty("robolectric.pixelCopyRenderMode", "hardware")
                // the real phone library under Robolectric (MainThreadRuleTest, kb PHN-682): its bindings pick
                // Android's cleaner there, which on the JVM is the JDK's own
                test.jvmArgs("--add-exports=java.base/jdk.internal.ref=ALL-UNNAMED")
            }
        }
    }
    // the engine's chat fixtures (the scenarios a phone must draw) for the JVM tests
    sourceSets["test"].resources.srcDir(engine.resolve("fixtures/chat-entries"))
    // :core's fake Mac, software keys and in-memory protector: the real phone object in the app's own tests, on the
    // JVM (MainThreadRuleTest) and in the storm test on the emulator (kb PHN-682, PHN-683)
    sourceSets["test"].kotlin.srcDir(rootDir.resolve("core/src/sharedTest/kotlin"))
    sourceSets["androidTest"].kotlin.srcDir(rootDir.resolve("core/src/sharedTest/kotlin"))
}

// The JVM tests' stand-in phone, also for the frame measurements on an emulator or phone (FrameBudgetOnPhoneTest, kb
// PHN-713): the chat drawn by the app's own screens, without a Mac.
tasks.matching { it.name == "compileDebugAndroidTestKotlin" }.configureEach {
    (this as org.jetbrains.kotlin.gradle.tasks.KotlinCompile).source(file("src/test/kotlin/com/ferb/app/FakePhone.kt"))
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }

androidComponents {
    beforeVariants(selector().withBuildType("release")) {
        (it as com.android.build.api.variant.HasUnitTestBuilder).enableUnitTest = false
    }
    beforeVariants(selector().withBuildType("benchmark")) {
        (it as com.android.build.api.variant.HasUnitTestBuilder).enableUnitTest = false
    }
}

// The JVM tests that open the real phone core load the host library through JNA (:core's cargoHost builds it).
val roborazziTask = gradle.startParameter.taskNames.any { it.contains("Roborazzi") }
tasks.withType<Test>().configureEach {
    dependsOn(":core:cargoHost")
    systemProperty("jna.library.path", project(":core").layout.buildDirectory.dir("rust/debug").get().asFile.path)
    if (!roborazziTask) systemProperty("roborazzi.test.verify", "true")
    // the :design goldens the app's own screens are held to (the app must look like the lab)
    systemProperty("ferb.design.goldens", rootDir.resolve("design/src/test/screenshots").path)
}

dependencies {
    implementation(project(":core"))
    implementation(project(":design"))
    val bom = platform(libs.compose.bom)
    implementation(bom)
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.animation)
    implementation(libs.compose.icons.extended)
    implementation(libs.activity.compose)
    implementation(libs.lifecycle.process)
    implementation(libs.lifecycle.viewmodel.savedstate)
    implementation(libs.lifecycle.service)
    implementation(libs.coroutines.android)
    // the QR scanner (kb PHN-447): CameraX with zxing-cpp's reader; no Google Play services, no ML Kit
    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.view)
    implementation(libs.zxing.cpp)

    testImplementation(bom)
    testImplementation(libs.junit)
    testImplementation(libs.jna)
    testImplementation(libs.coroutines.test)
    testImplementation(libs.robolectric)
    testImplementation(libs.roborazzi)
    testImplementation(libs.roborazzi.compose)
    testImplementation(libs.roborazzi.junit)
    testImplementation(libs.androidx.test.core)
    testImplementation(libs.androidx.test.junit)
    testImplementation(libs.compose.ui.test.junit4)
    debugImplementation(libs.compose.ui.test.manifest)

    androidTestImplementation(bom)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.core)
    androidTestImplementation(libs.androidx.test.junit)
    androidTestImplementation(libs.junit)
    androidTestImplementation(libs.uiautomator)
    androidTestImplementation(libs.compose.ui.test.junit4)
}
