plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.roborazzi)
}

android {
    namespace = "com.ferb.design"
    compileSdk = 37
    defaultConfig {
        minSdk = 30
        consumerProguardFiles()
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    testOptions {
        unitTests {
            isIncludeAndroidResources = true
            all { test ->
                test.maxHeapSize = "2g"
                // Robolectric's native graphics draws Compose the way a phone does; screenshots need it.
                test.systemProperty("robolectric.graphicsMode", "NATIVE")
                test.systemProperty("robolectric.pixelCopyRenderMode", "hardware")
            }
        }
    }
}

// The tests and goldens are the debug variant's; a release copy of them would only double the gate's time.
androidComponents {
    beforeVariants(selector().withBuildType("release")) {
        (it as com.android.build.api.variant.HasUnitTestBuilder).enableUnitTest = false
    }
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }

// The committed goldens (src/test/screenshots): `./gradlew :design:recordRoborazziDebug` rewrites them; a plain
// `:design:test` compares against them, so the gate fails when a screen drifts.
val roborazziTask = gradle.startParameter.taskNames.any { it.contains("Roborazzi") }
tasks.withType<Test>().configureEach {
    if (!roborazziTask) systemProperty("roborazzi.test.verify", "true")
}

dependencies {
    val bom = platform(libs.compose.bom)
    implementation(bom)
    api(libs.compose.ui)
    api(libs.compose.ui.graphics)
    api(libs.compose.foundation)
    implementation(libs.compose.animation)
    implementation(libs.compose.icons.extended)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)

    testImplementation(bom)
    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.roborazzi)
    testImplementation(libs.roborazzi.compose)
    testImplementation(libs.roborazzi.junit)
    testImplementation(libs.androidx.test.core)
    testImplementation(libs.androidx.test.junit)
    testImplementation(libs.compose.ui.test.junit4)
    debugImplementation(libs.compose.ui.test.manifest)
}
