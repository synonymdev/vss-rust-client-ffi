#!/bin/bash

set -e  # Exit immediately if a command exits with a non-zero status.

echo "Starting Android build process..."

# Define output directories
ANDROID_LIB_DIR="./bindings/android"
BASE_DIR="$ANDROID_LIB_DIR/src/main/kotlin/com/synonym/vssclient"
JNILIBS_DIR="$ANDROID_LIB_DIR/src/main/jniLibs"
NATIVE_DEBUG_SYMBOLS_ZIP="$ANDROID_LIB_DIR/native-debug-symbols.zip"

# Create output directories
mkdir -p "$BASE_DIR"
mkdir -p "$JNILIBS_DIR"

# Remove previous build
echo "Removing previous build..."
rm -rf "${BASE_DIR:?}"/*
rm -rf "${JNILIBS_DIR:?}"/*

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

export CARGO_PROFILE_RELEASE_DEBUG=2
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
export RUSTFLAGS="-C link-args=-Wl,-z,max-page-size=16384,-z,common-page-size=16384"
find_readelf() {
    if command -v llvm-readelf >/dev/null 2>&1; then
        command -v llvm-readelf
        return
    fi

    if command -v readelf >/dev/null 2>&1; then
        command -v readelf
        return
    fi

    for ndk_dir in "${ANDROID_NDK_ROOT:-}" "${ANDROID_NDK_HOME:-}" "${NDK_HOME:-}"; do
        if [ -z "$ndk_dir" ] || [ ! -d "$ndk_dir/toolchains/llvm/prebuilt" ]; then
            continue
        fi

        ndk_readelf=$(find "$ndk_dir/toolchains/llvm/prebuilt" -path '*/bin/llvm-readelf' | head -n 1)
        if [ -n "$ndk_readelf" ]; then
            echo "$ndk_readelf"
            return
        fi
    done

    echo "Error: llvm-readelf or readelf is required to validate Android native debug symbols"
    exit 1
}

find_strip() {
    if command -v llvm-strip >/dev/null 2>&1; then
        command -v llvm-strip
        return
    fi

    for ndk_dir in "${ANDROID_NDK_ROOT:-}" "${ANDROID_NDK_HOME:-}" "${NDK_HOME:-}"; do
        if [ -z "$ndk_dir" ] || [ ! -d "$ndk_dir/toolchains/llvm/prebuilt" ]; then
            continue
        fi

        ndk_strip=$(find "$ndk_dir/toolchains/llvm/prebuilt" -path '*/bin/llvm-strip' | head -n 1)
        if [ -n "$ndk_strip" ]; then
            echo "$ndk_strip"
            return
        fi
    done

    echo "Error: llvm-strip is required to strip Android native release libraries"
    exit 1
}

has_dwarf_debug_metadata() {
    "$READELF_BIN" -S "$1" | grep -Eq '\.debug_info'
}

has_dwarf_sections() {
    "$READELF_BIN" -S "$1" | grep -Eq '\.debug_'
}

readelf_program_headers() {
    if ! "$READELF_BIN" -W -l "$1"; then
        echo "Error: readelf must support wide program headers for Android native validation: $READELF_BIN"
        exit 1
    fi
}

validate_16kb_elf_segments() {
    lib="$1"
    headers=$(readelf_program_headers "$lib")
    alignments=$(printf '%s\n' "$headers" | awk '$1 == "LOAD" { print $NF }')
    if [ -z "$alignments" ]; then
        echo "Error: Android native library has no PT_LOAD segments: $lib"
        exit 1
    fi

    while read -r alignment; do
        if [ -z "$alignment" ]; then
            continue
        fi

        if [ "$((alignment))" -lt 16384 ]; then
            echo "Error: Android native library has PT_LOAD alignment $alignment below 0x4000: $lib"
            printf '%s\n' "$headers" | grep LOAD || true
            exit 1
        fi
    done <<EOF
$alignments
EOF

    relro_segments=$(printf '%s\n' "$headers" | awk '$1 == "GNU_RELRO" { print $3, $6 }')
    if [ -z "$relro_segments" ]; then
        echo "Error: Android native library has no PT_GNU_RELRO segment: $lib"
        exit 1
    fi

    while read -r virtual_address memory_size; do
        if [ -z "$virtual_address" ] || [ -z "$memory_size" ]; then
            continue
        fi

        relro_end=$((virtual_address + memory_size))
        if [ "$((relro_end % 16384))" -ne 0 ]; then
            printf 'Error: Android native library has PT_GNU_RELRO end 0x%x (vaddr %s + memsz %s), which is not 0x4000-aligned: %s\n' \
                "$relro_end" "$virtual_address" "$memory_size" "$lib"
            printf '%s\n' "$headers" | grep GNU_RELRO || true
            exit 1
        fi
    done <<EOF
$relro_segments
EOF
}

