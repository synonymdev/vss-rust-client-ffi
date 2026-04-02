#!/bin/bash

set -e  # Exit immediately if a command exits with a non-zero status.

echo "Starting iOS build process..."

# Remove previous builds and ensure clean state
echo "Cleaning previous builds..."
rm -rf bindings/ios/*
rm -rf ios/

# Create necessary directories
echo "Creating build directories..."
mkdir -p bindings/ios/

# Set iOS deployment target
export IPHONEOS_DEPLOYMENT_TARGET=13.4

# Cargo Build
echo "Building Rust libraries..."
cargo build --release

# Temporarily set crate-type for iOS (restored at end)
cp Cargo.toml Cargo.toml.bak
sed -i '' 's/crate-type = .*/crate-type = ["cdylib", "staticlib"]/' Cargo.toml
trap 'mv Cargo.toml.bak Cargo.toml' EXIT

# Build release
echo "Building release version..."
cargo build --release

# Add iOS targets
echo "Adding iOS targets..."
rustup target add aarch64-apple-ios-sim aarch64-apple-ios

# Build for iOS simulator and device
echo "Building for iOS targets..."
cargo build --release --target=aarch64-apple-ios-sim
cargo build --release --target=aarch64-apple-ios

# Generate Swift bindings
echo "Generating Swift bindings..."
# First, ensure any existing generated files are removed
rm -rf ./bindings/ios/vss_rust_client_ffi.swift
rm -rf ./bindings/ios/vss_rust_client_ffiFFI.h
rm -rf ./bindings/ios/vss_rust_client_ffiFFI.modulemap
rm -rf ./bindings/ios/Headers
rm -rf ./bindings/ios/ios-arm64
rm -rf ./bindings/ios/ios-arm64-sim

cargo run --bin uniffi-bindgen generate \
    --library ./target/aarch64-apple-ios/release/libvss_rust_client_ffi.a \
    --language swift \
    --out-dir ./bindings/ios \
    || { echo "Failed to generate Swift bindings"; exit 1; }

# Handle modulemap file — convert to framework module format for .framework bundle
echo "Handling modulemap file..."
if [ -f bindings/ios/vss_rust_client_ffiFFI.modulemap ]; then
    mv bindings/ios/vss_rust_client_ffiFFI.modulemap bindings/ios/module.modulemap
    # Convert "module X {" to "framework module X {" for framework bundle compatibility
    sed -i '' 's/^module /framework module /' bindings/ios/module.modulemap
else
    echo "Warning: modulemap file not found"
fi

# Clean up any existing XCFramework and temporary directories
echo "Cleaning up existing XCFramework..."
rm -rf "bindings/ios/VssRustClientFfi.xcframework"
rm -rf "bindings/ios/ios-arm64"
rm -rf "bindings/ios/ios-arm64-sim"

# Create framework bundles for each architecture
# Using .framework bundles (like LDK-node) avoids modulemap collision with other
# xcframeworks that also have flat Headers/module.modulemap (e.g., bitkit-core).
# Framework modulemaps go in Modules/, not Headers/, so there's no conflict.
FRAMEWORK_NAME="VssRustClientFfiFFI"
echo "Creating framework bundles..."
for ARCH_DIR in ios-arm64 ios-arm64-sim; do
    FW_DIR="bindings/ios/$ARCH_DIR/$FRAMEWORK_NAME.framework"
    mkdir -p "$FW_DIR/Headers"
    mkdir -p "$FW_DIR/Modules"
    cp bindings/ios/vss_rust_client_ffiFFI.h "$FW_DIR/Headers/"
    cp bindings/ios/module.modulemap "$FW_DIR/Modules/"
done

# Copy static libraries into framework bundles
cp ./target/aarch64-apple-ios/release/libvss_rust_client_ffi.a "bindings/ios/ios-arm64/$FRAMEWORK_NAME.framework/$FRAMEWORK_NAME"
cp ./target/aarch64-apple-ios-sim/release/libvss_rust_client_ffi.a "bindings/ios/ios-arm64-sim/$FRAMEWORK_NAME.framework/$FRAMEWORK_NAME"

# Create XCFramework from framework bundles
echo "Creating XCFramework..."
xcodebuild -create-xcframework \
    -framework "bindings/ios/ios-arm64-sim/$FRAMEWORK_NAME.framework" \
    -framework "bindings/ios/ios-arm64/$FRAMEWORK_NAME.framework" \
    -output "bindings/ios/VssRustClientFfi.xcframework" \
    || { echo "Failed to create XCFramework"; exit 1; }

# Clean up temporary directories
echo "Cleaning up temporary directories..."
rm -rf "bindings/ios/ios-arm64"
rm -rf "bindings/ios/ios-arm64-sim"

# Create zip file for distribution and checksum calculation
echo "Creating XCFramework zip file..."
rm -f ./bindings/ios/VssRustClientFfi.xcframework.zip
ditto -c -k --sequesterRsrc --keepParent ./bindings/ios/VssRustClientFfi.xcframework ./bindings/ios/VssRustClientFfi.xcframework.zip || { echo "Failed to create zip file"; exit 1; }

# Compute checksum
echo "Computing checksum..."
CHECKSUM=`swift package compute-checksum ./bindings/ios/VssRustClientFfi.xcframework.zip` || { echo "Failed to compute checksum"; exit 1; }
echo "New checksum: $CHECKSUM"

# Update Package.swift with the new checksum using Python script
echo "Updating Package.swift with new checksum..."
python3 ./update_package.py --checksum "$CHECKSUM" || { echo "Failed to update Package.swift"; exit 1; }

echo "iOS build process completed successfully!"