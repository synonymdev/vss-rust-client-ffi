import java.io.ByteArrayOutputStream
import java.io.File
import java.util.zip.ZipFile

plugins {
    id("com.android.library") version "8.5.2"
    id("org.jetbrains.kotlin.android") version "1.9.24"
    id("maven-publish")
    id("signing")
}

group = "com.synonym"
version = providers.gradleProperty("version").orNull ?: "0.0.0"

android {
    namespace = "com.synonym.vssclient"
    compileSdk = 34

    defaultConfig {
        minSdk = 21
        consumerProguardFiles("consumer-rules.pro")
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(file("proguard-android-optimize.txt"), file("proguard-rules.pro"))
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }
    kotlinOptions {
        jvmTarget = "11"
    }
    publishing {
        singleVariant("release") {
            withSourcesJar()
            withJavadocJar()
        }
    }
}

dependencies {
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
}

val androidNativeAbis = listOf("armeabi-v7a", "arm64-v8a", "x86", "x86_64")

fun executableFromPath(name: String): String? {
    return System.getenv("PATH")
        ?.split(File.pathSeparator)
        ?.asSequence()
        ?.map { File(it, name) }
        ?.firstOrNull { it.canExecute() }
        ?.absolutePath
}

fun findReadelf(): String {
    executableFromPath("llvm-readelf")?.let { return it }
    executableFromPath("readelf")?.let { return it }

    return listOf("ANDROID_NDK_ROOT", "ANDROID_NDK_HOME", "NDK_HOME")
        .mapNotNull { System.getenv(it) }
        .map { File(it, "toolchains/llvm/prebuilt") }
        .firstNotNullOfOrNull { prebuiltDir ->
            if (!prebuiltDir.isDirectory) return@firstNotNullOfOrNull null

            prebuiltDir
                .walkTopDown()
                .firstOrNull { it.name == "llvm-readelf" && it.canExecute() }
                ?.absolutePath
        }
        ?: throw GradleException(
            "llvm-readelf or readelf is required to validate Android native debug symbols"
        )
}

fun Project.runReadelf(readelf: String, vararg args: String): Pair<Int, String> {
    val stdout = ByteArrayOutputStream()
    val stderr = ByteArrayOutputStream()
    val result = exec {
        commandLine(readelf, *args)
        standardOutput = stdout
        errorOutput = stderr
        isIgnoreExitValue = true
    }

    return result.exitValue to stdout.toString().ifBlank { stderr.toString() }
}

fun String.parseElfAlignment(): Long {
    return if (startsWith("0x")) {
        removePrefix("0x").toLong(16)
    } else {
        toLong()
    }
}

val releaseAar = layout.buildDirectory.file("outputs/aar/${project.name}-release.aar")

