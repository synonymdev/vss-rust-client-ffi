import java.util.Properties

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

afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("maven") {
                val mavenArtifactId = "vss-client-android"
                groupId = project.group.toString()
                artifactId = mavenArtifactId
                version = project.version.toString()

                from(components["release"])
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
