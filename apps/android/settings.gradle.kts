// FERB for Android (stage 3 of the Rust rewrite, engine/PLAN.md): :design and its :gallery (S10), :core (S9) and
// :app, the app itself (S11).
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "FERB"
include(":design", ":gallery", ":core", ":app")
