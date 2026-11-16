import java.io.File
import javax.inject.Inject
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.CacheableTask
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.OutputFile
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

// :core (stage 3 slice S9, kb T-1209): the phone library (engine/crates/ferb-mobile-ffi, UniFFI) built for Android by
// cargo-ndk (PHN-446), its generated Kotlin, and the Kotlin around it: the Vault (Keystore keys, the sealed store key
// and link state; PHN-448), the signers, the lifecycle object and the Android signals. README.md "The Rust build".

val engine: File = rootDir.resolve("../../engine").canonicalFile
val ffiCrate: File = engine.resolve("crates/ferb-mobile-ffi")
// One stable cargo folder for every Rust build of this project (ignored by git as build output): cargo's own
// fingerprints make a repeat build cost seconds.
val rustTarget: File = layout.buildDirectory.dir("rust").get().asFile
// The ABIs packed into the APK: arm64-v8a (every phone FERB targets, and the Apple-silicon emulator). Add x86_64 for an
// emulator on an Intel Mac with -Pferb.abis=arm64-v8a,x86_64 (kb PHN-495).
val ferbAbis: List<String> = (findProperty("ferb.abis") as String? ?: "arm64-v8a").split(",").map { it.trim() }.filter { it.isNotEmpty() }
val ndkVersionPinned = "28.2.13676358"

// Every file a Rust build of the phone library reads: the engine's crates (broad on purpose: a path dependency added
// later is covered; cargo itself decides what really rebuilds), the lock files, the patches and the toolchain.
val rustInputs = fileTree(engine) {
    include("crates/*/src/**", "crates/*/Cargo.toml", "crates/*/Cargo.lock", "crates/*/build.rs", "crates/*/uniffi.toml")
    include("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "patches/**")
}

/** Where rustup's cargo and cargo-ndk live: a Gradle daemon started from an app has no shell PATH. */
fun cargoPath(): String {
    val home = System.getProperty("user.home")
    val cargoBin = System.getenv("CARGO_HOME")?.let { "$it/bin" } ?: "$home/.cargo/bin"
    return listOf(cargoBin, System.getenv("PATH") ?: "/usr/bin:/bin").joinToString(File.pathSeparator)
}

/** engine/patches/apply.sh (idempotent) then one cargo command in the FFI crate's folder. */
abstract class RustTask @Inject constructor(@get:Internal val exec: ExecOperations) : DefaultTask() {
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:Internal abstract val crateDir: DirectoryProperty
    @get:Internal abstract val engineDir: DirectoryProperty
    @get:Internal abstract val targetDir: DirectoryProperty
    @get:Input abstract val toolPath: Property<String>

    protected fun cargo(args: List<String>, extraEnv: Map<String, String> = emptyMap()) {
        exec.exec {
            commandLine("sh", engineDir.get().file("patches/apply.sh").asFile.path)
            environment("PATH", toolPath.get())
        }
        exec.exec {
            workingDir = crateDir.get().asFile
            // the daemon's own PATH decides which program starts: name cargo by its full path
            val cargo = toolPath.get().split(File.pathSeparator).map { File(it, "cargo") }.firstOrNull { it.canExecute() }
                ?: error("cargo is not installed (rustup: https://rustup.rs)")
            commandLine(listOf(cargo.path) + args)
            environment("PATH", toolPath.get())
            environment("CARGO_TARGET_DIR", targetDir.get().asFile.path)
            extraEnv.forEach { (k, v) -> environment(k, v) }
        }
    }
}

/**
 * The phone library for each ABI with cargo-ndk, release profile (opt-level s, LTO, stripped: the crate's
 * [profile.release]), 16 kB-aligned LOAD segments; only libferb_mobile_ffi.so is copied (cargo also links the cdylibs
 * of dependencies that declare one).
 */
abstract class CargoNdkTask @Inject constructor(exec: ExecOperations) : RustTask(exec) {
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val apiLevel: Property<Int>
    @get:Input abstract val ndkVersion: Property<String>
    @get:Internal abstract val ndkDir: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    private val triples = mapOf(
        "arm64-v8a" to "aarch64-linux-android",
        "x86_64" to "x86_64-linux-android",
        "armeabi-v7a" to "armv7-linux-androideabi",
    )

    @TaskAction
    fun build() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        for (abi in abis.get()) {
            val triple = triples[abi] ?: error("No Rust target for the ABI $abi")
            val flagsVar = "CARGO_TARGET_${triple.uppercase().replace('-', '_')}_RUSTFLAGS"
            cargo(
                listOf("ndk", "-t", abi, "-P", apiLevel.get().toString(), "build", "--release", "--lib", "--locked"),
                mapOf(
                    "ANDROID_NDK_HOME" to ndkDir.get().asFile.path,
                    // NDK r28 aligns at 16 kB already; said here so a change of NDK cannot drop it (Android 15+ phones)
                    flagsVar to "-C link-arg=-Wl,-z,max-page-size=16384",
                ),
            )
            val so = targetDir.get().asFile.resolve("$triple/release/libferb_mobile_ffi.so")
            so.copyTo(out.resolve("$abi/libferb_mobile_ffi.so"), overwrite = true)
        }
    }
}

/** The host library (the bindings are generated from it, and the JVM tests load it) and the crate's own generator. */
abstract class CargoHostTask @Inject constructor(exec: ExecOperations) : RustTask(exec) {
    @get:OutputFile abstract val library: RegularFileProperty
    @get:OutputFile abstract val bindgen: RegularFileProperty

    @TaskAction
    fun build() = cargo(listOf("build", "--locked", "--features", "bindgen", "--lib", "--bin", "uniffi-bindgen"))
}

/**
 * The Kotlin bindings, generated by the crate's Rust generator in library mode (no Python). From the host library:
 * the stripped Android library has no UniFFI metadata left. The two are the same source and features, and the
 * bindings check every function's checksum against the loaded library when it loads, so a mismatch cannot pass.
 */
abstract class UniffiKotlinTask @Inject constructor(@get:Internal val exec: ExecOperations) : DefaultTask() {
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val library: RegularFileProperty
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val bindgen: RegularFileProperty
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val config: RegularFileProperty
    @get:Internal abstract val crateDir: DirectoryProperty
    @get:Input abstract val toolPath: Property<String>
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        exec.exec {
            // in the crate's folder: library mode finds uniffi.toml through cargo metadata
            workingDir = crateDir.get().asFile
            environment("PATH", toolPath.get())
            commandLine(
                bindgen.get().asFile.path, "generate", "--library", library.get().asFile.path,
                "--language", "kotlin", "--out-dir", out.path, "--no-format",
            )
        }
    }
}