val validateReleaseNativeLibraries by tasks.registering {
    group = "verification"
    description = "Validates every final AAR JNI library is stripped and 16 KB compatible."
    dependsOn("bundleReleaseAar")
    inputs.file(releaseAar)

    doLast {
        val readelf = findReadelf()
        val aar = releaseAar.get().asFile
        if (!aar.isFile) {
            throw GradleException("Android release AAR missing at '${aar.path}'")
        }

        ZipFile(aar).use { zip ->
            androidNativeAbis.forEach { abi ->
                val libraryPath = "jni/$abi/libvss_rust_client_ffi.so"
                val entry = zip.getEntry(libraryPath)
                    ?: throw GradleException(
                        "Android release AAR ABI '$abi' library missing at '$libraryPath' in '${aar.path}'"
                    )
                val lib = temporaryDir.resolve("$abi/libvss_rust_client_ffi.so")
                lib.parentFile.mkdirs()
                zip.getInputStream(entry).use { input ->
                    lib.outputStream().use { output -> input.copyTo(output) }
                }

                val (sectionsExit, sections) = runReadelf(readelf, "-S", lib.absolutePath)
                if (sectionsExit != 0) {
                    throw GradleException(
                        "Unable to inspect Android release AAR ABI '$abi' library '$libraryPath'"
                    )
                }
                if (Regex("""\.debug_""").containsMatchIn(sections)) {
                    throw GradleException(
                        "Android release AAR ABI '$abi' library '$libraryPath' still contains .debug_* sections"
                    )
                }

                val wideHeaders = runReadelf(readelf, "-W", "-l", lib.absolutePath)
                val headers = if (wideHeaders.first == 0) {
                    wideHeaders.second
                } else {
                    val fallbackHeaders = runReadelf(readelf, "-l", lib.absolutePath)
                    if (fallbackHeaders.first != 0) {
                        throw GradleException(
                            "Unable to inspect Android release AAR ABI '$abi' library '$libraryPath'"
                        )
                    }
                    fallbackHeaders.second
                }

                val programHeaders = headers
                    .lineSequence()
                    .map { it.trim().split(Regex("""\s+""")) }
                    .filter { it.isNotEmpty() }
                    .toList()

                val loadAlignments = programHeaders
                    .filter { it.first() == "LOAD" }
                    .map { it.last().parseElfAlignment() }
                if (loadAlignments.isEmpty() || loadAlignments.any { it < 16_384 }) {
                    val detected = loadAlignments.joinToString { "0x${it.toString(16)}" }
                    throw GradleException(
                        "Android release AAR ABI '$abi' library '$libraryPath' has PT_LOAD " +
                            "alignment(s) [$detected]; every alignment must be at least 0x4000"
                    )
                }

                val relroEnds = programHeaders
                    .filter { it.first() == "GNU_RELRO" && it.size >= 6 }
                    .map {
                        val virtualAddress = it[2].parseElfAlignment()
                        val memorySize = it[5].parseElfAlignment()
                        virtualAddress + memorySize
                    }
                if (relroEnds.isEmpty() || relroEnds.any { it % 16_384 != 0L }) {
                    val detected = relroEnds.joinToString { "0x${it.toString(16)}" }
                    throw GradleException(
                        "Android release AAR ABI '$abi' library '$libraryPath' has PT_GNU_RELRO " +
                            "end(s) [$detected]; every end must be 0x4000-aligned"
                    )
                }
            }
        }
    }
}

tasks.named("check") {
    dependsOn(validateReleaseNativeLibraries)
}

tasks.matching { it.name.startsWith("publish") }.configureEach {
    dependsOn(validateReleaseNativeLibraries)
}

afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("maven") {
                val mavenArtifactId = "vss-client-android"
                groupId = project.group.toString()
                artifactId = mavenArtifactId
                version = project.version.toString()

                from(components["release"])
                artifact(rootProject.layout.projectDirectory.file("native-debug-symbols.zip")) {
                    classifier = "native-debug-symbols"
                    extension = "zip"
                }
                pom {
                    name.set(mavenArtifactId)
                    description.set("VSS Rust Client Android bindings.")
                    url.set("https://github.com/synonymdev/vss-rust-client-ffi")
                    licenses {
                        license {
                            name.set("MIT")
                            url.set("https://github.com/synonymdev/vss-rust-client-ffi/blob/master/LICENSE")
                        }
                    }
                    developers {
                        developer {
                            id.set("synonymdev")
                            name.set("Synonym")
                            email.set("noreply@synonym.to")
                        }
                    }
                }
            }
        }
        repositories {
            maven {
                name = "GitHubPackages"
                val repo = System.getenv("GITHUB_REPO") 
                    ?: providers.gradleProperty("gpr.repo").orNull
                    ?: "synonymdev/vss-rust-client-ffi"
                url = uri("https://maven.pkg.github.com/$repo")
                credentials {
                    username = System.getenv("GITHUB_ACTOR") ?: providers.gradleProperty("gpr.user").orNull
                    password = System.getenv("GITHUB_TOKEN") ?: providers.gradleProperty("gpr.key").orNull
                }
            }
        }
    }
}
