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

# Modify Cargo.toml
echo "Updating Cargo.toml..."
sed -i '' 's/crate-type = .*/crate-type = ["cdylib"]/' Cargo.toml

# Build release
echo "Building release version..."
cargo build --release

# Install cargo-ndk if not already installed
if ! command -v cargo-ndk &> /dev/null; then
    echo "Installing cargo-ndk..."
    cargo install cargo-ndk
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
cargo ndk \
    -o "$JNILIBS_DIR" \
    --manifest-path ./Cargo.toml \
    -t armeabi-v7a \
    -t arm64-v8a \
    -t x86 \
    -t x86_64 \
    build --release

# Generate Kotlin bindings
echo "Generating Kotlin bindings..."
LIBRARY_PATH="./target/release/libvss_rust_client_ffi.dylib"

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
