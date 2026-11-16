plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// A debug-only look at :design on the emulator or a phone: every token, component and screen, with invented data.
android {
    namespace = "com.ferb.gallery"
    compileSdk = 37
    defaultConfig {
        applicationId = "com.ferb.gallery"
        minSdk = 30
        targetSdk = 37
        versionCode = 1
        versionName = "s10"
        ndk { abiFilters += listOf("arm64-v8a") }
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }

// No release build: the gallery is never shipped.
androidComponents { beforeVariants(selector().withBuildType("release")) { it.enable = false } }

dependencies {
    implementation(project(":design"))
    implementation(platform(libs.compose.bom))
    implementation(libs.activity.compose)
}