validate_android_library() {
    lib="$1"
    if ! has_dwarf_debug_metadata "$lib"; then
        echo "Error: Android native library has no .debug_info DWARF metadata: $lib"
        exit 1
    fi

    validate_16kb_elf_segments "$lib"
}

validate_stripped_android_library() {
    lib="$1"
    if has_dwarf_sections "$lib"; then
        echo "Error: Android release native library still contains .debug_* sections: $lib"
        exit 1
    fi

    validate_16kb_elf_segments "$lib"
}

validate_android_symbols() {
    READELF_BIN=$(find_readelf)

    for abi in armeabi-v7a arm64-v8a x86 x86_64; do
        lib="$JNILIBS_DIR/$abi/libvss_rust_client_ffi.so"
        if [ ! -f "$lib" ]; then
            echo "Error: Android native library missing at $lib"
            exit 1
        fi

        validate_android_library "$lib"
    done
}

create_native_debug_symbols_archive() {
    tmp_dir=$(mktemp -d)

    for abi in armeabi-v7a arm64-v8a x86 x86_64; do
        mkdir -p "$tmp_dir/$abi"
        cp "$JNILIBS_DIR/$abi/libvss_rust_client_ffi.so" "$tmp_dir/$abi/"
    done

    rm -f "$NATIVE_DEBUG_SYMBOLS_ZIP"
    archive_path="$PWD/$NATIVE_DEBUG_SYMBOLS_ZIP"
    if ! (
        cd "$tmp_dir"
        zip -qr "$archive_path" armeabi-v7a arm64-v8a x86 x86_64
    ); then
        rm -rf "$tmp_dir"
        exit 1
    fi
    if ! zip -T "$NATIVE_DEBUG_SYMBOLS_ZIP" >/dev/null; then
        rm -rf "$tmp_dir"
        exit 1
    fi
    rm -rf "$tmp_dir"
}

strip_android_libraries() {
    STRIP_BIN=$(find_strip)

    for abi in armeabi-v7a arm64-v8a x86 x86_64; do
        "$STRIP_BIN" --strip-unneeded "$JNILIBS_DIR/$abi/libvss_rust_client_ffi.so"
    done
}

validate_stripped_android_symbols() {
    READELF_BIN=$(find_readelf)

    for abi in armeabi-v7a arm64-v8a x86 x86_64; do
        validate_stripped_android_library "$JNILIBS_DIR/$abi/libvss_rust_client_ffi.so"
    done
}

validate_android_aar_symbols() {
    READELF_BIN=$(find_readelf)
    aar=$(find "$ANDROID_LIB_DIR" -path '*/build/outputs/aar/*release.aar' -print | head -n 1)
    if [ -z "$aar" ]; then
        echo "Error: Android release AAR missing under $ANDROID_LIB_DIR"
        exit 1
    fi

    tmp_dir=$(mktemp -d)
    unzip -q "$aar" -d "$tmp_dir"

    for abi in armeabi-v7a arm64-v8a x86 x86_64; do
        lib="$tmp_dir/jni/$abi/libvss_rust_client_ffi.so"
        if [ ! -f "$lib" ]; then
            echo "Error: Android release AAR native library missing at $lib"
            rm -rf "$tmp_dir"
            exit 1
        fi

        validate_stripped_android_library "$lib"
    done

    rm -rf "$tmp_dir"
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
create_native_debug_symbols_archive
strip_android_libraries
validate_stripped_android_symbols
unset CARGO_PROFILE_RELEASE_DEBUG
unset CARGO_PROFILE_RELEASE_STRIP
unset RUSTFLAGS

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
validate_android_aar_symbols

echo "Android build process completed successfully!"
