# VSS Rust Client Android

Android library exposing Kotlin bindings for the VSS Rust client FFI.

## Installation

### GitHub Packages

1) Setup your GitHub credentials

Create a GitHub PAT (Personal Access Token):
- Go to GitHub → Settings → Developer settings → Personal access tokens → Tokens (classic)
- Generate new token with scopes: `read:packages` (and `repo` if package/repo is private)
- Copy the token once and use it in the next steps:

Set env vars:
```sh
export GITHUB_ACTOR="your_github_username"
export GITHUB_TOKEN="your_pat_with_read:packages"
```

Or add to `~/.gradle/gradle.properties`:
```properties
gpr.user=<your_github_username>
gpr.key=<your_pat_with_read:packages>
```

#### 2. Add the GitHub Packages repository

```kotlin
// settings.gradle.kts
dependencyResolutionManagement {
  repositories {
    google()
    mavenCentral()
    maven {
      url = uri("https://maven.pkg.github.com/synonymdev/vss-rust-client-ffi")
      credentials {
          username = System.getenv("GITHUB_ACTOR") ?: providers.gradleProperty("gpr.user").orNull
          password = System.getenv("GITHUB_TOKEN") ?: providers.gradleProperty("gpr.key").orNull
      }
    }
  }
}
```

#### 3. Declare the dependency

```kotlin
// app/build.gradle.kts
dependencies {
  implementation("com.synonym:vss-client-android:<VERSION>")
  // example:
  // implementation("com.synonym:vss-client-android:0.1.0")
}
```
### Maven Local (development)

```kotlin
// settings.gradle.kts
dependencyResolutionManagement {
  repositories {
    mavenLocal()
    // others
  }
}

// build.gradle.kts
dependencies {
  implementation("com.synonym:vss-rust-client-ffi:<LOCAL_VERSION>")
}
```

---

## Publishing

**⚠️ Reminder:** Versions are immutable, bump for each publish.

### GitHub Actions

Create a GitHub Release with a new tag like `v0.1.0`. The workflow `gradle-publish.yml` will publish that version.

Or trigger the "Gradle Publish" workflow manually.

### Terminal

```sh
cd bindings/android
./gradlew publish -Pversion=0.1.0
```
