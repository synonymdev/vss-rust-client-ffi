#!/bin/bash

set -e  # Exit immediately if a command exits with a non-zero status.

echo "Starting iOS build process..."

# Define output directories
IOS_BINDINGS_DIR="./bindings/ios"
IOS_DIST_DIR="./dist/ios"
XCFRAMEWORK_NAME="VssRustClientFfi.xcframework"
XCFRAMEWORK_PATH="$IOS_DIST_DIR/$XCFRAMEWORK_NAME"
XCFRAMEWORK_ZIP_PATH="$IOS_DIST_DIR/$XCFRAMEWORK_NAME.zip"

# Remove previous release artifacts and ensure clean state
echo "Cleaning previous release artifacts..."
rm -rf "$IOS_DIST_DIR"
rm -rf ios/

# Create necessary directories
echo "Creating build directories..."
mkdir -p "$IOS_BINDINGS_DIR"
mkdir -p "$IOS_DIST_DIR"

# Set iOS deployment target
export IPHONEOS_DEPLOYMENT_TARGET=17.0

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
rm -f "$IOS_BINDINGS_DIR/vss_rust_client_ffi.swift"
rm -f "$IOS_BINDINGS_DIR/vss_rust_client_ffiFFI.h"
rm -f "$IOS_BINDINGS_DIR/vss_rust_client_ffiFFI.modulemap"
rm -f "$IOS_BINDINGS_DIR/module.modulemap"
rm -rf "$IOS_BINDINGS_DIR/Headers"

cargo run --bin uniffi-bindgen generate \
    --library ./target/aarch64-apple-ios/release/libvss_rust_client_ffi.a \
    --language swift \
    --out-dir "$IOS_BINDINGS_DIR" \
    || { echo "Failed to generate Swift bindings"; exit 1; }

# Handle modulemap file
echo "Handling modulemap file..."
if [ -f "$IOS_BINDINGS_DIR/vss_rust_client_ffiFFI.modulemap" ]; then
    mv "$IOS_BINDINGS_DIR/vss_rust_client_ffiFFI.modulemap" "$IOS_BINDINGS_DIR/module.modulemap"
else
    echo "Warning: modulemap file not found"
fi

# Clean up any temporary directories
echo "Cleaning up temporary directories..."
rm -rf "$IOS_DIST_DIR/ios-arm64"
rm -rf "$IOS_DIST_DIR/ios-arm64-sim"

# Package each static library as a framework bundle.
FRAMEWORK_NAME="vss_rust_client_ffiFFI"
FRAMEWORK_BUNDLE_ID="com.synonym.vss-rust-client-ffi-ffi"
create_framework() {
    local framework_dir="$1/$FRAMEWORK_NAME.framework"
    local library_path="$2"

    mkdir -p "$framework_dir/Headers" "$framework_dir/Modules"
    cp "$library_path" "$framework_dir/$FRAMEWORK_NAME"
    cp "$IOS_BINDINGS_DIR/vss_rust_client_ffiFFI.h" "$framework_dir/Headers/"
    sed 's/^module /framework module /' "$IOS_BINDINGS_DIR/module.modulemap" > "$framework_dir/Modules/module.modulemap"
    cat > "$framework_dir/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$FRAMEWORK_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$FRAMEWORK_BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$FRAMEWORK_NAME</string>
    <key>CFBundlePackageType</key>
    <string>FMWK</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
</dict>
</plist>
EOF
}

echo "Creating framework bundles..."
mkdir -p "$IOS_DIST_DIR/ios-arm64"
mkdir -p "$IOS_DIST_DIR/ios-arm64-sim"
create_framework "$IOS_DIST_DIR/ios-arm64" "./target/aarch64-apple-ios/release/libvss_rust_client_ffi.a"
create_framework "$IOS_DIST_DIR/ios-arm64-sim" "./target/aarch64-apple-ios-sim/release/libvss_rust_client_ffi.a"

# Create XCFramework
echo "Creating XCFramework..."
xcodebuild -create-xcframework \
    -framework "$IOS_DIST_DIR/ios-arm64-sim/$FRAMEWORK_NAME.framework" \
    -framework "$IOS_DIST_DIR/ios-arm64/$FRAMEWORK_NAME.framework" \
    -output "$XCFRAMEWORK_PATH" \
    || { echo "Failed to create XCFramework"; exit 1; }

# Clean up temporary directories
echo "Cleaning up temporary directories..."
rm -rf "$IOS_DIST_DIR/ios-arm64"
rm -rf "$IOS_DIST_DIR/ios-arm64-sim"

# Create zip file for distribution and checksum calculation
echo "Creating XCFramework zip file..."
rm -f "$XCFRAMEWORK_ZIP_PATH"
ditto -c -k --sequesterRsrc --keepParent "$XCFRAMEWORK_PATH" "$XCFRAMEWORK_ZIP_PATH" || { echo "Failed to create zip file"; exit 1; }

# Compute checksum
echo "Computing checksum..."
CHECKSUM=`swift package compute-checksum "$XCFRAMEWORK_ZIP_PATH"` || { echo "Failed to compute checksum"; exit 1; }
echo "New checksum: $CHECKSUM"

# Update Package.swift with the new checksum using Python script
echo "Updating Package.swift with new checksum..."
python3 ./update_package.py --checksum "$CHECKSUM" || { echo "Failed to update Package.swift"; exit 1; }

echo "iOS build process completed successfully!"