val hostLibName = if (System.getProperty("os.name").startsWith("Mac")) "libferb_mobile_ffi.dylib" else "libferb_mobile_ffi.so"

val cargoHost = tasks.register<CargoHostTask>("cargoHost") {
    group = "rust"
    description = "Builds the host phone library and the UniFFI generator (dev profile)."
    sources.from(rustInputs)
    crateDir.set(ffiCrate)
    engineDir.set(engine)
    targetDir.set(rustTarget)
    toolPath.set(cargoPath())
    library.set(rustTarget.resolve("debug/$hostLibName"))
    bindgen.set(rustTarget.resolve("debug/uniffi-bindgen"))
}

val uniffiKotlin = tasks.register<UniffiKotlinTask>("uniffiKotlin") {
    group = "rust"
    description = "Generates the phone library's Kotlin bindings (com.ferb.mobile)."
    library.set(cargoHost.flatMap { it.library })
    bindgen.set(cargoHost.flatMap { it.bindgen })
    config.set(ffiCrate.resolve("uniffi.toml"))
    crateDir.set(ffiCrate)
    toolPath.set(cargoPath())
    outputDir.set(layout.buildDirectory.dir("generated/uniffi/kotlin"))
}

android {
    namespace = "com.ferb.core"
    compileSdk = 37
    ndkVersion = ndkVersionPinned
    defaultConfig {
        minSdk = 30
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
        // only the phone library's ABIs: JNA's aar also carries armeabi, mips and x86 dispatchers
        ndk { abiFilters += ferbAbis }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    // The fake Mac and the software keys serve the JVM tests and the instrumented ones alike.
    sourceSets["test"].kotlin.srcDir("src/sharedTest/kotlin")
    sourceSets["androidTest"].kotlin.srcDir("src/sharedTest/kotlin")
    testOptions {
        // android.jar's stubs answer defaults: the bindings read Build.VERSION.SDK_INT (0 on the JVM: JNA's cleaner)
        unitTests.isReturnDefaultValues = true
    }
    packaging {
        // the library and JNA's dispatcher stay uncompressed and page-aligned in the APK, loaded in place
        jniLibs.useLegacyPackaging = false
    }
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }

// kb PHN-495: debug and release APKs carry the same release-profile library (measured in journal 114: a dev-profile
// library is larger and gains little build time once cargo's cache is warm).
val cargoNdk = tasks.register<CargoNdkTask>("cargoNdk") {
    group = "rust"
    description = "Builds libferb_mobile_ffi.so for ${ferbAbis.joinToString()} with cargo-ndk (release profile)."
    sources.from(rustInputs)
    crateDir.set(ffiCrate)
    engineDir.set(engine)
    targetDir.set(rustTarget)
    toolPath.set(cargoPath())
    abis.set(ferbAbis)
    apiLevel.set(30)
    ndkVersion.set(ndkVersionPinned)
    ndkDir.set(androidComponents.sdkComponents.sdkDirectory.map { it.dir("ndk/$ndkVersionPinned") })
    outputDir.set(layout.buildDirectory.dir("rust-jni"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdk, CargoNdkTask::outputDir)
        variant.sources.java?.addGeneratedSourceDirectory(uniffiKotlin, UniffiKotlinTask::outputDir)
    }
}

// The JVM tests load the host library through JNA (the jar, which carries the Mac's dispatcher).
tasks.withType<Test>().configureEach {
    dependsOn(cargoHost)
    systemProperty("jna.library.path", rustTarget.resolve("debug").path)
}

dependencies {
    // JNA as Android's aar (its dispatcher for each ABI); the bindings' runtime. Apache-2.0 (chosen of its dual licence).
    implementation("${libs.jna.get()}@aar")
    implementation(libs.androidx.annotation)
    implementation(libs.coroutines.android)
    implementation(libs.lifecycle.process)

    testImplementation(libs.jna)
    testImplementation(libs.junit)
    testImplementation(libs.coroutines.core)

    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.core)
    androidTestImplementation(libs.androidx.test.junit)
    androidTestImplementation(libs.junit)
}
