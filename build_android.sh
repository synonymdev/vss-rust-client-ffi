#!/bin/bash

set -e  # Exit immediately if a command exits with a non-zero status.

echo "Starting Android build process..."

# Define output directories
ANDROID_LIB_DIR="./bindings/android"
BASE_DIR="$ANDROID_LIB_DIR/src/main/kotlin/com/synonym/vssclient"
JNILIBS_DIR="$ANDROID_LIB_DIR/src/main/jniLibs"

# Create output directories
mkdir -p "$BASE_DIR"
mkdir -p "$JNILIBS_DIR"

# Remove previous build
echo "Removing previous build..."
rm -rf "$BASE_DIR"/*
rm -rf "$JNILIBS_DIR"/*

# Cargo Build
echo "Building Rust libraries..."
cargo build

# Temporarily set crate-type for Android (restored at end)
cp Cargo.toml Cargo.toml.bak
sed -i.bak 's/crate-type = .*/crate-type = ["cdylib"]/' Cargo.toml
rm -f Cargo.toml.bak.bak
trap 'mv Cargo.toml.bak Cargo.toml' EXIT

# Build release
echo "Building release version..."
cargo build --release

export CARGO_PROFILE_RELEASE_STRIP=false

# Install the cargo-ndk version used by the mobile release scripts.
CARGO_NDK_VERSION="3.5.4"
if ! command -v cargo-ndk &> /dev/null || ! cargo ndk --version | grep -q "cargo-ndk $CARGO_NDK_VERSION"; then
    echo "Installing cargo-ndk $CARGO_NDK_VERSION..."
    cargo install cargo-ndk --version "$CARGO_NDK_VERSION" --locked --force
fi

# Check if Android NDK is available
if [ -z "$ANDROID_NDK_ROOT" ] && [ -z "$NDK_HOME" ]; then
    echo "Warning: ANDROID_NDK_ROOT or NDK_HOME not set. Attempting to find NDK..."
    
    # Common NDK locations
    POSSIBLE_NDK_PATHS=(
        "$HOME/Library/Android/sdk/ndk-bundle"
        "$HOME/Android/Sdk/ndk-bundle"
        "/usr/local/android-ndk"
        "/opt/android-ndk"
    )
    
    for path in "${POSSIBLE_NDK_PATHS[@]}"; do
        if [ -d "$path" ]; then
            export ANDROID_NDK_ROOT="$path"
            echo "Found NDK at: $ANDROID_NDK_ROOT"
            break
        fi
    done
    
    if [ -z "$ANDROID_NDK_ROOT" ]; then
        echo "Error: Android NDK not found. Please install Android NDK and set ANDROID_NDK_ROOT"
        echo "You can install it via Android Studio or download from https://developer.android.com/ndk/downloads"
        exit 1
    fi
fi

# Add Android targets
echo "Adding Android targets..."
rustup target add \
    aarch64-linux-android \
    armv7-linux-androideabi \
    i686-linux-android \
    x86_64-linux-android

# Build for all Android architectures
echo "Building for Android architectures..."
find_readelf() {
    if command -v llvm-readelf >/dev/null 2>&1; then
        command -v llvm-readelf
        return
    fi

    if command -v readelf >/dev/null 2>&1; then
        command -v readelf
        return
    fi

    echo "Error: llvm-readelf or readelf is required to validate Android native debug symbols"
    exit 1
}

has_debug_metadata() {
    "$READELF_BIN" -S "$1" | grep -Eq '\.(symtab|debug_|gnu_debugdata)'
}

validate_android_symbols() {
    READELF_BIN=$(find_readelf)

    for abi in armeabi-v7a arm64-v8a x86 x86_64; do
        lib="$JNILIBS_DIR/$abi/libvss_rust_client_ffi.so"
        if [ ! -f "$lib" ]; then
            echo "Error: Android native library missing at $lib"
            exit 1
        fi

        if ! has_debug_metadata "$lib"; then
            echo "Error: Android native library has no usable debug metadata: $lib"
            exit 1
        fi
    done
}

cargo ndk \
    -o "$JNILIBS_DIR" \
    --no-strip \
    --manifest-path ./Cargo.toml \
    -t armeabi-v7a \
    -t arm64-v8a \
    -t x86 \
    -t x86_64 \
    build --release

validate_android_symbols
unset CARGO_PROFILE_RELEASE_STRIP

# Generate Kotlin bindings
echo "Generating Kotlin bindings..."
case "$(uname -s)" in
    Darwin*) LIBRARY_PATH="./target/release/libvss_rust_client_ffi.dylib" ;;
    Linux*) LIBRARY_PATH="./target/release/libvss_rust_client_ffi.so" ;;
    *) LIBRARY_PATH="./target/release/libvss_rust_client_ffi.so" ;;
esac

# Check if the library file exists
if [ ! -f "$LIBRARY_PATH" ]; then
    echo "Error: Library file not found at $LIBRARY_PATH"
    echo "Available files in target/release:"
    ls -l ./target/release/
    exit 1
fi

# Create a temporary directory for initial generation
TMP_DIR=$(mktemp -d)

# Generate the bindings to temp directory first
cargo run --bin uniffi-bindgen generate \
    --library "$LIBRARY_PATH" \
    --language kotlin \
    --out-dir "$TMP_DIR"

# Move the Kotlin file from the nested directory to the final location
echo "Moving Kotlin file to final location..."
find "$TMP_DIR" -name "vss_rust_client_ffi.kt" -exec mv {} "$BASE_DIR/" \;

# Clean up temp directory and any remaining uniffi directories
echo "Cleaning up temporary files..."
rm -rf "$TMP_DIR"
rm -rf "$ANDROID_LIB_DIR/uniffi"

# Verify the file was moved correctly
if [ ! -f "$BASE_DIR/vss_rust_client_ffi.kt" ]; then
    echo "Error: Kotlin bindings were not moved correctly"
    echo "Contents of $BASE_DIR:"
    ls -la "$BASE_DIR"
    exit 1
fi

# Sync version
echo "Syncing version from Cargo.toml..."
CARGO_VERSION=$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/' | head -1)
sed -i.bak "s/^version=.*/version=$CARGO_VERSION/" "$ANDROID_LIB_DIR/gradle.properties"
rm -f "$ANDROID_LIB_DIR/gradle.properties.bak"

# Verify android library publish
echo "Testing android library publish to Maven Local..."
"$ANDROID_LIB_DIR"/gradlew --project-dir "$ANDROID_LIB_DIR" clean publishToMavenLocal

echo "Android build process completed successfully!"
