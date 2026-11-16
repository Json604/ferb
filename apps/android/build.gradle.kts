plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.roborazzi) apply false
}

// Release signing (kb PHN-406, PHN-499): an app module's release build is signed with the owner's own key, which never
// enters this repository. Where it is comes from outside the checkout: ~/.gradle/gradle.properties or the environment.
//   ferb.release.storeFile  (FERB_RELEASE_STORE_FILE)  the keystore (README "Release signing" makes one with keytool)
//   ferb.release.keyAlias   (FERB_RELEASE_KEY_ALIAS)   default "ferb"
//   ferb.release.password   (FERB_RELEASE_PASSWORD)    else read from the login Keychain item "ferb-android-release"
// Debug builds never need it. A release build without it stops with what to do instead of making an unsigned APK.
fun releaseSetting(name: String, env: String): String? =
    providers.gradleProperty("ferb.release.$name").orNull ?: providers.environmentVariable(env).orNull

val wantsRelease = gradle.startParameter.taskNames.any { it.contains("release", ignoreCase = true) }

subprojects {
    pluginManager.withPlugin("com.android.application") {
        val storeFile = releaseSetting("storeFile", "FERB_RELEASE_STORE_FILE")?.let { File(it.replaceFirst("~", System.getProperty("user.home"))) }
        val alias = releaseSetting("keyAlias", "FERB_RELEASE_KEY_ALIAS") ?: "ferb"
        // the Keychain is asked only for a release build, so a debug build never shows a Keychain prompt
        val password = if (storeFile == null || !wantsRelease) null else releaseSetting("password", "FERB_RELEASE_PASSWORD")
            ?: providers.exec {
                commandLine("security", "find-generic-password", "-s", "ferb-android-release", "-w")
                isIgnoreExitValue = true
            }.standardOutput.asText.get().trim().takeIf { it.isNotEmpty() }
        extensions.configure<com.android.build.api.dsl.ApplicationExtension> {
            if (storeFile != null && storeFile.isFile && password != null) {
                val owner = signingConfigs.create("ferbRelease") {
                    this.storeFile = storeFile
                    storeType = if (storeFile.name.endsWith(".p12") || storeFile.name.endsWith(".pfx")) "PKCS12" else null
                    storePassword = password
                    keyAlias = alias
                    keyPassword = password // PKCS12: the key's password is the store's
                    enableV2Signing = true
                    enableV3Signing = true
                }
                buildTypes.getByName("release").signingConfig = owner
            }
        }
        if (wantsRelease && (storeFile == null || !storeFile.isFile || password == null)) {
            val why = when {
                storeFile == null -> "ferb.release.storeFile is not set"
                !storeFile.isFile -> "the keystore $storeFile does not exist"
                else -> "no password (ferb.release.password, FERB_RELEASE_PASSWORD or the Keychain item ferb-android-release)"
            }
            // the first task of a release build: stops before minifying or packaging anything
            tasks.matching { it.name.startsWith("pre") && it.name.endsWith("ReleaseBuild") }.configureEach {
                doFirst { throw GradleException("Release signing is not set up: $why. See apps/android/README.md, Release signing.") }
            }
        }
    }
}
